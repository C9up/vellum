//! Core PDF engine.
//!
//! Two crates carry the load, and they answer different questions:
//!
//! - `lopdf` owns documents that already exist. It parses the object tree and
//!   lets us operate on it: pages, metadata, encryption, form fields.
//! - `krilla` owns documents we author. Its `pdf` feature can re-embed the
//!   pages of an existing file, which is how stamping a third-party document
//!   stays on the authoring path instead of hand-writing content streams.

use krilla::geom::Size;
use krilla::page::PageSettings;
mod edit;
mod metadata;
mod render;
mod stamp;
mod stamp_text;
mod text;

pub use edit::{merge, rotate, select_pages, split};
pub use metadata::{metadata, DocumentMetadata};
pub use stamp::{stamp_image, StampOptions};
pub use stamp_text::{stamp_text, StandardFont, TextStampOptions};
pub use text::{extract_text, extract_text_all};

pub use render::{
    page_dimensions, parse_color, render_all, render_page, ImageFormat, PageDimensions,
    RenderOptions, DEFAULT_JPEG_QUALITY,
};

use krilla::Document as Authored;
use lopdf::Document as Parsed;

/// What a document reveals without decoding any of its text.
///
/// Metadata strings are deliberately absent: the `/Info` dictionary stores
/// them in either UTF-16BE or PDFDocEncoding, and a half-correct decoder here
/// would silently mangle every accented character in a French document.
#[derive(Debug, Clone)]
pub struct DocumentInfo {
    pub page_count: u32,
    pub version: String,
    pub encrypted: bool,
}

/// Parse `bytes` and report its shape.
pub fn inspect(bytes: &[u8]) -> Result<DocumentInfo, String> {
    let document = Parsed::load_mem(bytes).map_err(|error| format!("cannot read PDF: {error}"))?;

    Ok(DocumentInfo {
        page_count: document.get_pages().len() as u32,
        version: document.version.clone(),
        encrypted: document.trailer.get(b"Encrypt").is_ok(),
    })
}

/// Author a document of blank pages, each sized `(width, height)` in points.
///
/// This exists to prove the authoring path end to end. Drawing — text, images,
/// vector content — arrives with the generation phase.
pub fn create_blank(pages: &[(f32, f32)]) -> Result<Vec<u8>, String> {
    if pages.is_empty() {
        return Err("a PDF needs at least one page".to_string());
    }

    let mut document = Authored::new();
    for &(width, height) in pages {
        let size = Size::from_wh(width, height).ok_or_else(|| {
            format!("page size must be positive and finite, got {width}x{height}")
        })?;
        let page = document.start_page_with(PageSettings::new(size));
        page.finish();
    }

    document
        .finish()
        .map_err(|error| format!("cannot write PDF: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The round trip that proves both crates agree on the same bytes: krilla
    /// writes, lopdf reads back. A mismatch here means the two halves of the
    /// engine are not talking about the same document.
    #[test]
    fn authored_pages_are_read_back() {
        let bytes = create_blank(&[(595.0, 842.0), (595.0, 842.0), (210.0, 297.0)])
            .expect("authoring should succeed");

        let info = inspect(&bytes).expect("the freshly authored bytes should parse");
        assert_eq!(info.page_count, 3);
        assert!(!info.encrypted);
        assert!(
            info.version.starts_with('1') || info.version.starts_with('2'),
            "unexpected PDF version: {}",
            info.version
        );
    }

    #[test]
    fn rejects_a_document_without_pages() {
        assert!(create_blank(&[]).is_err());
    }

    #[test]
    fn rejects_a_page_without_area() {
        assert!(create_blank(&[(0.0, 842.0)]).is_err());
    }

    #[test]
    fn rejects_bytes_that_are_not_a_pdf() {
        assert!(inspect(b"this is not a PDF at all").is_err());
    }
}
