//! Preparing a document for signature, and putting the signature into it.
//!
//! A PDF signature covers a byte range **of the document it lives in**, which
//! makes the usual order impossible: the value cannot be computed and then
//! assembled, because assembling it would change what it covers. The document
//! is instead written with a hole where the value will go, the `/ByteRange`
//! records everything but the hole, and the value is later dropped into the
//! space reserved for it without moving a single other byte.
//!
//! That is also what makes a local key and a remote provider interchangeable:
//! whoever signs never sees the document, only the digest of it. The two
//! halves are deliberately separate functions with plain bytes between them,
//! so what happens in the middle — a key in a file, a call to a certified
//! provider — is not this module's business.
//!
//! The signature is written incrementally: the original bytes are preserved
//! untouched and a revision is appended. Rewriting the file would invalidate
//! any signature already on it, and destroy the very history a signature
//! exists to establish.

use lopdf::{dictionary, Dictionary, Document, IncrementalDocument, Object, ObjectId};
use sha2::{Digest, Sha256};

/// What the signature says about itself.
#[derive(Debug, Clone, Default)]
pub struct SignatureOptions {
    /// Why the document was signed.
    pub reason: Option<String>,
    /// Where it was signed.
    pub location: Option<String>,
    /// How to reach the signatory.
    pub contact: Option<String>,
    /// Who signed, as it should be displayed.
    pub name: Option<String>,
    /// When, as an ISO 8601 instant. Omitted from the document when absent.
    pub signed_at: Option<String>,
    /// Bytes reserved for the signature value. A basic CMS needs a few
    /// thousand; one carrying a timestamp and revocation data needs more, and
    /// the room cannot be found later.
    pub capacity: usize,
}

/// The default reservation: comfortable for a timestamped signature, and
/// small enough not to matter next to the document.
pub const DEFAULT_CAPACITY: usize = 16384;

/// The smallest reservation accepted, in bytes.
///
/// It exists so a reservation can be told apart from any other hex string in
/// the document: nothing legitimate is a thousand zeros long. It is also far
/// below what any real signature needs.
const MINIMUM_CAPACITY: usize = 512;

/// A document waiting for its signature value.
#[derive(Debug, Clone)]
pub struct Prepared {
    /// The document, with the value reserved and empty. The `/ByteRange` is
    /// already final: dropping the value in changes no offset.
    pub document: Vec<u8>,
    /// What the signature has to be computed over.
    pub digest: [u8; 32],
}

/// The field name, and the only one this writes.
const FIELD_NAME: &str = "Signature1";

/// Turn an ISO 8601 instant into the form a PDF date takes.
///
/// Only the shape is converted, not the instant: an input that is not a date
/// comes back unchanged rather than becoming a wrong one.
fn iso_to_pdf_date(iso: &str) -> String {
    let digits: String = iso
        .chars()
        .take_while(|character| *character != '+' && *character != 'Z')
        .filter(|character| character.is_ascii_digit())
        .collect();
    if digits.len() < 14 {
        return iso.to_string();
    }

    let zone = if iso.ends_with('Z') {
        "Z00'00'".to_string()
    } else {
        match iso.rfind(['+', '-']) {
            // The sign has to sit after the time, not inside the date.
            Some(at) if at > 10 => {
                let offset: String = iso[at..].chars().filter(|c| c.is_ascii_digit()).collect();
                if offset.len() >= 4 {
                    format!("{}{}'{}'", &iso[at..=at], &offset[..2], &offset[2..4])
                } else {
                    "Z00'00'".to_string()
                }
            }
            _ => "Z00'00'".to_string(),
        }
    };

    format!("D:{}{zone}", &digits[..14])
}

/// The `/AcroForm` dictionary of the document, created when it has none.
///
/// Returned by identifier, because both the caller and the catalog have to
/// point at the same one.
fn acro_form(incremental: &mut IncrementalDocument) -> Result<ObjectId, String> {
    let catalog_id = incremental
        .get_prev_documents()
        .trailer
        .get(b"Root")
        .and_then(|root| root.as_reference())
        .map_err(|error| format!("the document has no catalog: {error}"))?;

    incremental
        .opt_clone_object_to_new_document(catalog_id)
        .map_err(|error| format!("cannot take the catalog: {error}"))?;

    let existing = incremental
        .new_document
        .get_dictionary(catalog_id)
        .ok()
        .and_then(|catalog| catalog.get(b"AcroForm").ok().cloned());

    let form_id = match existing {
        Some(Object::Reference(id)) => {
            incremental
                .opt_clone_object_to_new_document(id)
                .map_err(|error| format!("cannot take the form: {error}"))?;
            id
        }
        // An inline form has to become an indirect one, since the revision
        // cannot rewrite the catalog's own bytes in place.
        Some(Object::Dictionary(dictionary)) => incremental.new_document.add_object(dictionary),
        _ => incremental.new_document.add_object(Dictionary::new()),
    };

    if let Ok(catalog) = incremental
        .new_document
        .get_object_mut(catalog_id)
        .and_then(|object| object.as_dict_mut())
    {
        catalog.set("AcroForm", Object::Reference(form_id));
    }

    Ok(form_id)
}

/// Write the signature field, its widget and its empty value.
fn add_signature_field(
    incremental: &mut IncrementalDocument,
    options: &SignatureOptions,
    capacity: usize,
) -> Result<ObjectId, String> {
    let page_id = *incremental
        .get_prev_documents()
        .get_pages()
        .values()
        .next()
        .ok_or_else(|| "the document has no page".to_string())?;

    let mut signature = dictionary! {
        "Type" => "Sig",
        "Filter" => "Adobe.PPKLite",
        // The PAdES subfilter. A CMS detached signature, as ETSI EN 319 142
        // requires, rather than the older Adobe flavours.
        "SubFilter" => "ETSI.CAdES.detached",
        // Placeholder digits, wide enough that the real offsets always fit and
        // can be written over them without moving anything.
        "ByteRange" => vec![
            Object::Integer(0),
            Object::Integer(9_999_999_999),
            Object::Integer(9_999_999_999),
            Object::Integer(9_999_999_999),
        ],
        "Contents" => Object::String(vec![0; capacity], lopdf::StringFormat::Hexadecimal),
    };
    for (key, value) in [
        ("Reason", &options.reason),
        ("Location", &options.location),
        ("ContactInfo", &options.contact),
        ("Name", &options.name),
    ] {
        if let Some(value) = value {
            signature.set(key, Object::string_literal(value.as_str()));
        }
    }
    if let Some(signed_at) = &options.signed_at {
        signature.set("M", Object::string_literal(iso_to_pdf_date(signed_at)));
    }
    let signature_id = incremental.new_document.add_object(signature);

    // The field and its widget are one object, which is what an invisible
    // signature is: a zero-sized annotation carrying the value.
    let field_id = incremental.new_document.add_object(dictionary! {
        "Type" => "Annot",
        "Subtype" => "Widget",
        "FT" => "Sig",
        "T" => lopdf::text_string(FIELD_NAME),
        "V" => Object::Reference(signature_id),
        "Rect" => vec![0.into(), 0.into(), 0.into(), 0.into()],
        // Printed, and its appearance locked.
        "F" => 132,
        "P" => Object::Reference(page_id),
    });

    // The page keeps whatever annotations it had; the widget joins them.
    incremental
        .opt_clone_object_to_new_document(page_id)
        .map_err(|error| format!("cannot take the page: {error}"))?;
    let existing = incremental
        .new_document
        .get_dictionary(page_id)
        .ok()
        .and_then(|page| page.get(b"Annots").ok().cloned());
    let mut annotations = match existing {
        Some(Object::Array(items)) => items,
        Some(Object::Reference(id)) => incremental
            .get_prev_documents()
            .get_object(id)
            .and_then(|object| object.as_array())
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    annotations.push(Object::Reference(field_id));
    if let Ok(page) = incremental
        .new_document
        .get_object_mut(page_id)
        .and_then(|object| object.as_dict_mut())
    {
        page.set("Annots", Object::Array(annotations));
    }

    let form_id = acro_form(incremental)?;
    let existing = incremental
        .new_document
        .get_dictionary(form_id)
        .ok()
        .and_then(|form| form.get(b"Fields").ok().cloned());
    let mut fields = match existing {
        Some(Object::Array(items)) => items,
        Some(Object::Reference(id)) => incremental
            .get_prev_documents()
            .get_object(id)
            .and_then(|object| object.as_array())
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    fields.push(Object::Reference(field_id));
    if let Ok(form) = incremental
        .new_document
        .get_object_mut(form_id)
        .and_then(|object| object.as_dict_mut())
    {
        form.set("Fields", Object::Array(fields));
        // Signatures exist and one of them is unsigned.
        form.set("SigFlags", 3);
    }

    Ok(signature_id)
}

/// Find the span the signature value was reserved in, brackets included.
fn reserved_span(document: &[u8], capacity: usize) -> Option<(usize, usize)> {
    let mut needle = Vec::with_capacity(capacity * 2 + 2);
    needle.push(b'<');
    needle.extend(std::iter::repeat_n(b'0', capacity * 2));
    needle.push(b'>');

    let at = document
        .windows(needle.len())
        .position(|window| window == needle)?;
    Some((at, at + needle.len()))
}

/// Find a reservation of any size: an unbroken run of zeros in angle
/// brackets, long enough that no other hex string could be mistaken for it.
fn reserved_run(document: &[u8]) -> Option<(usize, usize)> {
    let mut from = 0;
    while let Some(offset) = document[from..].iter().position(|byte| *byte == b'<') {
        let open = from + offset;
        match document[open + 1..].iter().position(|byte| *byte == b'>') {
            Some(length) if length >= MINIMUM_CAPACITY * 2 => {
                let close = open + 1 + length;
                if document[open + 1..close].iter().all(|byte| *byte == b'0') {
                    return Some((open, close + 1));
                }
                from = open + 1;
            }
            _ => from = open + 1,
        }
    }
    None
}

/// Find the `/ByteRange` array, so the placeholder can be written over.
fn byte_range_span(document: &[u8]) -> Option<(usize, usize)> {
    let key = b"/ByteRange";
    let at = document.windows(key.len()).position(|w| w == key)?;
    let open = at + document[at..].iter().position(|byte| *byte == b'[')?;
    let close = open + document[open..].iter().position(|byte| *byte == b']')?;
    Some((open, close + 1))
}

/// Write a document with room for a signature, and say what has to be signed.
///
/// The document that comes back is complete and valid; it simply carries an
/// empty signature. Nothing else about it moves when the value arrives.
pub fn prepare(pdf: &[u8], options: &SignatureOptions) -> Result<Prepared, String> {
    let capacity = match options.capacity {
        0 => DEFAULT_CAPACITY,
        asked if asked < MINIMUM_CAPACITY => {
            return Err(format!(
            "a signature needs at least {MINIMUM_CAPACITY} bytes reserved, {asked} were asked for"
        ))
        }
        asked => asked,
    };

    let previous = Document::load_mem(pdf).map_err(|error| format!("cannot read PDF: {error}"))?;
    let version = previous.version.clone();
    let mut incremental = IncrementalDocument::create_from(pdf.to_vec(), previous);
    // The appended revision carries a header of its own, which lopdf writes
    // from the new document's version — 1.4 by default. It is only a comment
    // and a reader ignores it, but a revision announcing a version older than
    // the document it extends is misleading to anyone reading the bytes.
    incremental.new_document.version = version;

    add_signature_field(&mut incremental, options, capacity)?;

    let mut document = Vec::new();
    incremental
        .save_to(&mut document)
        .map_err(|error| format!("cannot write PDF: {error}"))?;

    let (hole_start, hole_end) = reserved_span(&document, capacity)
        .ok_or_else(|| "the reserved signature space was not written as expected".to_string())?;
    let (range_start, range_end) = byte_range_span(&document)
        .ok_or_else(|| "the byte range was not written as expected".to_string())?;

    // Everything but the hole, which is exactly what the signature covers.
    let tail = document.len() - hole_end;
    let written = format!("[0 {hole_start} {hole_end} {tail}");
    let room = range_end - range_start;
    if written.len() + 1 > room {
        return Err("the byte range does not fit its placeholder".to_string());
    }
    let mut replacement = written.into_bytes();
    replacement.resize(room - 1, b' ');
    replacement.push(b']');
    document[range_start..range_end].copy_from_slice(&replacement);

    let mut hasher = Sha256::new();
    hasher.update(&document[..hole_start]);
    hasher.update(&document[hole_end..]);

    Ok(Prepared {
        document,
        digest: hasher.finalize().into(),
    })
}

/// Put the signature value into the space `prepare` reserved for it.
///
/// Nothing else in the document is touched, which is the point: the bytes the
/// digest was taken over are the same bytes afterwards.
pub fn embed_signature(prepared: &[u8], value: &[u8]) -> Result<Vec<u8>, String> {
    // The reservation's size is not passed in; it is read back from the
    // document, as the run of zeros between the brackets.
    let (hole_start, hole_end) = reserved_run(prepared)
        .ok_or_else(|| "this document has no reserved signature space".to_string())?;

    let room = hole_end - hole_start - 2;
    if value.len() * 2 > room {
        return Err(format!(
            "the signature needs {} bytes of hex and only {room} were reserved — \
             prepare the document with a larger capacity",
            value.len() * 2
        ));
    }

    let mut signed = prepared.to_vec();
    let mut hex = Vec::with_capacity(room);
    for byte in value {
        hex.extend_from_slice(format!("{byte:02X}").as_bytes());
    }
    // The rest stays zero: the reservation keeps its length whatever the
    // signature's, because every offset in the document depends on it.
    hex.resize(room, b'0');
    signed[hole_start + 1..hole_end - 1].copy_from_slice(&hex);
    Ok(signed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{create_blank, form_fields, inspect};

    const A4: (f32, f32) = (595.28, 841.89);

    fn blank() -> Vec<u8> {
        create_blank(&[A4]).unwrap()
    }

    fn options() -> SignatureOptions {
        SignatureOptions {
            reason: Some("Mandat de prevoyance".to_string()),
            location: Some("Lausanne".to_string()),
            name: Some("Amelie Durand".to_string()),
            signed_at: Some("2026-09-04T14:30:00Z".to_string()),
            ..Default::default()
        }
    }

    /// Read the four numbers back out of the document.
    fn byte_range(document: &[u8]) -> Vec<usize> {
        let (start, end) = byte_range_span(document).expect("a byte range");
        String::from_utf8_lossy(&document[start + 1..end - 1])
            .split_whitespace()
            .map(|number| number.parse().expect("a number"))
            .collect()
    }

    /// The whole correctness of a signature rests on these four numbers: they
    /// have to cover every byte of the document except the reserved space,
    /// exactly, or the signature verifies against something that is not the
    /// document.
    #[test]
    fn the_byte_range_covers_everything_but_the_hole() {
        let prepared = prepare(&blank(), &options()).expect("preparing should succeed");
        let range = byte_range(&prepared.document);
        let (hole_start, hole_end) =
            reserved_run(&prepared.document).expect("the reservation is there");

        assert_eq!(range.len(), 4);
        assert_eq!(range[0], 0, "the first stretch starts at the beginning");
        assert_eq!(range[1], hole_start, "and stops where the hole opens");
        assert_eq!(range[2], hole_end, "the second resumes after it");
        assert_eq!(
            range[1] + range[3] + (hole_end - hole_start),
            prepared.document.len(),
            "and together they account for every byte of the file"
        );
        assert_eq!(
            range[2] + range[3],
            prepared.document.len(),
            "the second stretch runs to the end"
        );
    }

    /// The digest has to be over the covered ranges and nothing else. Computed
    /// here independently of the code that produced it.
    #[test]
    fn the_digest_is_over_what_the_byte_range_says() {
        let prepared = prepare(&blank(), &options()).expect("preparing should succeed");
        let range = byte_range(&prepared.document);

        let mut expected = Sha256::new();
        expected.update(&prepared.document[range[0]..range[0] + range[1]]);
        expected.update(&prepared.document[range[2]..range[2] + range[3]]);
        let expected: [u8; 32] = expected.finalize().into();

        assert_eq!(prepared.digest, expected);
    }

    /// The reason the two halves are separate: putting the value in must not
    /// disturb a single byte the digest was taken over.
    #[test]
    fn embedding_the_value_changes_nothing_else() {
        let prepared = prepare(&blank(), &options()).expect("preparing should succeed");
        let signed = embed_signature(&prepared.document, &[0xAB; 200]).expect("embedding");

        assert_eq!(
            signed.len(),
            prepared.document.len(),
            "the file cannot change length"
        );

        let range = byte_range(&signed);
        let mut after = Sha256::new();
        after.update(&signed[range[0]..range[0] + range[1]]);
        after.update(&signed[range[2]..range[2] + range[3]]);
        let after: [u8; 32] = after.finalize().into();

        assert_eq!(
            after, prepared.digest,
            "what was signed is still what the document holds"
        );
    }

    #[test]
    fn the_value_lands_where_it_was_reserved() {
        let prepared = prepare(&blank(), &options()).expect("preparing should succeed");
        let signed = embed_signature(&prepared.document, &[0x30, 0x82, 0x01]).expect("embedding");

        let (hole_start, hole_end) = reserved_run(&prepared.document).expect("a reservation");
        assert_eq!(
            &signed[hole_start + 1..hole_start + 7],
            b"308201",
            "the value goes in as hex, at the front of the space"
        );
        assert!(
            signed[hole_start + 7..hole_end - 1]
                .iter()
                .all(|byte| *byte == b'0'),
            "and the rest of the reservation stays as it was"
        );
    }

    /// The original bytes are preserved, not rewritten. Rewriting them would
    /// invalidate any signature already on the document and destroy the very
    /// history a signature exists to establish.
    #[test]
    fn the_document_it_signs_is_left_untouched() {
        let source = blank();
        let prepared = prepare(&source, &options()).expect("preparing should succeed");

        assert_eq!(
            &prepared.document[..source.len()],
            &source[..],
            "the previous revision has to survive byte for byte"
        );
        assert!(prepared.document.len() > source.len(), "and be added to");
    }

    /// A signature is a form field, so the document has to come back with one
    /// — and still be a document a reader can open.
    #[test]
    fn the_result_is_a_document_with_a_signature_field() {
        let prepared = prepare(&blank(), &options()).expect("preparing should succeed");
        let signed = embed_signature(&prepared.document, &[0xAB; 200]).expect("embedding");

        assert_eq!(inspect(&signed).expect("it parses").page_count, 1);
        let fields = form_fields(&signed).expect("it has a form");
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].name, FIELD_NAME);
        assert_eq!(fields[0].kind.as_str(), "signature");
    }

    /// Signing a document that already has a form adds to it rather than
    /// replacing it — a mandate is signed once its fields are filled.
    #[test]
    fn a_document_that_already_has_a_form_keeps_it() {
        let filled = crate::fill_form(
            &crate::fill::tests::form_document(),
            &[crate::FieldValue {
                name: "fullName".to_string(),
                value: "Amelie Durand".to_string(),
            }],
        )
        .expect("filling should succeed");
        let before = form_fields(&filled).expect("it has a form").len();

        let prepared = prepare(&filled, &options()).expect("preparing should succeed");
        let signed = embed_signature(&prepared.document, &[0xAB; 200]).expect("embedding");

        let after = form_fields(&signed).expect("it still has a form");
        assert_eq!(after.len(), before + 1, "the fields are kept and one added");
        assert!(after.iter().any(|field| field.name == FIELD_NAME));
    }

    #[test]
    fn refuses_a_signature_larger_than_the_space_reserved() {
        let prepared = prepare(&blank(), &options()).expect("preparing should succeed");
        let error = embed_signature(&prepared.document, &[0xAB; DEFAULT_CAPACITY + 1])
            .expect_err("it does not fit");
        assert!(error.contains("larger capacity"), "got {error:?}");
    }

    #[test]
    fn refuses_a_reservation_too_small_to_find_again() {
        let error = prepare(
            &blank(),
            &SignatureOptions {
                capacity: 8,
                ..Default::default()
            },
        )
        .expect_err("eight bytes is not a signature");
        assert!(error.contains("at least"), "got {error:?}");
    }

    /// The appended revision must not announce a version older than the
    /// document it extends.
    #[test]
    fn the_revision_keeps_the_document_version() {
        let source = blank();
        let before = inspect(&source).expect("it parses").version;
        let prepared = prepare(&source, &options()).expect("preparing should succeed");

        assert_eq!(
            inspect(&prepared.document).expect("it parses").version,
            before
        );
        let revision = String::from_utf8_lossy(&prepared.document[source.len()..]).to_string();
        assert!(
            revision.contains(&format!("%PDF-{before}")),
            "the revision header should match, got {:?}",
            &revision[..40.min(revision.len())]
        );
    }

    #[test]
    fn writes_the_date_in_the_form_a_pdf_takes() {
        assert_eq!(
            iso_to_pdf_date("2026-09-04T14:30:00Z"),
            "D:20260904143000Z00'00'"
        );
        assert_eq!(
            iso_to_pdf_date("2026-09-04T14:30:00+02:00"),
            "D:20260904143000+02'00'"
        );
        // Not a date: kept as it came rather than turned into a wrong one.
        assert_eq!(iso_to_pdf_date("yesterday"), "yesterday");
    }
}
