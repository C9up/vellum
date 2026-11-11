//! Where lopdf reads an untrusted document.
//!
//! lopdf decodes every object stream, and the cross-reference stream, while
//! it loads a document, before any of this engine's code has looked at it.
//! Unbounded, a 1 MiB upload whose object stream inflates to 1 GiB takes the
//! process with it, on every path that parses: inspecting, listing fields,
//! the check that refuses to rewrite a signed document, every edit. So every
//! one of them parses through here, with a ceiling on what one stream may
//! inflate to.

use lopdf::xref::XrefEntry;
use lopdf::{Document, LoadOptions, Reader};

/// The most one object or cross-reference stream may inflate to while a
/// document is parsed. They hold dictionaries and numbers, not page content:
/// a few hundred kilobytes is already a large one.
pub const MAX_OBJECT_STREAM_BYTES: usize = 32 * 1024 * 1024;

/// Parse a document, every stream lopdf decodes to do it bounded by
/// [`MAX_OBJECT_STREAM_BYTES`].
///
/// lopdf drops an object stream it could not decode, and every object in it,
/// without a word. Read without them, a document is not the one that was
/// sent: its signature field, say, could vanish from the check that refuses
/// to rewrite a signed document. So one the cross-reference table points into
/// and that did not load refuses the document.
pub(crate) fn parse(bytes: &[u8]) -> Result<Document, String> {
    let document = Document::load_mem_with_options(
        bytes,
        LoadOptions {
            max_decompressed_size: Some(MAX_OBJECT_STREAM_BYTES),
            ..Default::default()
        },
    )
    .map_err(|error| format!("cannot read PDF: {error}"))?;

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
            "cannot read PDF: object stream {container} cannot be read within {} MiB",
            MAX_OBJECT_STREAM_BYTES / (1024 * 1024)
        ));
    }
    Ok(document)
}

/// Read the trailer and the Info dictionary of a document, under the same
/// ceiling: the Info dictionary may sit in an object stream.
pub(crate) fn parse_metadata(bytes: &[u8]) -> Result<lopdf::PdfMetadata, String> {
    Reader {
        buffer: bytes,
        document: Document::new(),
        encryption_state: None,
        raw_objects: Default::default(),
        password: None,
        strict: false,
        max_decompressed_size: Some(MAX_OBJECT_STREAM_BYTES),
    }
    .read_metadata()
    .map_err(|error| format!("cannot read PDF metadata: {error}"))
}
