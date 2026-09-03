//! Reading the interactive form of a document (AcroForm).
//!
//! Written here rather than taken from a crate: `pdfer_forms` is four months
//! old with a few thousand downloads, `acroform` stopped in October 2025, and
//! `pdf_forms` died in 2020. A form filler for official documents cannot rest
//! on that.
//!
//! Two pieces of PDF 32000-1 §12.7.3 shape the code:
//!
//! - A field's type, flags and value are INHERITED down `/Parent`, so a
//!   terminal field often carries none of them itself.
//! - A field's name is the `/T` of every ancestor joined with dots, which is
//!   the name a caller uses to fill it in.

use std::collections::HashSet;

use lopdf::{Dictionary, Document, Object, ObjectId};

/// Field flags, as PDF 32000-1 Table 227 numbers them. Adobe counts bits from
/// 1, so bit 1 is the least significant.
const READ_ONLY: i64 = 1;
const REQUIRED: i64 = 1 << 1;
const MULTILINE: i64 = 1 << 12;
const PASSWORD: i64 = 1 << 13;
const RADIO: i64 = 1 << 15;
const PUSH_BUTTON: i64 = 1 << 16;
const COMBO: i64 = 1 << 17;

/// What kind of control a field is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Checkbox,
    Radio,
    /// A button that triggers an action and holds no value.
    PushButton,
    Dropdown,
    ListBox,
    Signature,
}

impl FieldKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Checkbox => "checkbox",
            Self::Radio => "radio",
            Self::PushButton => "pushButton",
            Self::Dropdown => "dropdown",
            Self::ListBox => "listBox",
            Self::Signature => "signature",
        }
    }
}

/// One interactive field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormField {
    /// The fully qualified name — every ancestor's `/T` joined with dots.
    pub name: String,
    pub kind: FieldKind,
    /// The current value. A checkbox or radio reports its selected state name.
    pub value: Option<String>,
    /// What a choice field offers, or the states a checkbox and radio accept.
    pub options: Vec<String>,
    pub read_only: bool,
    pub required: bool,
    /// Text fields only.
    pub multiline: bool,
    pub password: bool,
    pub max_length: Option<u32>,
}

/// Resolve an object through any number of references.
fn resolve<'a>(document: &'a Document, object: &'a Object) -> Option<&'a Object> {
    match object {
        Object::Reference(id) => document.get_object(*id).ok(),
        other => Some(other),
    }
}

/// Read an inheritable attribute, walking up `/Parent` until it is found.
///
/// `/FT`, `/Ff`, `/V` and `/DA` are all inheritable: a terminal field commonly
/// declares none of them and takes them from the node above.
fn inherited<'a>(document: &'a Document, field: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    let mut current = field;
    let mut seen: HashSet<ObjectId> = HashSet::new();

    loop {
        if let Ok(value) = current.get(key) {
            return resolve(document, value);
        }
        let parent_id = current.get(b"Parent").and_then(Object::as_reference).ok()?;
        // A crafted document can point a parent back at its own child.
        if !seen.insert(parent_id) {
            return None;
        }
        current = document.get_dictionary(parent_id).ok()?;
    }
}

/// Decode a PDF text string, whatever encoding it was written in.
fn text_of(object: &Object) -> Option<String> {
    match object {
        Object::String(..) => lopdf::decode_text_string(object).ok(),
        Object::Name(name) => Some(String::from_utf8_lossy(name).to_string()),
        _ => None,
    }
}

/// The value a field currently holds.
fn value_of(document: &Document, field: &Dictionary) -> Option<String> {
    match inherited(document, field, b"V")? {
        // A multi-select list holds an array; the first entry is reported,
        // which is what a single-value API can honestly say.
        Object::Array(values) => values.first().and_then(text_of),
        other => text_of(other),
    }
}

/// What a choice field offers.
///
/// `/Opt` entries are either a string, or a two-element array pairing the
/// exported value with the label shown to the user. The exported value is what
/// gets written back, so that is what is reported.
fn choice_options(document: &Document, field: &Dictionary) -> Vec<String> {
    let Some(Object::Array(entries)) = inherited(document, field, b"Opt") else {
        return Vec::new();
    };

    entries
        .iter()
        .filter_map(|entry| match resolve(document, entry)? {
            Object::Array(pair) => pair.first().and_then(text_of),
            other => text_of(other),
        })
        .collect()
}

/// The states a checkbox or radio accepts, read from its appearance streams.
///
/// A checkbox's "on" state is not a fixed name: the document chooses it (`/Yes`,
/// `/On`, `/1`, …), and writing the wrong one leaves the box unticked. The
/// names live in the widget's `/AP /N` dictionary, alongside `/Off`.
fn button_states(document: &Document, field_id: ObjectId, field: &Dictionary) -> Vec<String> {
    let mut states = Vec::new();

    let mut collect = |widget: &Dictionary| {
        let Some(Object::Dictionary(appearances)) =
            widget.get(b"AP").ok().and_then(|ap| resolve(document, ap))
        else {
            return;
        };
        let Some(Object::Dictionary(normal)) = appearances
            .get(b"N")
            .ok()
            .and_then(|normal| resolve(document, normal))
        else {
            return;
        };
        for (name, _) in normal.iter() {
            let name = String::from_utf8_lossy(name).to_string();
            if name != "Off" && !states.contains(&name) {
                states.push(name);
            }
        }
    };

    // The widget may be merged into the field itself, or be one of its kids.
    collect(field);
    if let Ok(Object::Array(kids)) = field.get(b"Kids") {
        for kid in kids {
            if let Some(Object::Dictionary(widget)) = resolve(document, kid) {
                collect(widget);
            }
        }
    }
    let _ = field_id;

    states
}

fn kind_of(document: &Document, field: &Dictionary, flags: i64) -> Option<FieldKind> {
    let field_type = match inherited(document, field, b"FT")? {
        Object::Name(name) => name.clone(),
        _ => return None,
    };

    Some(match field_type.as_slice() {
        b"Tx" => FieldKind::Text,
        b"Sig" => FieldKind::Signature,
        b"Btn" => {
            if flags & PUSH_BUTTON != 0 {
                FieldKind::PushButton
            } else if flags & RADIO != 0 {
                FieldKind::Radio
            } else {
                FieldKind::Checkbox
            }
        }
        b"Ch" => {
            if flags & COMBO != 0 {
                FieldKind::Dropdown
            } else {
                FieldKind::ListBox
            }
        }
        _ => return None,
    })
}

/// Walk a field and its children, collecting the terminal ones.
fn collect_fields(
    document: &Document,
    field_id: ObjectId,
    prefix: &str,
    seen: &mut HashSet<ObjectId>,
    out: &mut Vec<(ObjectId, FormField)>,
) {
    // Guards a /Kids cycle, which a crafted document can carry.
    if !seen.insert(field_id) {
        return;
    }
    let Ok(field) = document.get_dictionary(field_id) else {
        return;
    };

    let partial = field.get(b"T").ok().and_then(text_of).unwrap_or_default();
    let name = match (prefix.is_empty(), partial.is_empty()) {
        (_, true) => prefix.to_string(),
        (true, false) => partial,
        (false, false) => format!("{prefix}.{partial}"),
    };

    // A kid carrying /T is another field; one without is a widget — the
    // annotation that draws this field on a page.
    let child_fields: Vec<ObjectId> = match field.get(b"Kids") {
        Ok(Object::Array(kids)) => kids
            .iter()
            .filter_map(|kid| {
                let id = kid.as_reference().ok()?;
                let kid_dictionary = document.get_dictionary(id).ok()?;
                kid_dictionary.get(b"T").ok().map(|_| id)
            })
            .collect(),
        _ => Vec::new(),
    };

    if !child_fields.is_empty() {
        for child in child_fields {
            collect_fields(document, child, &name, seen, out);
        }
        return;
    }

    let flags = inherited(document, field, b"Ff")
        .and_then(|value| value.as_i64().ok())
        .unwrap_or(0);
    let Some(kind) = kind_of(document, field, flags) else {
        // A node with no /FT anywhere up the chain is a grouping node, not a
        // control. Skipped rather than reported as an unknown field.
        return;
    };

    let options = match kind {
        FieldKind::Dropdown | FieldKind::ListBox => choice_options(document, field),
        FieldKind::Checkbox | FieldKind::Radio => button_states(document, field_id, field),
        _ => Vec::new(),
    };

    out.push((
        field_id,
        FormField {
            name,
            kind,
            value: value_of(document, field),
            options,
            read_only: flags & READ_ONLY != 0,
            required: flags & REQUIRED != 0,
            multiline: kind == FieldKind::Text && flags & MULTILINE != 0,
            password: kind == FieldKind::Text && flags & PASSWORD != 0,
            max_length: inherited(document, field, b"MaxLen")
                .and_then(|value| value.as_i64().ok())
                .and_then(|value| u32::try_from(value).ok()),
        },
    ));
}

/// Every interactive field in the document, in the order the form declares
/// them. A document with no form yields an empty list rather than an error.
pub fn form_fields(bytes: &[u8]) -> Result<Vec<FormField>, String> {
    let document =
        Document::load_mem(bytes).map_err(|error| format!("cannot read PDF: {error}"))?;
    Ok(fields_of(&document)
        .into_iter()
        .map(|(_, field)| field)
        .collect())
}

/// The same walk, keeping each field's object id.
///
/// Filling needs the id to write back to; resolving names a second time would
/// be a second implementation of §12.7.3's hierarchy rules, free to drift.
pub(crate) fn fields_of(document: &Document) -> Vec<(ObjectId, FormField)> {
    let Ok(catalog) = document.catalog() else {
        return Vec::new();
    };
    let Some(Object::Dictionary(acro_form)) = catalog
        .get(b"AcroForm")
        .ok()
        .and_then(|form| resolve(document, form))
    else {
        return Vec::new();
    };
    let Some(Object::Array(roots)) = acro_form
        .get(b"Fields")
        .ok()
        .and_then(|fields| resolve(document, fields))
    else {
        return Vec::new();
    };

    let mut fields = Vec::new();
    let mut seen = HashSet::new();
    for root in roots {
        if let Ok(id) = root.as_reference() {
            collect_fields(document, id, "", &mut seen, &mut fields);
        }
    }

    fields
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    /// Build a document carrying an interactive form.
    ///
    /// Assembled by hand because no producer we depend on writes AcroForms,
    /// and because the cases that matter — inheritance, hierarchy, a
    /// checkbox's chosen "on" state — are exactly the ones a simple fixture
    /// would miss.
    fn form_document() -> Vec<u8> {
        let mut document = Document::with_version("1.7");
        let pages_id = document.new_object_id();

        // A plain text field, filled in.
        let full_name = document.add_object(dictionary! {
            "FT" => "Tx",
            "T" => lopdf::text_string("fullName"),
            "V" => lopdf::text_string("Amélie Durand"),
        });

        // Multiline, required, capped.
        let notes = document.add_object(dictionary! {
            "FT" => "Tx",
            "T" => lopdf::text_string("notes"),
            "Ff" => MULTILINE | REQUIRED,
            "MaxLen" => 500,
        });

        // Read-only, as a pre-filled reference usually is.
        let reference = document.add_object(dictionary! {
            "FT" => "Tx",
            "T" => lopdf::text_string("reference"),
            "V" => lopdf::text_string("CTR-2026-0042"),
            "Ff" => READ_ONLY,
        });

        // A checkbox whose "on" state is /Yes — the document picks that name,
        // and writing anything else would leave the box unticked.
        let accepted = document.add_object(dictionary! {
            "FT" => "Btn",
            "T" => lopdf::text_string("accepted"),
            "V" => Object::Name(b"Off".to_vec()),
            "AP" => dictionary! {
                "N" => dictionary! {
                    "Yes" => Object::Null,
                    "Off" => Object::Null,
                },
            },
        });

        // A radio group: the states live on the KID widgets, not on the field.
        let radio_id = document.new_object_id();
        let monthly = document.add_object(dictionary! {
            "Parent" => radio_id,
            "AP" => dictionary! {
                "N" => dictionary! { "Monthly" => Object::Null, "Off" => Object::Null },
            },
        });
        let yearly = document.add_object(dictionary! {
            "Parent" => radio_id,
            "AP" => dictionary! {
                "N" => dictionary! { "Yearly" => Object::Null, "Off" => Object::Null },
            },
        });
        document.objects.insert(
            radio_id,
            Object::Dictionary(dictionary! {
                "FT" => "Btn",
                "T" => lopdf::text_string("frequency"),
                "Ff" => RADIO,
                "V" => Object::Name(b"Yearly".to_vec()),
                "Kids" => vec![Object::Reference(monthly), Object::Reference(yearly)],
            }),
        );

        // A dropdown whose options mix bare strings and [export, label] pairs.
        let country = document.add_object(dictionary! {
            "FT" => "Ch",
            "T" => lopdf::text_string("country"),
            "Ff" => COMBO,
            "V" => lopdf::text_string("CH"),
            "Opt" => vec![
                lopdf::text_string("CH"),
                Object::Array(vec![
                    lopdf::text_string("FR"),
                    lopdf::text_string("France"),
                ]),
            ],
        });

        // A hierarchy: the children inherit /FT and are named through the parent.
        let address_id = document.new_object_id();
        let city = document.add_object(dictionary! {
            "Parent" => address_id,
            "T" => lopdf::text_string("city"),
            "V" => lopdf::text_string("Genève"),
        });
        let zip = document.add_object(dictionary! {
            "Parent" => address_id,
            "T" => lopdf::text_string("zip"),
        });
        document.objects.insert(
            address_id,
            Object::Dictionary(dictionary! {
                // Declared here only: the children carry no /FT of their own.
                "FT" => "Tx",
                "T" => lopdf::text_string("address"),
                "Kids" => vec![Object::Reference(city), Object::Reference(zip)],
            }),
        );

        let fields = vec![
            Object::Reference(full_name),
            Object::Reference(notes),
            Object::Reference(reference),
            Object::Reference(accepted),
            Object::Reference(radio_id),
            Object::Reference(country),
            Object::Reference(address_id),
        ];

        let page_id = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        });
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Count" => 1,
                "Kids" => vec![Object::Reference(page_id)],
            }),
        );
        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
            "AcroForm" => dictionary! { "Fields" => fields },
        });
        document.trailer.set("Root", catalog_id);

        let mut out = Vec::new();
        document.save_to(&mut out).unwrap();
        out
    }

    fn field<'a>(fields: &'a [FormField], name: &str) -> &'a FormField {
        fields
            .iter()
            .find(|field| field.name == name)
            .unwrap_or_else(|| {
                panic!(
                    "no field {name:?} — found {:?}",
                    fields.iter().map(|f| &f.name).collect::<Vec<_>>()
                )
            })
    }

    #[test]
    fn lists_every_field() {
        let fields = form_fields(&form_document()).expect("the form should read");

        let names: Vec<&str> = fields.iter().map(|field| field.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "fullName",
                "notes",
                "reference",
                "accepted",
                "frequency",
                "country",
                "address.city",
                "address.zip",
            ]
        );
    }

    #[test]
    fn reads_a_text_field_and_its_value() {
        let fields = form_fields(&form_document()).unwrap();

        let name = field(&fields, "fullName");
        assert_eq!(name.kind, FieldKind::Text);
        // Accented text survives the /Info-style encoding.
        assert_eq!(name.value.as_deref(), Some("Amélie Durand"));
        assert!(!name.required);
        assert!(!name.read_only);
    }

    #[test]
    fn reads_the_flags_that_change_how_a_field_behaves() {
        let fields = form_fields(&form_document()).unwrap();

        let notes = field(&fields, "notes");
        assert!(notes.multiline);
        assert!(notes.required);
        assert_eq!(notes.max_length, Some(500));
        assert_eq!(notes.value, None);

        assert!(field(&fields, "reference").read_only);
    }

    /// A checkbox's "on" state is chosen by the document, not fixed by the
    /// spec. Reporting it is what lets a caller tick the box correctly.
    #[test]
    fn reports_the_states_a_checkbox_accepts() {
        let fields = form_fields(&form_document()).unwrap();

        let accepted = field(&fields, "accepted");
        assert_eq!(accepted.kind, FieldKind::Checkbox);
        assert_eq!(accepted.options, vec!["Yes".to_string()]);
        assert_eq!(accepted.value.as_deref(), Some("Off"));
    }

    /// A radio group keeps its states on the kid widgets, so reading the field
    /// alone finds nothing.
    #[test]
    fn reports_the_states_of_a_radio_group() {
        let fields = form_fields(&form_document()).unwrap();

        let frequency = field(&fields, "frequency");
        assert_eq!(frequency.kind, FieldKind::Radio);
        assert_eq!(
            frequency.options,
            vec!["Monthly".to_string(), "Yearly".to_string()]
        );
        assert_eq!(frequency.value.as_deref(), Some("Yearly"));
    }

    #[test]
    fn reports_the_exported_value_of_each_choice() {
        let fields = form_fields(&form_document()).unwrap();

        let country = field(&fields, "country");
        assert_eq!(country.kind, FieldKind::Dropdown);
        // "FR" not "France": the export value is what gets written back.
        assert_eq!(country.options, vec!["CH".to_string(), "FR".to_string()]);
        assert_eq!(country.value.as_deref(), Some("CH"));
    }

    /// The two rules of §12.7.3 at once: the name is built from the ancestors,
    /// and `/FT` comes from a parent the child never repeats.
    #[test]
    fn resolves_names_and_types_through_the_hierarchy() {
        let fields = form_fields(&form_document()).unwrap();

        let city = field(&fields, "address.city");
        assert_eq!(city.kind, FieldKind::Text, "type comes from the parent");
        assert_eq!(city.value.as_deref(), Some("Genève"));

        assert_eq!(field(&fields, "address.zip").kind, FieldKind::Text);
    }

    #[test]
    fn a_document_without_a_form_has_no_fields() {
        let plain = crate::create_blank(&[(595.28, 841.89)]).unwrap();

        assert_eq!(form_fields(&plain).unwrap(), Vec::new());
    }

    #[test]
    fn refuses_bytes_that_are_not_a_pdf() {
        assert!(form_fields(b"not a PDF").is_err());
    }
}
