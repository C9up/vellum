//! Tagged PDF: marked content on the pages, and the structure tree it hangs
//! from.
//!
//! The tree is built on the TypeScript side, with pdfkit's `struct()` and
//! `markStructureContent()`. Here, content is marked as it is drawn — one
//! marked sequence at a time, since krilla cannot nest them — and the tree
//! is handed over before the document ends, its leaves naming the marked
//! sequences and the links they hold.

use std::num::{NonZeroU16, NonZeroU32};

use krilla::tagging::{
    Artifact, ArtifactType, ContentTag, Identifier, ListNumbering, Node, SpanTag, TableHeaderScope,
    Tag, TagGroup, TagKind, TagTree,
};

use super::ops::Op;
use super::{ComposeError, Composer};

/// What a marked sequence is.
#[derive(Debug, Clone, PartialEq)]
pub enum Mark {
    /// Not part of the logical content: headers, footers, decoration.
    Artifact(Option<ArtifactKind>),
    /// A span with its own properties.
    Span {
        lang: Option<String>,
        alt: Option<String>,
        expanded: Option<String>,
        actual: Option<String>,
    },
    /// Content a structure element holds.
    Structure,
}

/// pdfkit's artifact types, with the finer kinds PDF 2.0 adds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    Pagination,
    Layout,
    Page,
    Header,
    Footer,
    Watermark,
    PageNumber,
}

impl ArtifactKind {
    fn krilla(self) -> ArtifactType {
        match self {
            Self::Pagination => ArtifactType::PaginationOther,
            Self::Layout => ArtifactType::Layout,
            Self::Page => ArtifactType::Page,
            Self::Header => ArtifactType::Header,
            Self::Footer => ArtifactType::Footer,
            Self::Watermark => ArtifactType::Watermark,
            Self::PageNumber => ArtifactType::PageNumber,
        }
    }
}

/// One element of the structure tree, as the caller describes it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Element {
    /// pdfkit's structure type: `P`, `H1`…`H6`, `Table`, `Figure`…
    pub kind: String,
    pub title: Option<String>,
    pub lang: Option<String>,
    pub alt: Option<String>,
    pub expanded: Option<String>,
    pub actual: Option<String>,
    /// For a `TH`: `Row`, `Column` or `Both`.
    pub scope: Option<String>,
    pub row_span: Option<u32>,
    pub col_span: Option<u32>,
    pub children: Vec<Child>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Child {
    Element(Element),
    /// A marked sequence, by the handle `begin_mark` answered.
    Content(usize),
    /// A link, by the handle `link` answered.
    Annotation(usize),
}

/// The deepest a structure tree may go.
pub const MAX_DEPTH: usize = 256;
/// Marked sequences in one document.
pub const MAX_MARKS: usize = 1_000_000;

fn invalid(message: impl Into<String>) -> ComposeError {
    ComposeError::new("INVALID_STRUCTURE", message)
}

fn scope(name: Option<&str>) -> TableHeaderScope {
    match name {
        Some("Row") | Some("row") => TableHeaderScope::Row,
        Some("Both") | Some("both") => TableHeaderScope::Both,
        _ => TableHeaderScope::Column,
    }
}

/// krilla's tag for a pdfkit structure type, with its attributes.
fn tag(element: &Element) -> Result<TagKind, ComposeError> {
    let lang = element.lang.clone();
    let alt = element.alt.clone();
    let expanded = element.expanded.clone();
    let actual = element.actual.clone();
    macro_rules! with_common {
        ($tag:expr) => {
            $tag.with_lang(lang)
                .with_alt_text(alt)
                .with_expanded(expanded)
                .with_actual_text(actual)
                .into()
        };
    }
    let span = |value: Option<u32>| value.and_then(NonZeroU32::new);
    Ok(match element.kind.as_str() {
        "Document" | "Part" => with_common!(Tag::Part),
        "Art" | "Article" => with_common!(Tag::Article),
        "Sect" | "Section" => with_common!(Tag::Section),
        "Div" => with_common!(Tag::Div),
        "BlockQuote" => with_common!(Tag::BlockQuote),
        "Caption" => with_common!(Tag::Caption),
        "TOC" => with_common!(Tag::TOC),
        "TOCI" => with_common!(Tag::TOCI),
        "Index" => with_common!(Tag::Index),
        "P" => with_common!(Tag::P),
        "H" | "H1" | "H2" | "H3" | "H4" | "H5" | "H6" => {
            let level = element.kind[1..].parse::<u16>().unwrap_or(1);
            let level = NonZeroU16::new(level).unwrap_or(NonZeroU16::MIN);
            with_common!(Tag::Hn(level, element.title.clone()))
        }
        "L" => with_common!(Tag::L(ListNumbering::None)),
        "LI" => with_common!(Tag::LI),
        "Lbl" => with_common!(Tag::Lbl),
        "LBody" => with_common!(Tag::LBody),
        "Table" => with_common!(Tag::Table),
        "TR" => with_common!(Tag::TR),
        "TH" => with_common!(Tag::TH(scope(element.scope.as_deref()))
            .with_row_span(span(element.row_span))
            .with_col_span(span(element.col_span))),
        "TD" => with_common!(Tag::TD
            .with_row_span(span(element.row_span))
            .with_col_span(span(element.col_span))),
        "THead" => with_common!(Tag::THead),
        "TBody" => with_common!(Tag::TBody),
        "TFoot" => with_common!(Tag::TFoot),
        "Span" => with_common!(Tag::Span),
        "Quote" | "InlineQuote" => with_common!(Tag::InlineQuote),
        "Note" => with_common!(Tag::Note),
        "Reference" => with_common!(Tag::Reference),
        "BibEntry" => with_common!(Tag::BibEntry),
        "Code" => with_common!(Tag::Code),
        "Link" => with_common!(Tag::Link),
        "Annot" => with_common!(Tag::Annot),
        "Figure" => with_common!(Tag::Figure(element.alt.clone())),
        "Formula" => with_common!(Tag::Formula(element.alt.clone())),
        "Form" => with_common!(Tag::Form),
        "NonStruct" => with_common!(Tag::NonStruct),
        "Title" => with_common!(Tag::Title),
        "Strong" => with_common!(Tag::Strong),
        "Em" => with_common!(Tag::Em),
        other => {
            return Err(invalid(format!(
                "{other:?} is not a structure type a tagged PDF knows"
            )))
        }
    })
}

/// What replaying the pages answered for each mark and link.
#[derive(Default)]
pub struct Identifiers {
    /// Each mark, every sequence it was drawn as.
    pub marks: Vec<Vec<Identifier>>,
    pub links: Vec<Option<Identifier>>,
}

/// Which marks and links the tree has used already.
struct Used {
    marks: Vec<bool>,
    links: Vec<bool>,
}

fn node(
    element: &Element,
    identifiers: &Identifiers,
    depth: usize,
    used: &mut Used,
) -> Result<TagGroup, ComposeError> {
    if depth > MAX_DEPTH {
        return Err(invalid(format!(
            "a structure tree may be at most {MAX_DEPTH} deep"
        )));
    }
    let mut group = TagGroup::new(tag(element)?);
    for child in &element.children {
        match child {
            Child::Element(element) => {
                group.push(Node::Group(node(element, identifiers, depth + 1, used)?));
            }
            Child::Content(id) => {
                let sequences = identifiers.marks.get(*id).ok_or_else(|| {
                    invalid("a structure element holds content that was never marked")
                })?;
                // krilla panics on a leaf used twice in the tree.
                if std::mem::replace(&mut used.marks[*id], true) {
                    return Err(invalid(
                        "marked content belongs to one structure element only",
                    ));
                }
                for identifier in sequences {
                    group.push(Node::Leaf(*identifier));
                }
            }
            Child::Annotation(id) => {
                if let Some(Some(identifier)) = identifiers.links.get(*id) {
                    if !std::mem::replace(&mut used.links[*id], true) {
                        group.push(Node::Leaf(*identifier));
                    }
                }
            }
        }
    }
    Ok(group)
}

pub fn tag_tree(
    roots: &[Element],
    identifiers: &Identifiers,
    lang: Option<String>,
) -> Result<TagTree, ComposeError> {
    let mut tree = TagTree::new().with_lang(lang);
    let mut used = Used {
        marks: vec![false; identifiers.marks.len()],
        links: vec![false; identifiers.links.len()],
    };
    for root in roots {
        tree.push(Node::Group(node(root, identifiers, 0, &mut used)?));
    }
    // A link the tree does not hold still has to be in it: PDF/UA wants
    // every annotation under a Link element.
    for (id, identifier) in identifiers.links.iter().enumerate() {
        if let (Some(identifier), false) = (identifier, used.links[id]) {
            let mut link = TagGroup::new(Tag::Link);
            link.push(Node::Leaf(*identifier));
            tree.push(Node::Group(link));
        }
    }
    Ok(tree)
}

/// Which of `count` marks the tree holds. krilla numbers every structure
/// and span sequence and panics on one the tree leaves out; pdfkit writes it
/// and leaves it unreferenced. One no element holds is drawn untagged.
pub fn held_marks(roots: &[Element], count: usize) -> Vec<bool> {
    let mut held = vec![false; count];
    // Walked without recursion: the depth is checked later, by `tag_tree`.
    let mut pending: Vec<&Element> = roots.iter().collect();
    while let Some(element) = pending.pop() {
        for child in &element.children {
            match child {
                Child::Element(element) => pending.push(element),
                Child::Content(id) => {
                    if let Some(slot) = held.get_mut(*id) {
                        *slot = true;
                    }
                }
                Child::Annotation(_) => {}
            }
        }
    }
    held
}

pub fn content_tag(mark: &Mark) -> ContentTag<'_> {
    match mark {
        Mark::Artifact(kind) => ContentTag::Artifact(Artifact::new(
            kind.map_or(ArtifactType::Other, ArtifactKind::krilla),
            None,
        )),
        Mark::Span {
            lang,
            alt,
            expanded,
            actual,
        } => ContentTag::Span(
            SpanTag::empty()
                .with_lang(lang.as_deref())
                .with_alt_text(alt.as_deref())
                .with_expanded(expanded.as_deref())
                .with_actual_text(actual.as_deref()),
        ),
        Mark::Structure => ContentTag::Other,
    }
}

impl Composer {
    /// Begin a marked sequence and answer its handle. Sequences do not
    /// nest: krilla cannot write them nested.
    pub fn begin_mark(&mut self, mark: Mark) -> Result<usize, ComposeError> {
        if self.open_mark.is_some() {
            return Err(invalid(
                "marked content does not nest: end the open one first",
            ));
        }
        if self.marks.len() >= MAX_MARKS {
            return Err(invalid(format!(
                "a document takes at most {MAX_MARKS} marked sequences"
            )));
        }
        self.marks.push(mark);
        let id = self.marks.len() - 1;
        self.page_mut()?.ops.push(Op::BeginMark(id));
        self.open_mark = Some(id);
        Ok(id)
    }

    /// Carry on an open mark: the same handle, a new sequence on this page.
    pub fn continue_mark(&mut self, id: usize) -> Result<(), ComposeError> {
        if self.open_mark.is_some() {
            return Err(invalid(
                "marked content does not nest: end the open one first",
            ));
        }
        if id >= self.marks.len() {
            return Err(invalid("no marked content has this handle"));
        }
        self.page_mut()?.ops.push(Op::BeginMark(id));
        self.open_mark = Some(id);
        Ok(())
    }

    pub fn end_mark(&mut self) -> Result<(), ComposeError> {
        if self.open_mark.take().is_none() {
            return Err(invalid("there is no marked content to end"));
        }
        self.page_mut()?.ops.push(Op::EndMark);
        Ok(())
    }

    /// The structure tree, written when the document ends.
    pub fn set_structure(&mut self, roots: Vec<Element>) -> Result<(), ComposeError> {
        self.structure = roots;
        Ok(())
    }

    pub fn open_mark(&self) -> Option<usize> {
        self.open_mark
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pdfkit_structure_type_maps_to_a_tag() {
        for kind in [
            "Document",
            "Part",
            "Art",
            "Sect",
            "Div",
            "BlockQuote",
            "Caption",
            "TOC",
            "TOCI",
            "Index",
            "P",
            "H",
            "H1",
            "H6",
            "L",
            "LI",
            "Lbl",
            "LBody",
            "Table",
            "TR",
            "TH",
            "TD",
            "THead",
            "TBody",
            "TFoot",
            "Span",
            "Quote",
            "Note",
            "Reference",
            "BibEntry",
            "Code",
            "Link",
            "Annot",
            "Figure",
            "Formula",
            "Form",
            "NonStruct",
        ] {
            let element = Element {
                kind: kind.into(),
                ..Element::default()
            };
            assert!(tag(&element).is_ok(), "{kind}");
        }
        let unknown = Element {
            kind: "Blink".into(),
            ..Element::default()
        };
        assert!(tag(&unknown).is_err());
    }

    #[test]
    fn content_no_element_holds_is_written_untagged() {
        use crate::compose::{DocumentSettings, PageSetup, Sides};
        let mut document = Composer::new(DocumentSettings {
            tagged: true,
            ..DocumentSettings::default()
        })
        .unwrap();
        let margins = Sides {
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        };
        document
            .add_page(PageSetup {
                width: 100.0,
                height: 100.0,
                margins,
                user_unit: 1.0,
            })
            .unwrap();
        // pdfkit lets a sequence be marked and never attached.
        document.begin_mark(Mark::Structure).unwrap();
        document.end_mark().unwrap();
        let held = document.begin_mark(Mark::Structure).unwrap();
        document.end_mark().unwrap();
        document
            .set_structure(vec![Element {
                kind: "P".into(),
                children: vec![Child::Content(held)],
                ..Element::default()
            }])
            .unwrap();
        assert_eq!(
            held_marks(&document.structure, document.marks.len()),
            [false, true]
        );
        let bytes = document.end().expect("written, not a panic");
        assert!(lopdf::Document::load_mem(&bytes).is_ok());
    }
}
