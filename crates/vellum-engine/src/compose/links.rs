//! Link annotations and named destinations.
//!
//! A link is kept as a rectangle on its page and a target, and written when
//! the document ends: a link may point at a page or a destination that only
//! exists later in the document.

use krilla::action::{Action, LinkAction};
use krilla::annotation::{Annotation, LinkAnnotation, Target};
use krilla::destination::{Destination, NamedDestination, XyzDestination};
use krilla::geom::{Point, Rect};
use krilla::outline::{Outline, OutlineNode};

use super::graphics::Matrix;
use super::{ComposeError, Composer};

/// Where a link goes.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkTarget {
    Uri(String),
    /// A page of this document, counted from zero.
    Page(usize),
    /// A destination named with [`Composer::add_named_destination`].
    Named(String),
}

/// A link on a page, its rectangle in page space.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkRecord {
    /// The document-wide handle a structure tree names the link by.
    pub id: usize,
    /// What a screen reader says for the link.
    pub alt: Option<String>,
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub target: LinkTarget,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NamedPoint {
    pub name: String,
    pub page: usize,
    pub x: f32,
    pub y: f32,
}

/// The schemes a link may open. pdfkit writes any URI, `javascript:` and
/// `file:` included; a reader that follows those runs code or opens local
/// files from a document someone else may have filled in.
const SCHEMES: [&str; 4] = ["http:", "https:", "mailto:", "tel:"];

pub fn check_uri(uri: &str) -> Result<(), ComposeError> {
    let lower = uri.trim_start().to_ascii_lowercase();
    if uri.len() > 8192 || !SCHEMES.iter().any(|scheme| lower.starts_with(scheme)) {
        return Err(ComposeError::new(
            "INVALID_LINK",
            format!(
                "{:?} is not a link a document may carry: use http, https, mailto or tel",
                uri.chars().take(80).collect::<String>()
            ),
        ));
    }
    if uri.chars().any(char::is_control) {
        return Err(ComposeError::new(
            "INVALID_LINK",
            "a link may not contain control characters",
        ));
    }
    Ok(())
}

pub(crate) fn bounding_box(
    matrix: Matrix,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) -> (f32, f32, f32, f32) {
    let [a, b, c, d, e, f] = matrix;
    let corners = [
        (x, y),
        (x + width, y),
        (x, y + height),
        (x + width, y + height),
    ];
    let mapped = corners.map(|(px, py)| (a * px + c * py + e, b * px + d * py + f));
    let left = mapped.iter().map(|p| p.0).fold(f32::INFINITY, f32::min);
    let right = mapped.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max);
    let top = mapped.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
    let bottom = mapped.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max);
    (left, top, right, bottom)
}

/// An entry of the document outline (its bookmarks).
#[derive(Debug, Clone, PartialEq)]
pub struct OutlineItem {
    pub title: String,
    /// From zero.
    pub page: usize,
    /// Where the entry opens the page, in page space.
    pub x: f32,
    pub y: f32,
    /// Shown with its children unfolded.
    pub open: bool,
    pub children: Vec<OutlineItem>,
}

/// Entries an outline may hold, all levels together.
pub const MAX_OUTLINE: usize = 100_000;

fn outline_node(
    item: &OutlineItem,
    pages: usize,
    depth: usize,
) -> Result<OutlineNode, ComposeError> {
    if depth > 64 {
        return Err(ComposeError::new(
            "INVALID_OUTLINE",
            "an outline nests at most 64 deep",
        ));
    }
    if item.page >= pages {
        return Err(ComposeError::new(
            "INVALID_OUTLINE",
            format!(
                "an outline entry goes to page {}, and the document has {pages}",
                item.page
            ),
        ));
    }
    let mut node = OutlineNode::new(
        item.title.clone(),
        XyzDestination::new(item.page, Point::from_xy(item.x, item.y)),
    )
    .with_open(item.open);
    for child in &item.children {
        node.push_child(outline_node(child, pages, depth + 1)?);
    }
    Ok(node)
}

/// At most this many links on one page, and named destinations in all.
pub const MAX_LINKS: usize = 100_000;

impl Composer {
    /// A link over a rectangle of the current page, in user space.
    /// A link over a rectangle; its handle names it in a structure tree.
    pub fn link(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        target: LinkTarget,
    ) -> Result<usize, ComposeError> {
        self.link_with_alt(x, y, width, height, target, None)
    }

    pub fn link_with_alt(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        target: LinkTarget,
        alt: Option<String>,
    ) -> Result<usize, ComposeError> {
        if let LinkTarget::Uri(uri) = &target {
            check_uri(uri)?;
        }
        if let LinkTarget::Named(name) = &target {
            check_name(name)?;
        }
        if ![x, y, width, height].iter().all(|value| value.is_finite()) {
            return Err(ComposeError::new(
                "INVALID_OPTIONS",
                "a link's rectangle must be finite numbers",
            ));
        }
        let ctm = self.state.ctm;
        let (left, top, right, bottom) = bounding_box(ctm, x, y, width, height);
        let id = self.link_count;
        // A screen reader names a link by its alternate text; the URL is the
        // least it can say.
        let alt = alt.or_else(|| match &target {
            LinkTarget::Uri(uri) => Some(uri.clone()),
            LinkTarget::Page(page) => Some(format!("Page {}", page + 1)),
            LinkTarget::Named(name) => Some(name.clone()),
        });
        let page = self.page_mut()?;
        if page.links.len() >= MAX_LINKS {
            return Err(ComposeError::new(
                "INVALID_OPTIONS",
                format!("a page takes at most {MAX_LINKS} links"),
            ));
        }
        page.links.push(LinkRecord {
            id,
            alt,
            left,
            top,
            right,
            bottom,
            target,
        });
        self.link_count += 1;
        Ok(id)
    }

    /// pdfkit's `addNamedDestination(name, 'XYZ', x, y)`: a place a link can
    /// go to by name. A name given again moves it, as a name tree keeps one.
    pub fn add_named_destination(
        &mut self,
        name: &str,
        x: f32,
        y: f32,
    ) -> Result<(), ComposeError> {
        check_name(name)?;
        let page = self.current_page_index().ok_or_else(|| {
            ComposeError::new("NO_PAGE", "there is no current page: call addPage() first")
        })?;
        let [a, b, c, d, e, f] = self.state.ctm;
        let (x, y) = (a * x + c * y + e, b * x + d * y + f);
        if let Some(existing) = self
            .destinations
            .iter_mut()
            .find(|point| point.name == name)
        {
            *existing = NamedPoint {
                name: name.to_string(),
                page,
                x,
                y,
            };
            return Ok(());
        }
        if self.destinations.len() >= MAX_LINKS {
            return Err(ComposeError::new(
                "INVALID_OPTIONS",
                format!("a document takes at most {MAX_LINKS} named destinations"),
            ));
        }
        self.destinations.push(NamedPoint {
            name: name.to_string(),
            page,
            x,
            y,
        });
        Ok(())
    }

    fn named(&self, name: &str) -> Option<NamedDestination> {
        self.destinations
            .iter()
            .find(|point| point.name == name)
            .map(|point| {
                NamedDestination::new(
                    point.name.clone(),
                    XyzDestination::new(point.page, Point::from_xy(point.x, point.y)),
                )
            })
    }

    /// The annotations of page `index`, checked against the whole document:
    /// krilla panics on a page index out of range.
    /// The links of a page as krilla annotations, each with its handle.
    pub(crate) fn annotations(
        &self,
        index: usize,
    ) -> Result<Vec<(usize, Annotation)>, ComposeError> {
        let Some(page) = self.pages.get(index) else {
            return Ok(Vec::new());
        };
        page.links
            .iter()
            .filter_map(|link| {
                let rect = Rect::from_ltrb(link.left, link.top, link.right, link.bottom)?;
                Some((rect, link))
            })
            .map(|(rect, link)| {
                let target = match &link.target {
                    LinkTarget::Uri(uri) => {
                        Target::Action(Action::Link(LinkAction::new(uri.clone())))
                    }
                    LinkTarget::Page(target) => {
                        if *target >= self.pages.len() {
                            return Err(ComposeError::new(
                                "INVALID_LINK",
                                format!(
                                    "a link goes to page {target}, and the document has {}",
                                    self.pages.len()
                                ),
                            ));
                        }
                        Target::Destination(Destination::Xyz(XyzDestination::new(
                            *target,
                            Point::from_xy(0.0, 0.0),
                        )))
                    }
                    LinkTarget::Named(name) => {
                        let named = self.named(name).ok_or_else(|| {
                            ComposeError::new(
                                "INVALID_LINK",
                                format!("a link goes to {name:?}, which no destination is named"),
                            )
                        })?;
                        Target::Destination(Destination::Named(named))
                    }
                };
                Ok((
                    link.id,
                    Annotation::new_link(LinkAnnotation::new(rect, target), link.alt.clone()),
                ))
            })
            .collect()
    }

    /// The document's outline, replacing any set before.
    pub fn set_outline(&mut self, items: Vec<OutlineItem>) -> Result<(), ComposeError> {
        fn count(items: &[OutlineItem]) -> usize {
            items.iter().map(|item| 1 + count(&item.children)).sum()
        }
        if count(&items) > MAX_OUTLINE {
            return Err(ComposeError::new(
                "INVALID_OUTLINE",
                format!("an outline holds at most {MAX_OUTLINE} entries"),
            ));
        }
        self.outline = items;
        Ok(())
    }

    pub(crate) fn krilla_outline(&self) -> Result<Option<Outline>, ComposeError> {
        if self.outline.is_empty() {
            return Ok(None);
        }
        let mut outline = Outline::new();
        for item in &self.outline {
            outline.push_child(outline_node(item, self.pages.len(), 0)?);
        }
        Ok(Some(outline))
    }

    pub(crate) fn named_destinations(&self) -> Vec<NamedDestination> {
        self.destinations
            .iter()
            .filter_map(|point| self.named(&point.name))
            .collect()
    }
}

fn check_name(name: &str) -> Result<(), ComposeError> {
    if name.is_empty() || name.len() > 1024 {
        return Err(ComposeError::new(
            "INVALID_OPTIONS",
            "a destination's name is 1 to 1024 bytes",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_safe_schemes_are_linked() {
        for uri in [
            "https://c9up.dev",
            "http://a.b",
            "mailto:x@y.z",
            "tel:+41000000000",
        ] {
            assert!(check_uri(uri).is_ok(), "{uri}");
        }
        for uri in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            "file:///etc/passwd",
            "data:text/html,x",
            "relative/path",
            "https://a.b/\u{7}",
        ] {
            assert!(check_uri(uri).is_err(), "{uri}");
        }
    }

    #[test]
    fn a_rotated_rectangle_is_boxed() {
        // 90° about the origin: (x, y) → (-y, x).
        let (left, top, right, bottom) =
            bounding_box([0.0, 1.0, -1.0, 0.0, 0.0, 0.0], 10.0, 0.0, 20.0, 5.0);
        assert_eq!((left, top, right, bottom), (-5.0, 10.0, 0.0, 30.0));
    }
}
