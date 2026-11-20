//! Where lopdf reads an untrusted document.
//!
//! lopdf decodes every object stream, and the cross-reference stream, while
//! it loads a document, before any of this engine's code has looked at it,
//! and parses every object it finds. Unbounded, a 1 MiB upload whose object
//! stream inflates to 1 GiB takes the process with it, on every path that
//! parses: inspecting, listing fields, the check that refuses to rewrite a
//! signed document, every edit. So every one of them parses through here,
//! under one budget for the whole document: what its object and
//! cross-reference streams inflate to together, and how many objects the
//! parser may build from them and from the body.

use lopdf::budget::LoadBudget;
use lopdf::xref::XrefEntry;
use lopdf::{Document, LoadOptions, Reader};

/// The most every object and cross-reference stream of one document may
/// inflate to while it is parsed, together. They hold dictionaries and
/// numbers, not page content: a few hundred kilobytes is already a large
/// one, and a document of a hundred thousand objects holds a few megabytes.
pub const MAX_OBJECT_STREAM_BYTES: usize = 32 * 1024 * 1024;

/// The most objects parsing one document may build: array elements,
/// dictionary values and cross-reference entries, counted alike.
///
/// The inflated size does not bound this: `0 ` is two bytes and one object of
/// about a hundred, so a 32 MiB stream of them was two gigabytes. Four
/// million is room for a document of several hundred thousand objects, and
/// a few hundred megabytes at most.
pub const MAX_PARSED_OBJECTS: u64 = 4_000_000;

/// Parse a document under the budget above.
///
/// lopdf drops an object stream it could not decode, and every object in it,
/// without a word. Read without them, a document is not the one that was
/// sent: its signature field, say, could vanish from the check that refuses
/// to rewrite a signed document. So one the cross-reference table points into
/// and that did not load refuses the document, and so does a document whose
/// objects ran past the count, since lopdf drops those too.
pub(crate) fn parse(bytes: &[u8]) -> Result<Document, String> {
    let budget = LoadBudget::open(MAX_OBJECT_STREAM_BYTES, MAX_PARSED_OBJECTS);
    let document = Document::load_mem_with_options(
        bytes,
        LoadOptions {
            max_decompressed_size: Some(MAX_OBJECT_STREAM_BYTES),
            ..Default::default()
        },
    );
    if budget.objects_exhausted() {
        return Err(format!(
            "cannot read PDF: it holds more than {MAX_PARSED_OBJECTS} objects"
        ));
    }
    let document = document.map_err(|error| format!("cannot read PDF: {error}"))?;
    drop(budget);

    let lost = document
        .reference_table
        .entries
        .iter()
        .find_map(|(&number, entry)| match entry {
            XrefEntry::Compressed { container, .. }
                if !document.objects.contains_key(&(number, 0)) =>
            {
                Some(*container)
            }
            _ => None,
        });
    if let Some(container) = lost {
        return Err(format!(
            "cannot read PDF: object stream {container} cannot be read within the {} MiB its object streams may inflate to together",
            MAX_OBJECT_STREAM_BYTES / (1024 * 1024)
        ));
    }
    Ok(document)
}

/// Read the trailer and the Info dictionary of a document, under the same
/// ceiling: the Info dictionary may sit in an object stream.
pub(crate) fn parse_metadata(bytes: &[u8]) -> Result<lopdf::PdfMetadata, String> {
    let budget = LoadBudget::open(MAX_OBJECT_STREAM_BYTES, MAX_PARSED_OBJECTS);
    let metadata = Reader {
        buffer: bytes,
        document: Document::new(),
        encryption_state: None,
        raw_objects: Default::default(),
        password: None,
        strict: false,
        max_decompressed_size: Some(MAX_OBJECT_STREAM_BYTES),
    }
    .read_metadata();
    if budget.objects_exhausted() {
        return Err(format!(
            "cannot read PDF metadata: it holds more than {MAX_PARSED_OBJECTS} objects"
        ));
    }
    metadata.map_err(|error| format!("cannot read PDF metadata: {error}"))
}
