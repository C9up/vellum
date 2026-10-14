//! Interactive form fields and encryption, which krilla does not write.
//!
//! Both are added by the finishing pass. pdfkit writes fields without an
//! appearance and sets `NeedAppearances`, leaving each reader to draw them
//! (and many draw nothing); here every widget carries its appearance, and a
//! field with a value is drawn by the same code that fills existing forms.
//!
//! Encryption is AES-256 (revision 6, PDF 2.0) only. pdfkit picks RC4 for
//! the PDF versions it writes by default, a cipher long broken.

use std::collections::BTreeMap;
use std::sync::Arc;

use lopdf::encryption::crypt_filters::{Aes256CryptFilter, CryptFilter};
use lopdf::{
    Dictionary, Document, EncryptionState, EncryptionVersion, Object, ObjectId, Permissions, Stream,
};

use super::{text_string, ComposeError, Composer};
use crate::metrics::width_of;
use crate::stamp_text::{escape_pdf_literal, to_win_ansi, StandardFont};

fn invalid(message: impl Into<String>) -> ComposeError {
    ComposeError::new("INVALID_OPTIONS", message)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    Text,
    PushButton,
    RadioButton,
    Checkbox,
    Combo,
    List,
}

/// pdfkit's `format`: Acrobat's own formatting functions, with parameters
/// checked rather than interpolated into the script as pdfkit does.
#[derive(Debug, Clone, PartialEq)]
pub enum Format {
    Zip,
    ZipPlus4,
    Phone,
    Ssn,
    Date(String),
    Time(u8),
    Number {
        decimals: u8,
        separate_thousands: bool,
        negative: String,
        currency: String,
        currency_first: bool,
    },
    Percent {
        decimals: u8,
        separate_thousands: bool,
    },
}

impl Format {
    pub fn check(&self) -> Result<(), ComposeError> {
        match self {
            Self::Date(format) => {
                if format.is_empty()
                    || format.len() > 32
                    || !format
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || " /.-:".contains(c))
                {
                    return Err(invalid(format!("{format:?} is not a date format")));
                }
            }
            Self::Time(style) if *style > 3 => return Err(invalid("a time format is 0 to 3")),
            Self::Number {
                decimals,
                negative,
                currency,
                ..
            } => {
                if *decimals > 10 {
                    return Err(invalid("a number format has at most 10 decimals"));
                }
                if !["MinusBlack", "Red", "ParensBlack", "ParensRed"].contains(&negative.as_str()) {
                    return Err(invalid(format!("{negative:?} is not a negative style")));
                }
                if currency.len() > 8
                    || currency
                        .chars()
                        .any(|c| c == '"' || c == '\\' || c.is_control())
                {
                    return Err(invalid(
                        "a currency is up to 8 characters, without quotes or backslashes",
                    ));
                }
            }
            Self::Percent { decimals, .. } if *decimals > 10 => {
                return Err(invalid("a percent format has at most 10 decimals"))
            }
            _ => {}
        }
        Ok(())
    }

    /// The keystroke and format scripts, as Acrobat names them.
    fn scripts(&self) -> (String, String) {
        let (name, parameters) = match self {
            Self::Zip => ("Special", "0".to_string()),
            Self::ZipPlus4 => ("Special", "1".to_string()),
            Self::Phone => ("Special", "2".to_string()),
            Self::Ssn => ("Special", "3".to_string()),
            Self::Date(format) => ("DateEx", format!("\"{format}\"")),
            Self::Time(style) => ("Time", style.to_string()),
            Self::Number {
                decimals,
                separate_thousands,
                negative,
                currency,
                currency_first,
            } => (
                "Number",
                format!(
                    "{decimals},{},\"{negative}\",null,\"{currency}\",{currency_first}",
                    if *separate_thousands { 0 } else { 1 }
                ),
            ),
            Self::Percent {
                decimals,
                separate_thousands,
            } => (
                "Percent",
                format!("{decimals},{}", if *separate_thousands { 0 } else { 1 }),
            ),
        };
        let (keystroke, format) = match name {
            "DateEx" => (
                "AFDate_KeystrokeEx".to_string(),
                "AFDate_FormatEx".to_string(),
            ),
            other => (format!("AF{other}_Keystroke"), format!("AF{other}_Format")),
        };
        (
            format!("{keystroke}({parameters});"),
            format!("{format}({parameters});"),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct FieldSpec {
    /// The partial name; the full name is its parents' joined with dots.
    pub name: String,
    /// `None` for a field that only groups others (pdfkit's `formField`).
    pub kind: Option<FieldType>,
    pub parent: Option<usize>,
    /// pdfkit's flags, already combined.
    pub flags: u32,
    /// 0 left, 1 centre, 2 right.
    pub align: u8,
    pub value: Option<String>,
    pub default_value: Option<String>,
    /// A choice field's options.
    pub choices: Vec<String>,
    /// A push button's caption.
    pub label: Option<String>,
    pub background: Option<[u8; 3]>,
    pub border: Option<[u8; 3]>,
    /// 0 sizes the text to its box.
    pub font_size: f32,
    pub format: Option<Format>,
    /// The state a checkbox or radio button is on in; default `Yes`.
    pub on_state: Option<String>,
    /// The page and the widget's rectangle in page space, for a field shown
    /// on a page.
    pub widget: Option<(usize, [f32; 4])>,
}

/// Fields in one document.
pub const MAX_FIELDS: usize = 10_000;

/// pdfkit's `permissions`: what a reader with the user password may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rights {
    /// `None` no printing, `Some(false)` low resolution, `Some(true)` high.
    pub printing: Option<bool>,
    pub modifying: bool,
    pub copying: bool,
    pub annotating: bool,
    pub filling_forms: bool,
    pub content_accessibility: bool,
    pub document_assembly: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Encryption {
    pub user_password: String,
    pub owner_password: String,
    pub rights: Rights,
}

impl Composer {
    /// A field; `kind` absent groups the fields that name it as their
    /// parent. Answers the field's handle.
    pub fn form_field(
        &mut self,
        mut spec: FieldSpec,
        rect: Option<(f32, f32, f32, f32)>,
    ) -> Result<usize, ComposeError> {
        if spec.name.is_empty() || spec.name.len() > 1024 || spec.name.contains('.') {
            return Err(invalid("a field's name is 1 to 1024 bytes, without a dot"));
        }
        if let Some(parent) = spec.parent {
            if parent >= self.fields.len() || self.fields[parent].kind.is_some() {
                return Err(invalid("a field's parent is a field made with formField"));
            }
        }
        if let Some(format) = &spec.format {
            format.check()?;
        }
        if !spec.font_size.is_finite() || !(0.0..=1000.0).contains(&spec.font_size) {
            return Err(invalid("a field's font size is 0 (automatic) to 1000"));
        }
        if self.fields.len() >= MAX_FIELDS {
            return Err(invalid(format!(
                "a document takes at most {MAX_FIELDS} fields"
            )));
        }
        if let Some((x, y, width, height)) = rect {
            if ![x, y, width, height].iter().all(|value| value.is_finite()) {
                return Err(invalid("a field's rectangle must be finite numbers"));
            }
            let page = self.current_page_index().ok_or_else(|| {
                ComposeError::new("NO_PAGE", "there is no current page: call addPage() first")
            })?;
            let (left, top, right, bottom) =
                super::links::bounding_box(self.state.ctm, x, y, width, height);
            spec.widget = Some((page, [left, top, right, bottom]));
        }
        self.fields.push(spec);
        Ok(self.fields.len() - 1)
    }

    pub fn set_encryption(&mut self, encryption: Encryption) -> Result<(), ComposeError> {
        if encryption.user_password.len() > 127 || encryption.owner_password.len() > 127 {
            return Err(invalid("a password is at most 127 bytes"));
        }
        self.encryption = Some(encryption);
        Ok(())
    }

    /// What PDF/A and PDF/UA forbid or cannot check here.
    pub(crate) fn check_forms(&self) -> Result<(), ComposeError> {
        if self.settings.subset.is_some() && (!self.fields.is_empty() || self.encryption.is_some())
        {
            return Err(ComposeError::new(
                "VALIDATION",
                "PDF/A and PDF/UA documents carry neither encryption nor, written here, form fields",
            ));
        }
        Ok(())
    }

    pub(crate) fn needs_form_pass(&self) -> bool {
        !self.fields.is_empty() || self.encryption.is_some()
    }

    fn full_name(&self, index: usize) -> String {
        let mut names = vec![self.fields[index].name.clone()];
        let mut current = self.fields[index].parent;
        while let Some(parent) = current {
            names.push(self.fields[parent].name.clone());
            current = self.fields[parent].parent;
        }
        names.reverse();
        names.join(".")
    }

    /// Write the fields into a document, then the values through `fill_form`.
    pub(crate) fn write_fields(
        &self,
        document: &mut Document,
    ) -> Result<Vec<crate::FieldValue>, ComposeError> {
        if self.fields.is_empty() {
            return Ok(Vec::new());
        }
        let failed = |error: lopdf::Error| ComposeError::new("WRITE_FAILED", error.to_string());
        let pages = document.get_pages();
        let helvetica = document.add_object(standard_font(b"Helvetica"));
        let ids: Vec<ObjectId> = self
            .fields
            .iter()
            .map(|_| document.new_object_id())
            .collect();
        let mut top_level = Vec::new();
        let mut kids: Vec<Vec<ObjectId>> = vec![Vec::new(); self.fields.len()];
        for (index, field) in self.fields.iter().enumerate() {
            match field.parent {
                Some(parent) => kids[parent].push(ids[index]),
                None => top_level.push(Object::Reference(ids[index])),
            }
        }
        let mut values = Vec::new();
        for (index, field) in self.fields.iter().enumerate() {
            let mut dictionary = Dictionary::new();
            dictionary.set("T", text_string(&field.name));
            if let Some(parent) = field.parent {
                dictionary.set("Parent", Object::Reference(ids[parent]));
            }
            if !kids[index].is_empty() {
                dictionary.set(
                    "Kids",
                    Object::Array(
                        kids[index]
                            .iter()
                            .map(|id| Object::Reference(*id))
                            .collect(),
                    ),
                );
            }
            if let Some(kind) = field.kind {
                self.field_entries(document, &mut dictionary, field, kind, helvetica, &pages)?;
                if let (Some(value), FieldType::Text | FieldType::Combo | FieldType::List) =
                    (&field.value, kind)
                {
                    values.push(crate::FieldValue {
                        name: self.full_name(index),
                        value: value.clone(),
                    });
                }
            }
            document
                .objects
                .insert(ids[index], Object::Dictionary(dictionary));
            if let Some((page, _)) = field.widget {
                if let Some(&page_id) = pages.get(&(page as u32 + 1)) {
                    let page = document
                        .get_object_mut(page_id)
                        .and_then(|object| object.as_dict_mut())
                        .map_err(failed)?;
                    match page.get_mut(b"Annots") {
                        Ok(Object::Array(annotations)) => {
                            annotations.push(Object::Reference(ids[index]))
                        }
                        _ => page.set("Annots", Object::Array(vec![Object::Reference(ids[index])])),
                    }
                }
            }
        }
        let mut fonts = Dictionary::new();
        fonts.set("Helv", Object::Reference(helvetica));
        let mut resources = Dictionary::new();
        resources.set("Font", Object::Dictionary(fonts));
        let mut form = Dictionary::new();
        form.set("Fields", Object::Array(top_level));
        form.set("DA", Object::string_literal("/Helv 0 Tf 0 g"));
        form.set("DR", Object::Dictionary(resources));
        let form = document.add_object(form);
        document
            .catalog_mut()
            .map_err(failed)?
            .set("AcroForm", Object::Reference(form));
        Ok(values)
    }

    fn field_entries(
        &self,
        document: &mut Document,
        dictionary: &mut Dictionary,
        field: &FieldSpec,
        kind: FieldType,
        helvetica: ObjectId,
        pages: &BTreeMap<u32, ObjectId>,
    ) -> Result<(), ComposeError> {
        let (field_type, extra): (&[u8], u32) = match kind {
            FieldType::Text => (b"Tx", 0),
            FieldType::PushButton => (b"Btn", 0x10000),
            FieldType::RadioButton => (b"Btn", 0x8000),
            FieldType::Checkbox => (b"Btn", 0),
            FieldType::Combo => (b"Ch", 0x20000),
            FieldType::List => (b"Ch", 0),
        };
        dictionary.set("FT", Object::Name(field_type.to_vec()));
        let flags = field.flags | extra;
        if flags != 0 {
            dictionary.set("Ff", Object::Integer(i64::from(flags)));
        }
        if field.align != 0 {
            dictionary.set("Q", Object::Integer(i64::from(field.align)));
        }
        dictionary.set(
            "DA",
            Object::string_literal(format!("/Helv {} Tf 0 g", field.font_size)),
        );
        if !field.choices.is_empty() {
            dictionary.set(
                "Opt",
                Object::Array(
                    field
                        .choices
                        .iter()
                        .map(|choice| text_string(choice))
                        .collect(),
                ),
            );
        }
        let on = field.on_state.clone().unwrap_or_else(|| "Yes".into());
        let checked = matches!(kind, FieldType::Checkbox | FieldType::RadioButton)
            && field.value.as_deref().is_some_and(|value| value == on);
        match kind {
            FieldType::Checkbox | FieldType::RadioButton => {
                let state = if checked {
                    on.as_bytes().to_vec()
                } else {
                    b"Off".to_vec()
                };
                dictionary.set("V", Object::Name(state.clone()));
                dictionary.set("AS", Object::Name(state));
            }
            _ => {
                if let Some(default) = &field.default_value {
                    dictionary.set("DV", text_string(default));
                }
            }
        }
        let mut characteristics = Dictionary::new();
        if let Some(label) = &field.label {
            characteristics.set("CA", text_string(label));
        }
        let colour = |[red, green, blue]: [u8; 3]| {
            Object::Array(
                [red, green, blue]
                    .iter()
                    .map(|component| Object::Real(f32::from(*component) / 255.0))
                    .collect(),
            )
        };
        if let Some(background) = field.background {
            characteristics.set("BG", colour(background));
        }
        if let Some(border) = field.border {
            characteristics.set("BC", colour(border));
        }
        if !characteristics.is_empty() {
            dictionary.set("MK", Object::Dictionary(characteristics));
        }
        if let Some(format) = &field.format {
            let (keystroke, format) = format.scripts();
            let script = |source: String| {
                let mut action = Dictionary::new();
                action.set("S", Object::Name(b"JavaScript".to_vec()));
                action.set("JS", Object::string_literal(source));
                Object::Dictionary(action)
            };
            let mut actions = Dictionary::new();
            actions.set("K", script(keystroke));
            actions.set("F", script(format));
            dictionary.set("AA", Object::Dictionary(actions));
        }
        let Some((page, [left, top, right, bottom])) = field.widget else {
            return Ok(());
        };
        let height = self.pages.get(page).map_or(0.0, |page| page.setup.height);
        dictionary.set("Type", Object::Name(b"Annot".to_vec()));
        dictionary.set("Subtype", Object::Name(b"Widget".to_vec()));
        dictionary.set("F", Object::Integer(4));
        dictionary.set(
            "Rect",
            Object::Array(vec![
                Object::Real(left),
                Object::Real(height - bottom),
                Object::Real(right),
                Object::Real(height - top),
            ]),
        );
        if let Some(&page_id) = pages.get(&(page as u32 + 1)) {
            dictionary.set("P", Object::Reference(page_id));
        }
        let (width, box_height) = (right - left, bottom - top);
        let frame = frame(field.background, field.border, width, box_height);
        let appearance = match kind {
            FieldType::Checkbox | FieldType::RadioButton => {
                let mark = if kind == FieldType::Checkbox {
                    check_mark(width, box_height)
                } else {
                    dot(width, box_height)
                };
                let mut states = Dictionary::new();
                states.set(
                    on.as_bytes().to_vec(),
                    Object::Reference(document.add_object(appearance_stream(
                        &[frame.clone(), mark].concat(),
                        width,
                        box_height,
                        None,
                    ))),
                );
                states.set(
                    "Off",
                    Object::Reference(
                        document.add_object(appearance_stream(&frame, width, box_height, None)),
                    ),
                );
                Object::Dictionary(states)
            }
            FieldType::PushButton => {
                let caption = field.label.as_deref().unwrap_or("");
                let content = [frame, caption_content(caption, width, box_height)?].concat();
                Object::Reference(document.add_object(appearance_stream(
                    &content,
                    width,
                    box_height,
                    Some(helvetica),
                )))
            }
            // Text and choices: their frame now, their value drawn by
            // `fill_form` once the document holds the field.
            FieldType::Text | FieldType::Combo | FieldType::List => {
                Object::Reference(document.add_object(appearance_stream(
                    &frame,
                    width,
                    box_height,
                    Some(helvetica),
                )))
            }
        };
        let mut appearances = Dictionary::new();
        appearances.set("N", appearance);
        dictionary.set("AP", Object::Dictionary(appearances));
        Ok(())
    }

    /// Encrypt the written document, AES-256 revision 6.
    pub(crate) fn encrypt(&self, document: &mut Document) -> Result<(), ComposeError> {
        let Some(encryption) = &self.encryption else {
            return Ok(());
        };
        let failed = |error: lopdf::Error| ComposeError::new("WRITE_FAILED", error.to_string());
        let mut key = [0u8; 32];
        getrandom::getrandom(&mut key).map_err(|error| {
            ComposeError::new("WRITE_FAILED", format!("cannot draw a key: {error}"))
        })?;
        let rights = encryption.rights;
        let mut permissions = Permissions::empty();
        let mut grant = |allowed: bool, permission: Permissions| {
            if allowed {
                permissions |= permission;
            }
        };
        grant(rights.printing.is_some(), Permissions::PRINTABLE);
        grant(
            rights.printing == Some(true),
            Permissions::PRINTABLE_IN_HIGH_QUALITY,
        );
        grant(rights.modifying, Permissions::MODIFIABLE);
        grant(rights.copying, Permissions::COPYABLE);
        grant(rights.annotating, Permissions::ANNOTABLE);
        grant(rights.filling_forms, Permissions::FILLABLE);
        grant(
            rights.content_accessibility,
            Permissions::COPYABLE_FOR_ACCESSIBILITY,
        );
        grant(rights.document_assembly, Permissions::ASSEMBLABLE);
        let filter: Arc<dyn CryptFilter> = Arc::new(Aes256CryptFilter);
        let owner = if encryption.owner_password.is_empty() {
            &encryption.user_password
        } else {
            &encryption.owner_password
        };
        let version = EncryptionVersion::V5 {
            encrypt_metadata: true,
            crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), filter)]),
            file_encryption_key: &key,
            stream_filter: b"StdCF".to_vec(),
            string_filter: b"StdCF".to_vec(),
            owner_password: owner,
            user_password: &encryption.user_password,
            permissions,
        };
        let state = EncryptionState::try_from(version).map_err(failed)?;
        document.encrypt(&state).map_err(failed)?;
        // Revision 6 is PDF 2.0's.
        document.version = "2.0".to_string();
        Ok(())
    }
}

fn standard_font(name: &[u8]) -> Dictionary {
    let mut font = Dictionary::new();
    font.set("Type", Object::Name(b"Font".to_vec()));
    font.set("Subtype", Object::Name(b"Type1".to_vec()));
    font.set("BaseFont", Object::Name(name.to_vec()));
    font.set("Encoding", Object::Name(b"WinAnsiEncoding".to_vec()));
    font
}

/// The widget's background and border, as its `MK` asks.
fn frame(background: Option<[u8; 3]>, border: Option<[u8; 3]>, width: f32, height: f32) -> Vec<u8> {
    let mut content = Vec::new();
    let rgb = |[red, green, blue]: [u8; 3]| {
        format!(
            "{} {} {}",
            f32::from(red) / 255.0,
            f32::from(green) / 255.0,
            f32::from(blue) / 255.0
        )
    };
    if let Some(colour) = background {
        content.extend(format!("{} rg 0 0 {width} {height} re f\n", rgb(colour)).bytes());
    }
    if let Some(colour) = border {
        content.extend(
            format!(
                "{} RG 1 w 0.5 0.5 {} {} re S\n",
                rgb(colour),
                width - 1.0,
                height - 1.0
            )
            .bytes(),
        );
    }
    content
}

fn check_mark(width: f32, height: f32) -> Vec<u8> {
    let size = width.min(height);
    format!(
        "q 0 g {w} w 1 J 1 j {} {} m {} {} l {} {} l S Q\n",
        width * 0.2,
        height * 0.5,
        width * 0.42,
        height * 0.25,
        width * 0.8,
        height * 0.78,
        w = (size * 0.12).max(0.5)
    )
    .into_bytes()
}

fn dot(width: f32, height: f32) -> Vec<u8> {
    let radius = width.min(height) * 0.25;
    let (cx, cy) = (width / 2.0, height / 2.0);
    let k = radius * 0.552_284_8;
    format!(
        "q 0 g {} {cy} m {} {} {} {} {cx} {} c {} {} {} {} {} {cy} c {} {} {} {} {cx} {} c {} {} {} {} {} {cy} c f Q\n",
        cx + radius,
        cx + radius, cy + k, cx + k, cy + radius, cy + radius,
        cx - k, cy + radius, cx - radius, cy + k, cx - radius,
        cx - radius, cy - k, cx - k, cy - radius, cy - radius,
        cx + k, cy - radius, cx + radius, cy - k, cx + radius,
    )
    .into_bytes()
}

/// A push button's caption, centred, in Helvetica.
fn caption_content(caption: &str, width: f32, height: f32) -> Result<Vec<u8>, ComposeError> {
    if caption.is_empty() {
        return Ok(Vec::new());
    }
    let bytes =
        to_win_ansi(caption).map_err(|error| invalid(format!("a button's label: {error}")))?;
    let size = (height * 0.6).clamp(1.0, 24.0);
    let text_width = width_of(StandardFont::Helvetica, &bytes, size);
    let x = ((width - text_width) / 2.0).max(1.0);
    let y = (height - size * 0.7) / 2.0;
    let mut content = format!("BT /Helv {size} Tf 0 g {x} {y} Td (").into_bytes();
    content.extend(escape_pdf_literal(&bytes));
    content.extend(b") Tj ET\n");
    Ok(content)
}

fn appearance_stream(content: &[u8], width: f32, height: f32, font: Option<ObjectId>) -> Stream {
    let mut dictionary = Dictionary::new();
    dictionary.set("Type", Object::Name(b"XObject".to_vec()));
    dictionary.set("Subtype", Object::Name(b"Form".to_vec()));
    dictionary.set(
        "BBox",
        Object::Array(vec![
            Object::Real(0.0),
            Object::Real(0.0),
            Object::Real(width),
            Object::Real(height),
        ]),
    );
    if let Some(font) = font {
        let mut fonts = Dictionary::new();
        fonts.set("Helv", Object::Reference(font));
        let mut resources = Dictionary::new();
        resources.set("Font", Object::Dictionary(fonts));
        dictionary.set("Resources", Object::Dictionary(resources));
    }
    Stream::new(dictionary, content.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::compose::{DocumentSettings, PageSetup, Sides};

    fn document() -> Composer {
        let mut document = Composer::new(DocumentSettings::default()).unwrap();
        document
            .add_page(PageSetup {
                width: 300.0,
                height: 300.0,
                margins: Sides {
                    top: 0.0,
                    right: 0.0,
                    bottom: 0.0,
                    left: 0.0,
                },
                user_unit: 1.0,
            })
            .unwrap();
        document
    }

    fn field(name: &str, kind: FieldType) -> FieldSpec {
        FieldSpec {
            name: name.into(),
            kind: Some(kind),
            ..FieldSpec::default()
        }
    }

    #[test]
    fn writes_fields_that_read_back_and_fill() {
        let mut document = document();
        let client = document
            .form_field(
                FieldSpec {
                    name: "client".into(),
                    ..FieldSpec::default()
                },
                None,
            )
            .unwrap();
        document
            .form_field(
                FieldSpec {
                    parent: Some(client),
                    value: Some("Atelier Dubois".into()),
                    border: Some([0, 0, 0]),
                    ..field("nom", FieldType::Text)
                },
                Some((20.0, 20.0, 200.0, 24.0)),
            )
            .unwrap();
        document
            .form_field(
                FieldSpec {
                    value: Some("Yes".into()),
                    ..field("accord", FieldType::Checkbox)
                },
                Some((20.0, 60.0, 14.0, 14.0)),
            )
            .unwrap();
        document
            .form_field(
                FieldSpec {
                    choices: vec!["CHF".into(), "EUR".into()],
                    value: Some("EUR".into()),
                    ..field("devise", FieldType::Combo)
                },
                Some((20.0, 90.0, 80.0, 20.0)),
            )
            .unwrap();
        document
            .form_field(
                FieldSpec {
                    label: Some("Envoyer".into()),
                    ..field("envoyer", FieldType::PushButton)
                },
                Some((20.0, 130.0, 80.0, 24.0)),
            )
            .unwrap();
        let bytes = document.end().unwrap();
        let fields = crate::form_fields(&bytes).unwrap();
        let names: Vec<&str> = fields.iter().map(|field| field.name.as_str()).collect();
        assert!(names.contains(&"client.nom"), "{names:?}");
        assert!(names.contains(&"accord") && names.contains(&"devise"));
        let name = fields
            .iter()
            .find(|field| field.name == "client.nom")
            .unwrap();
        assert_eq!(name.value.as_deref(), Some("Atelier Dubois"));

        // The value is drawn, not left to the reader.
        let text = crate::extract_text(&bytes, 0).unwrap();
        assert!(text.contains("Atelier Dubois"), "{text}");

        // Over the field's own border, which a filled appearance used to drop.
        let png = crate::render_page(
            &bytes,
            0,
            &crate::RenderOptions {
                format: crate::ImageFormat::Png,
                ..Default::default()
            },
        )
        .unwrap();
        let pixmap = hayro::vello_cpu::Pixmap::from_png(std::io::Cursor::new(png)).unwrap();
        let at = |x: u32, y: u32| pixmap.data()[(y * u32::from(pixmap.width()) + x) as usize];
        let edge = at(20, 32);
        assert!(edge.r < 100, "the border is drawn: {edge:?}");

        // And the form fills again as any other.
        let refilled = crate::fill_form(
            &bytes,
            &[crate::FieldValue {
                name: "client.nom".into(),
                value: "Autre nom".into(),
            }],
        )
        .unwrap();
        assert!(crate::extract_text(&refilled, 0)
            .unwrap()
            .contains("Autre nom"));
    }

    #[test]
    fn refuses_names_and_parents_it_cannot_write() {
        let mut document = document();
        assert!(document
            .form_field(field("a.b", FieldType::Text), None)
            .is_err());
        let text = document
            .form_field(field("texte", FieldType::Text), None)
            .unwrap();
        assert!(document
            .form_field(
                FieldSpec {
                    parent: Some(text),
                    ..field("enfant", FieldType::Text)
                },
                None
            )
            .is_err());
    }

    #[test]
    fn encrypts_with_aes_256_and_the_right_password_opens_it() {
        let mut document = document();
        document
            .set_encryption(Encryption {
                user_password: "lecture".into(),
                owner_password: "proprietaire".into(),
                rights: Rights {
                    printing: Some(true),
                    ..Rights::default()
                },
            })
            .unwrap();
        let bytes = document.end().unwrap();
        let info = crate::inspect(&bytes).unwrap();
        assert!(info.encrypted);
        assert!(
            bytes.windows(5).any(|window| window == b"/AESV"),
            "AES crypt filter"
        );
        let mut parsed = lopdf::Document::load_mem(&bytes).unwrap();
        assert!(parsed.decrypt("mauvais").is_err());
        let mut parsed_again = lopdf::Document::load_mem(&bytes).unwrap();
        assert!(parsed_again.decrypt("lecture").is_ok());
        assert!(parsed.is_encrypted());
    }

    #[test]
    fn pdf_a_carries_neither_fields_nor_encryption() {
        let mut document = Composer::new(DocumentSettings {
            subset: Some(crate::compose::Subset::PdfA2B),
            ..DocumentSettings::default()
        })
        .unwrap();
        document
            .add_page(PageSetup {
                width: 300.0,
                height: 300.0,
                margins: Sides {
                    top: 0.0,
                    right: 0.0,
                    bottom: 0.0,
                    left: 0.0,
                },
                user_unit: 1.0,
            })
            .unwrap();
        document
            .set_encryption(Encryption {
                user_password: "x".into(),
                owner_password: String::new(),
                rights: Rights::default(),
            })
            .unwrap();
        assert_eq!(document.end().unwrap_err().code, "VALIDATION");
    }

    #[test]
    fn a_format_never_carries_script_from_its_parameters() {
        let injected = Format::Date("dd/mm\"); app.alert(1); (\"".into());
        assert!(injected.check().is_err());
        let currency = Format::Number {
            decimals: 2,
            separate_thousands: true,
            negative: "MinusBlack".into(),
            currency: "\"); app.alert(1);//".into(),
            currency_first: false,
        };
        assert!(currency.check().is_err());
        let chf = Format::Number {
            decimals: 2,
            separate_thousands: true,
            negative: "ParensRed".into(),
            currency: "CHF ".into(),
            currency_first: true,
        };
        assert!(chf.check().is_ok());
        assert_eq!(
            chf.scripts().0,
            "AFNumber_Keystroke(2,0,\"ParensRed\",null,\"CHF \",true);"
        );
        assert_eq!(
            Format::Date("dd.mm.yyyy".into()).scripts().1,
            "AFDate_FormatEx(\"dd.mm.yyyy\");"
        );
        assert_eq!(Format::Zip.scripts().0, "AFSpecial_Keystroke(0);");
    }
}
