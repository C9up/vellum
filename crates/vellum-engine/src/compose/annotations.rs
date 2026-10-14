//! Embedded files, annotations other than links, and destinations other
//! than XYZ.
//!
//! krilla embeds files at document level and writes link annotations only.
//! Notes, markup, shapes, file attachments on a page and the other views a
//! destination can open are added to the written document by the finishing
//! pass, with lopdf. None of them carries an appearance stream — pdfkit
//! writes none either — so they are refused under PDF/A and PDF/UA, which
//! require one, rather than written into a document that would then fail.

use std::sync::Arc;

use krilla::embed::{AssociationKind, EmbeddedFile, MimeType};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use super::{text_string, ComposeError, Composer, Instant};

/// Larger than any file a document has a use for carrying.
pub const MAX_FILE_BYTES: usize = 64 * 1024 * 1024;
/// Files, and annotations other than links, each, in one document.
pub const MAX_EXTRAS: usize = 100_000;

fn invalid(message: impl Into<String>) -> ComposeError {
    ComposeError::new("INVALID_OPTIONS", message)
}

/// How an embedded file relates to the document (PDF/A-3 `AFRelationship`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Relationship {
    Source,
    Data,
    Alternative,
    Supplement,
    #[default]
    Unspecified,
}

impl Relationship {
    pub fn parse(name: &str) -> Result<Self, ComposeError> {
        Ok(match name {
            "Source" => Self::Source,
            "Data" => Self::Data,
            "Alternative" => Self::Alternative,
            "Supplement" => Self::Supplement,
            "Unspecified" => Self::Unspecified,
            other => {
                return Err(invalid(format!(
                    "relationship {other:?} is not one of Alternative, Data, Source, Supplement, Unspecified"
                )))
            }
        })
    }

    fn krilla(self) -> AssociationKind {
        match self {
            Self::Source => AssociationKind::Source,
            Self::Data => AssociationKind::Data,
            Self::Alternative => AssociationKind::Alternative,
            Self::Supplement => AssociationKind::Supplement,
            Self::Unspecified => AssociationKind::Unspecified,
        }
    }

    fn name(self) -> &'static [u8] {
        match self {
            Self::Source => b"Source",
            Self::Data => b"Data",
            Self::Alternative => b"Alternative",
            Self::Supplement => b"Supplement",
            Self::Unspecified => b"Unspecified",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    pub name: String,
    pub data: Arc<Vec<u8>>,
    pub mime: Option<String>,
    pub description: Option<String>,
    pub relationship: Relationship,
    pub created: Option<Instant>,
    pub modified: Option<Instant>,
    /// Not listed among the document's files: only an annotation shows it.
    pub hidden: bool,
}

/// What a page annotation is.
#[derive(Debug, Clone, PartialEq)]
pub enum AnnotationKind {
    /// A sticky note; `icon` is its `/Name` (Comment, Note, Help…).
    Note {
        icon: String,
    },
    Highlight,
    Underline,
    StrikeOut,
    /// From one point to another, in page space.
    Line {
        from: (f32, f32),
        to: (f32, f32),
    },
    Square,
    Circle,
    FreeText,
    /// A file, by the handle `attach` answered.
    File {
        attachment: usize,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct PageAnnotation {
    pub kind: AnnotationKind,
    /// `[left, top, right, bottom]`, page space, from the top-left corner.
    pub rect: [f32; 4],
    pub contents: Option<String>,
    /// 0 to 255 each.
    pub colour: Option<[u8; 3]>,
}

/// A destination's view other than XYZ: what pdfkit's `addNamedDestination`
/// takes after the name.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Fit {
    Fit,
    FitB,
    FitH(Option<f32>),
    FitBH(Option<f32>),
    FitV(Option<f32>),
    FitBV(Option<f32>),
    FitR([f32; 4]),
}

impl Composer {
    /// Embed a file and answer its handle. The same name and bytes answer
    /// the same handle, as pdfkit embeds them once.
    pub fn attach(&mut self, attachment: Attachment) -> Result<usize, ComposeError> {
        if attachment.data.len() > self.settings.max_file_bytes {
            return Err(invalid(format!(
                "a file of {} bytes is over the limit of {}",
                attachment.data.len(),
                self.settings.max_file_bytes
            )));
        }
        if attachment.name.is_empty() || attachment.name.len() > 1024 {
            return Err(invalid("a file's name is 1 to 1024 bytes"));
        }
        if let Some(mime) = &attachment.mime {
            MimeType::new(mime).ok_or_else(|| invalid(format!("{mime:?} is not a MIME type")))?;
        }
        if let Some(index) = self
            .attachments
            .iter()
            .position(|known| *known == attachment)
        {
            return Ok(index);
        }
        if self.attachments.len() >= MAX_EXTRAS {
            return Err(invalid(format!(
                "a document takes at most {MAX_EXTRAS} files"
            )));
        }
        self.attachments.push(attachment);
        Ok(self.attachments.len() - 1)
    }

    /// An annotation over `x`, `y`, `width`, `height` of the current page,
    /// in user space, as pdfkit's annotation methods take it.
    #[allow(clippy::too_many_arguments)]
    pub fn annotate(
        &mut self,
        kind: AnnotationKind,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        contents: Option<String>,
        colour: Option<[u8; 3]>,
    ) -> Result<(), ComposeError> {
        if ![x, y, width, height].iter().all(|value| value.is_finite()) {
            return Err(invalid("an annotation's rectangle must be finite numbers"));
        }
        let ctm = self.state.ctm;
        let [a, b, c, d, e, f] = ctm;
        let (kind, (left, top, right, bottom)) = match kind {
            // A line's rectangle is the box of its two ends; pdfkit takes
            // the second end for a width and a height.
            AnnotationKind::Line { from, to } => {
                let map = |(px, py): (f32, f32)| (a * px + c * py + e, b * px + d * py + f);
                let (from, to) = (map(from), map(to));
                let rect = (
                    from.0.min(to.0),
                    from.1.min(to.1),
                    from.0.max(to.0),
                    from.1.max(to.1),
                );
                (AnnotationKind::Line { from, to }, rect)
            }
            other => (other, super::links::bounding_box(ctm, x, y, width, height)),
        };
        let annotation = PageAnnotation {
            kind,
            rect: [left, top, right, bottom],
            contents,
            colour,
        };
        if let AnnotationKind::File { attachment } = annotation.kind {
            if attachment >= self.attachments.len() {
                return Err(invalid("no file has this handle"));
            }
        }
        if self.extra_annotations.len() >= MAX_EXTRAS {
            return Err(invalid(format!(
                "a document takes at most {MAX_EXTRAS} annotations"
            )));
        }
        let page = self.current_page_index().ok_or_else(|| {
            ComposeError::new("NO_PAGE", "there is no current page: call addPage() first")
        })?;
        self.extra_annotations.push((page, annotation));
        Ok(())
    }

    /// A destination named before opens a view other than XYZ.
    pub fn set_destination_fit(&mut self, name: &str, fit: Fit) -> Result<(), ComposeError> {
        if !self.destinations.iter().any(|point| point.name == name) {
            return Err(invalid(format!("no destination is named {name:?}")));
        }
        self.fits.retain(|(known, _)| known != name);
        self.fits.push((name.to_string(), fit));
        Ok(())
    }

    /// What PDF/A and PDF/UA cannot take: annotations without an
    /// appearance.
    pub(crate) fn check_extras(&self) -> Result<(), ComposeError> {
        if self.settings.subset.is_some() && !self.extra_annotations.is_empty() {
            return Err(ComposeError::new(
                "VALIDATION",
                "PDF/A and PDF/UA require every annotation but links to carry an appearance; notes, markup, shapes and file annotations have none",
            ));
        }
        Ok(())
    }

    /// The files krilla embeds: those not hidden.
    pub(crate) fn embedded_files(&self) -> Vec<EmbeddedFile> {
        self.attachments
            .iter()
            .filter(|file| !file.hidden)
            .map(|file| EmbeddedFile {
                path: file.name.clone(),
                mime_type: file.mime.as_deref().and_then(MimeType::new),
                description: file.description.clone(),
                association_kind: file.relationship.krilla(),
                data: file.data.clone().into(),
                modification_date: file.modified.or(file.created).and_then(Instant::krilla),
                compress: None,
                location: None,
            })
            .collect()
    }

    pub(crate) fn needs_annotation_pass(&self) -> bool {
        !self.extra_annotations.is_empty() || !self.fits.is_empty()
    }

    /// The annotations and destination views krilla does not write.
    pub(crate) fn write_extras(&self, document: &mut Document) -> Result<(), ComposeError> {
        let failed = |error: lopdf::Error| ComposeError::new("WRITE_FAILED", error.to_string());
        let pages = document.get_pages();
        for (page, annotation) in &self.extra_annotations {
            let Some(&page_id) = pages.get(&(*page as u32 + 1)) else {
                continue;
            };
            let height = self.pages.get(*page).map_or(0.0, |page| page.setup.height);
            let dictionary = self.annotation_dictionary(document, annotation, height)?;
            let id = document.add_object(dictionary);
            let page = document
                .get_object_mut(page_id)
                .and_then(|object| object.as_dict_mut())
                .map_err(failed)?;
            match page.get_mut(b"Annots") {
                Ok(Object::Array(annotations)) => annotations.push(Object::Reference(id)),
                Ok(Object::Reference(array)) => {
                    let array = *array;
                    if let Ok(Object::Array(annotations)) = document.get_object_mut(array) {
                        annotations.push(Object::Reference(id));
                    }
                }
                _ => page.set("Annots", Object::Array(vec![Object::Reference(id)])),
            }
        }
        if !self.fits.is_empty() {
            self.write_fits(document, &pages)?;
        }
        Ok(())
    }

    fn annotation_dictionary(
        &self,
        document: &mut Document,
        annotation: &PageAnnotation,
        height: f32,
    ) -> Result<Dictionary, ComposeError> {
        let [left, top, right, bottom] = annotation.rect;
        let rect = vec![
            Object::Real(left),
            Object::Real(height - bottom),
            Object::Real(right),
            Object::Real(height - top),
        ];
        let mut dictionary = Dictionary::new();
        dictionary.set("Type", Object::Name(b"Annot".to_vec()));
        dictionary.set("Rect", Object::Array(rect.clone()));
        // Printed, as pdfkit's are.
        dictionary.set("F", Object::Integer(4));
        if let Some(contents) = &annotation.contents {
            dictionary.set("Contents", text_string(contents));
        }
        if let Some([red, green, blue]) = annotation.colour {
            dictionary.set(
                "C",
                Object::Array(
                    [red, green, blue]
                        .iter()
                        .map(|component| Object::Real(f32::from(*component) / 255.0))
                        .collect(),
                ),
            );
        }
        let quad = || {
            Object::Array(
                [
                    left,
                    height - top,
                    right,
                    height - top,
                    left,
                    height - bottom,
                    right,
                    height - bottom,
                ]
                .iter()
                .map(|value| Object::Real(*value))
                .collect(),
            )
        };
        let subtype: &[u8] = match &annotation.kind {
            AnnotationKind::Note { icon } => {
                dictionary.set("Name", Object::Name(icon.as_bytes().to_vec()));
                b"Text"
            }
            AnnotationKind::Highlight => {
                dictionary.set("QuadPoints", quad());
                b"Highlight"
            }
            AnnotationKind::Underline => {
                dictionary.set("QuadPoints", quad());
                b"Underline"
            }
            AnnotationKind::StrikeOut => {
                dictionary.set("QuadPoints", quad());
                b"StrikeOut"
            }
            AnnotationKind::Line { from, to } => {
                dictionary.set(
                    "L",
                    Object::Array(vec![
                        Object::Real(from.0),
                        Object::Real(height - from.1),
                        Object::Real(to.0),
                        Object::Real(height - to.1),
                    ]),
                );
                b"Line"
            }
            AnnotationKind::Square => b"Square",
            AnnotationKind::Circle => b"Circle",
            AnnotationKind::FreeText => {
                // pdfkit leaves the default appearance empty.
                dictionary.set("DA", Object::string_literal(""));
                b"FreeText"
            }
            AnnotationKind::File { attachment } => {
                let file = self
                    .attachments
                    .get(*attachment)
                    .ok_or_else(|| invalid("no file has this handle"))?;
                let filespec = filespec(document, file);
                dictionary.set("FS", Object::Reference(filespec));
                dictionary.set("Name", Object::Name(b"PushPin".to_vec()));
                b"FileAttachment"
            }
        };
        dictionary.set("Subtype", Object::Name(subtype.to_vec()));
        Ok(dictionary)
    }

    fn write_fits(
        &self,
        document: &mut Document,
        pages: &std::collections::BTreeMap<u32, ObjectId>,
    ) -> Result<(), ComposeError> {
        let failed = |error: lopdf::Error| ComposeError::new("WRITE_FAILED", error.to_string());
        for (name, fit) in &self.fits {
            let Some(point) = self.destinations.iter().find(|point| &point.name == name) else {
                continue;
            };
            let Some(&page_id) = pages.get(&(point.page as u32 + 1)) else {
                continue;
            };
            let height = self
                .pages
                .get(point.page)
                .map_or(0.0, |page| page.setup.height);
            let flip = |value: Option<f32>| {
                value.map_or(Object::Null, |value| Object::Real(height - value))
            };
            let x = |value: Option<f32>| value.map_or(Object::Null, Object::Real);
            let mut array = vec![Object::Reference(page_id)];
            match *fit {
                Fit::Fit => array.push(Object::Name(b"Fit".to_vec())),
                Fit::FitB => array.push(Object::Name(b"FitB".to_vec())),
                Fit::FitH(top) => array.extend([Object::Name(b"FitH".to_vec()), flip(top)]),
                Fit::FitBH(top) => array.extend([Object::Name(b"FitBH".to_vec()), flip(top)]),
                Fit::FitV(left) => array.extend([Object::Name(b"FitV".to_vec()), x(left)]),
                Fit::FitBV(left) => array.extend([Object::Name(b"FitBV".to_vec()), x(left)]),
                Fit::FitR([left, top, right, bottom]) => array.extend([
                    Object::Name(b"FitR".to_vec()),
                    Object::Real(left),
                    Object::Real(height - bottom),
                    Object::Real(right),
                    Object::Real(height - top),
                ]),
            }
            replace_named_destination(document, name, Object::Array(array)).map_err(failed)?;
        }
        Ok(())
    }
}

/// A filespec with its embedded file, as a file annotation points to.
fn filespec(document: &mut Document, file: &Attachment) -> ObjectId {
    let mut parameters = Dictionary::new();
    parameters.set("Size", Object::Integer(file.data.len() as i64));
    if let Some(date) = file.created.and_then(Instant::pdf_string) {
        parameters.set("CreationDate", Object::string_literal(date));
    }
    if let Some(date) = file.modified.and_then(Instant::pdf_string) {
        parameters.set("ModDate", Object::string_literal(date));
    }
    let mut header = Dictionary::new();
    header.set("Type", Object::Name(b"EmbeddedFile".to_vec()));
    if let Some(mime) = &file.mime {
        // A MIME type is a name with `/` written `#2F`; lopdf escapes it.
        header.set("Subtype", Object::Name(mime.as_bytes().to_vec()));
    }
    header.set("Params", Object::Dictionary(parameters));
    let mut stream = Stream::new(header, file.data.to_vec());
    // Left as it is when deflate fails: still a valid stream.
    stream.compress().unwrap_or(());
    let data = document.add_object(stream);
    let mut embedded = Dictionary::new();
    embedded.set("F", Object::Reference(data));
    embedded.set("UF", Object::Reference(data));
    let mut spec = Dictionary::new();
    spec.set("Type", Object::Name(b"Filespec".to_vec()));
    spec.set("F", text_string(&file.name));
    spec.set("UF", text_string(&file.name));
    spec.set("EF", Object::Dictionary(embedded));
    spec.set(
        "AFRelationship",
        Object::Name(file.relationship.name().to_vec()),
    );
    if let Some(description) = &file.description {
        spec.set("Desc", text_string(description));
    }
    document.add_object(spec)
}

/// Replace a destination in the document's `/Dests` name tree.
fn replace_named_destination(
    document: &mut Document,
    name: &str,
    destination: Object,
) -> lopdf::Result<()> {
    // krilla writes the name dictionary, and its Dests tree, inline: each
    // is moved into an object of its own, so the tree can be walked and
    // changed by reference.
    let names = match document.catalog()?.get(b"Names")?.clone() {
        Object::Reference(id) => id,
        Object::Dictionary(inline) => {
            let id = document.add_object(inline);
            document.catalog_mut()?.set("Names", Object::Reference(id));
            id
        }
        _ => return Ok(()),
    };
    let dests = match document.get_dictionary(names)?.get(b"Dests")?.clone() {
        Object::Reference(id) => id,
        Object::Dictionary(inline) => {
            let id = document.add_object(inline);
            if let Ok(Object::Dictionary(names)) = document.get_object_mut(names) {
                names.set("Dests", Object::Reference(id));
            }
            id
        }
        _ => return Ok(()),
    };
    let mut stack = vec![dests];
    while let Some(node) = stack.pop() {
        let dictionary = document.get_dictionary(node)?.clone();
        if let Ok(Object::Array(kids)) = dictionary.get(b"Kids") {
            stack.extend(kids.iter().filter_map(|kid| kid.as_reference().ok()));
        }
        let Ok(Object::Array(entries)) = dictionary.get(b"Names") else {
            continue;
        };
        let mut entries = entries.clone();
        let mut changed = false;
        for pair in entries.chunks_mut(2) {
            let matches = matches!(&pair[0], Object::String(bytes, _) if bytes == name.as_bytes());
            if matches && pair.len() == 2 {
                pair[1] = destination.clone();
                changed = true;
            }
        }
        if changed {
            if let Ok(Object::Dictionary(target)) = document.get_object_mut(node) {
                target.set("Names", Object::Array(entries));
            }
        }
    }
    Ok(())
}
