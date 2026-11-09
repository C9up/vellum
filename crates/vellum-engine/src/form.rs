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
use std::rc::Rc;

use lopdf::{Dictionary, Document, Object, ObjectId};

/// How deep the field tree may go. Fully qualified names this deep do not
/// occur in real forms, and the bound is what keeps a crafted /Kids or
/// /Parent chain from costing work quadratic in its length.
pub(crate) const MAX_FIELD_DEPTH: usize = 64;

/// How many field nodes, groups included, one walk may visit: ten times the
/// fields this package writes into a document of its own.
const MAX_FIELD_NODES: usize = 100_000;

/// How much text — names, values, options — one walk may produce. Every
/// field repeats its ancestors' names and may inherit a shared /Opt array,
/// so without a budget a small document could expand into gigabytes.
const MAX_FORM_TEXT: usize = 32 * 1024 * 1024;

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
pub(crate) fn inherited<'a>(
    document: &'a Document,
    field: &'a Dictionary,
    key: &[u8],
) -> Option<&'a Object> {
    let mut current = field;
    // No deeper than the tree itself may go, which also ends a /Parent cycle.
    for _ in 0..=MAX_FIELD_DEPTH {
        if let Ok(value) = current.get(key) {
            return resolve(document, value);
        }
        let parent_id = current.get(b"Parent").and_then(Object::as_reference).ok()?;
        current = document.get_dictionary(parent_id).ok()?;
    }
    None
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
    let mut known = HashSet::new();

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
            if name != "Off" && known.insert(name.clone()) {
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

/// What one walk has spent so far.
struct Walk {
    nodes: usize,
    text: usize,
}

impl Walk {
    fn charge(&mut self, bytes: usize) -> Result<(), String> {
        self.text = self.text.saturating_add(bytes);
        if self.text > MAX_FORM_TEXT {
            return Err(format!(
                "the form's fields describe more than {} MiB of text",
                MAX_FORM_TEXT / (1024 * 1024)
            ));
        }
        Ok(())
    }
}

/// A field still to visit, with the qualified name of its parent.
struct Pending {
    id: ObjectId,
    prefix: Rc<str>,
    depth: usize,
}

/// Visit one field: either queue its child fields, or describe it.
fn visit(
    document: &Document,
    pending: Pending,
    walk: &mut Walk,
    stack: &mut Vec<Pending>,
    out: &mut Vec<(ObjectId, FormField)>,
) -> Result<(), String> {
    let Pending { id, prefix, depth } = pending;
    let Ok(field) = document.get_dictionary(id) else {
        return Ok(());
    };

    let partial = field.get(b"T").ok().and_then(text_of).unwrap_or_default();
    let name = match (prefix.is_empty(), partial.is_empty()) {
        (_, true) => prefix.to_string(),
        (true, false) => partial,
        (false, false) => format!("{prefix}.{partial}"),
    };
    walk.charge(name.len())?;

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
        if depth + 1 >= MAX_FIELD_DEPTH {
            return Err(format!(
                "the form's field tree is deeper than {MAX_FIELD_DEPTH} levels"
            ));
        }
        let name: Rc<str> = Rc::from(name);
        // Reversed, so that the stack pops them in declaration order.
        for child in child_fields.into_iter().rev() {
            stack.push(Pending {
                id: child,
                prefix: Rc::clone(&name),
                depth: depth + 1,
            });
        }
        return Ok(());
    }

    let flags = inherited(document, field, b"Ff")
        .and_then(|value| value.as_i64().ok())
        .unwrap_or(0);
    let Some(kind) = kind_of(document, field, flags) else {
        // A node with no /FT anywhere up the chain is a grouping node, not a
        // control. Skipped rather than reported as an unknown field.
        return Ok(());
    };

    let options = match kind {
        FieldKind::Dropdown | FieldKind::ListBox => choice_options(document, field),
        FieldKind::Checkbox | FieldKind::Radio => button_states(document, id, field),
        _ => Vec::new(),
    };
    let value = value_of(document, field);
    walk.charge(options.iter().map(String::len).sum::<usize>())?;
    walk.charge(value.as_ref().map_or(0, String::len))?;

    out.push((
        id,
        FormField {
            name,
            kind,
            value,
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
    Ok(())
}

/// Every interactive field in the document, in the order the form declares
/// them. A document with no form yields an empty list rather than an error.
pub fn form_fields(bytes: &[u8]) -> Result<Vec<FormField>, String> {
    let document =
        Document::load_mem(bytes).map_err(|error| format!("cannot read PDF: {error}"))?;
    Ok(fields_of(&document)?
        .into_iter()
        .map(|(_, field)| field)
        .collect())
}

/// Whether the document carries a signature: a signature field holding a
/// value.
///
/// A signature's value sits as raw bytes between the two ranges it signs, so
/// its dictionary cannot be inside a compressed object stream: a file with no
/// `/ByteRange` anywhere in its bytes carries none, without being parsed.
pub fn is_signed(bytes: &[u8]) -> Result<bool, String> {
    if !bytes.windows(10).any(|window| window == b"/ByteRange") {
        return Ok(false);
    }
    let document =
        Document::load_mem(bytes).map_err(|error| format!("cannot read PDF: {error}"))?;
    Ok(fields_of(&document)?.iter().any(|(id, field)| {
        field.kind == FieldKind::Signature
            && document
                .get_dictionary(*id)
                .is_ok_and(|dictionary| dictionary.get(b"V").is_ok())
    }))
}

/// Refuse to rewrite a signed document unless told the signatures may go.
///
/// Every edit here writes the whole file anew, and a signature covers bytes:
/// rewritten, it no longer verifies, and flattening even removes the field
/// that held it. Nothing about the result says so, so the caller is asked.
pub fn refuse_if_signed(bytes: &[u8], discard_signatures: bool) -> Result<(), String> {
    if !discard_signatures && is_signed(bytes)? {
        return Err("SIGNED_DOCUMENT: the document is signed, and rewriting it would invalidate the signature — pass discardSignatures: true to do it anyway".to_string());
    }
    Ok(())
}

/// The same walk, keeping each field's object id.
///
/// Filling needs the id to write back to; resolving names a second time would
/// be a second implementation of §12.7.3's hierarchy rules, free to drift.
///
/// Walked with a stack of our own rather than by recursion: the depth of the
/// tree is the document's to choose, and a thread's stack is not.
pub(crate) fn fields_of(document: &Document) -> Result<Vec<(ObjectId, FormField)>, String> {
    let Ok(catalog) = document.catalog() else {
        return Ok(Vec::new());
    };
    let Some(Object::Dictionary(acro_form)) = catalog
        .get(b"AcroForm")
        .ok()
        .and_then(|form| resolve(document, form))
    else {
        return Ok(Vec::new());
    };
    let Some(Object::Array(roots)) = acro_form
        .get(b"Fields")
        .ok()
        .and_then(|fields| resolve(document, fields))
    else {
        return Ok(Vec::new());
    };

    let root_prefix: Rc<str> = Rc::from("");
    let mut stack: Vec<Pending> = roots
        .iter()
        .rev()
        .filter_map(|root| root.as_reference().ok())
        .map(|id| Pending {
            id,
            prefix: Rc::clone(&root_prefix),
            depth: 0,
        })
        .collect();

    let mut fields = Vec::new();
    // Guards a /Kids cycle, which a crafted document can carry.
    let mut seen = HashSet::new();
    let mut walk = Walk { nodes: 0, text: 0 };
    while let Some(pending) = stack.pop() {
        if !seen.insert(pending.id) {
            continue;
        }
        walk.nodes += 1;
        if walk.nodes > MAX_FIELD_NODES {
            return Err(format!("the form has more than {MAX_FIELD_NODES} fields"));
        }
        visit(document, pending, &mut walk, &mut stack, &mut fields)?;
    }

    Ok(fields)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    /// A one-page document whose form holds one signature field, filled with
    /// a value or left empty.
    fn signature_field(filled: bool) -> Vec<u8> {
        let mut document = Document::with_version("1.7");
        let pages_id = document.new_object_id();
        let page_id = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        });
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(page_id)],
                "Count" => 1,
            }),
        );
        let mut field = dictionary! {
            "FT" => "Sig",
            "T" => lopdf::text_string("signature"),
        };
        if filled {
            let value = document.add_object(dictionary! {
                "Type" => "Sig",
                "ByteRange" => vec![0.into(), 1.into(), 2.into(), 3.into()],
                "Contents" => Object::String(vec![0], lopdf::StringFormat::Hexadecimal),
            });
            field.set("V", value);
        } else {
            // The key alone, so the fast path does not decide.
            field.set("Note", Object::Name(b"ByteRange".to_vec()));
        }
        let field_id = document.add_object(field);
        let catalog = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
            "AcroForm" => dictionary! { "Fields" => vec![Object::Reference(field_id)] },
        });
        document.trailer.set("Root", catalog);
        let mut bytes = Vec::new();
        document.save_to(&mut bytes).expect("saves");
        bytes
    }

    #[test]
    fn a_signature_field_with_a_value_is_a_signed_document() {
        assert_eq!(is_signed(&signature_field(true)), Ok(true));
        let refused = refuse_if_signed(&signature_field(true), false).expect_err("refused");
        assert!(refused.starts_with("SIGNED_DOCUMENT: "), "{refused}");
        assert_eq!(refuse_if_signed(&signature_field(true), true), Ok(()));
    }

    #[test]
    fn an_empty_signature_field_is_a_place_for_one_not_a_signature() {
        let bytes = signature_field(false);
        assert!(bytes.windows(10).any(|window| window == b"/ByteRange"));
        assert_eq!(is_signed(&bytes), Ok(false));
    }

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
