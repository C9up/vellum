//! Where an edited document is written.
//!
//! Every edit and every signature re-writes the objects of the document it
//! read, strings and all, so what writing costs is the document's to choose
//! as much as what reading it costs. lopdf's writer is linear in what it
//! writes (its literal-string escaping and its cross-reference walk are
//! patched to be, see `vendor/lopdf`), and what one call may write is bounded
//! here: past the ceiling the write stops and the edit is refused, instead of
//! growing a buffer until the process is out of memory.

use std::collections::BTreeMap;
use std::io::{self, Write};

use lopdf::{Document, IncrementalDocument, ObjectId};

/// The most one document written by an edit or a signature may weigh: 1 GiB,
/// as much as a split may copy into all its parts. Parsing bounds what a
/// document may hold, and escaping at most doubles a string, so a document
/// read within the parse budget writes well under it; the ceiling is the
/// writer's own bound, whatever it is handed.
pub const MAX_WRITTEN_BYTES: usize = 1024 * 1024 * 1024;

/// A buffer that refuses to grow past its ceiling.
struct Bounded {
    out: Vec<u8>,
    left: usize,
    ceiling: usize,
}

impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.left {
            self.left = 0;
            return Err(io::Error::other(format!(
                "the document written would be larger than {} MiB",
                self.ceiling / (1024 * 1024)
            )));
        }
        self.left -= bytes.len();
        self.out.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn bounded(ceiling: usize) -> Bounded {
    Bounded {
        out: Vec::new(),
        left: ceiling,
        ceiling,
    }
}

/// Write `document` whole.
pub(crate) fn save(document: &mut Document) -> Result<Vec<u8>, String> {
    save_within(document, MAX_WRITTEN_BYTES)
}

fn save_within(document: &mut Document, ceiling: usize) -> Result<Vec<u8>, String> {
    let mut out = bounded(ceiling);
    document
        .save_to(&mut out)
        .map_err(|error| format!("cannot write PDF: {error}"))?;
    Ok(out.out)
}

/// Write `incremental` (the previous revision's bytes, then the new one),
/// and say where each object of the new revision starts.
pub(crate) fn save_incremental(
    incremental: &mut IncrementalDocument,
) -> Result<(Vec<u8>, BTreeMap<ObjectId, usize>), String> {
    let mut out = bounded(MAX_WRITTEN_BYTES);
    let offsets = incremental
        .save_to_with_offsets(&mut out)
        .map_err(|error| format!("cannot write PDF: {error}"))?;
    Ok((out.out, offsets))
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use lopdf::{dictionary, Object, StringFormat};

    use super::*;

    fn with_string(text: Vec<u8>) -> Document {
        let mut document = Document::with_version("1.7");
        let id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "X" => Object::String(text, StringFormat::Literal),
        });
        document.trailer.set("Root", id);
        document
    }

    /// Escaping is one pass: a mebibyte of backslashes, every one of which
    /// is escaped, is written at once. Escaping them used to cost the
    /// length squared — an hour for this string.
    #[test]
    fn a_string_of_backslashes_is_written_in_linear_time() {
        let mut document = with_string(vec![b'\\'; 1024 * 1024]);
        let started = Instant::now();
        let written = save(&mut document).expect("written");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "took {:?}",
            started.elapsed()
        );
        assert!(written
            .windows(2 * 1024 * 1024)
            .any(|window| window.iter().all(|&byte| byte == b'\\')));
    }

    /// The escaping itself: a backslash, a carriage return and an unbalanced
    /// parenthesis are escaped, a balanced pair is not, and the string reads
    /// back as it was.
    #[test]
    fn a_literal_string_is_escaped_as_the_syntax_requires() {
        let text = b"a\\b\rc(d)e)f(g".to_vec();
        let mut document = with_string(text.clone());
        let written = save(&mut document).expect("written");
        let needle = b"(a\\\\b\\rc(d)e\\)f\\(g)";
        assert!(
            written.windows(needle.len()).any(|window| window == needle),
            "{}",
            String::from_utf8_lossy(&written)
        );
        let read = Document::load_mem(&written).expect("it reads back");
        let catalog = read.catalog().expect("a catalog");
        assert_eq!(
            catalog
                .get(b"X")
                .and_then(Object::as_str)
                .expect("the string"),
            text
        );
    }

    /// A document naming one object four billion: writing it walks the
    /// objects there are, not every number below the largest, in a
    /// cross-reference table and in a cross-reference stream alike.
    #[test]
    fn a_huge_object_number_does_not_make_writing_count_to_it() {
        for kind in [
            lopdf::xref::XrefType::CrossReferenceTable,
            lopdf::xref::XrefType::CrossReferenceStream,
        ] {
            let mut document = with_string(b"x".to_vec());
            document.reference_table.cross_reference_type = kind;
            let far = (4_000_000_000, 0);
            document.objects.insert(far, Object::Integer(1));
            document.max_id = far.0;
            let started = Instant::now();
            let written = save(&mut document).expect("written");
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "{kind:?} took {:?}",
                started.elapsed()
            );
            let read = Document::load_mem(&written).expect("it reads back");
            assert_eq!(
                read.get_object(far).and_then(Object::as_i64).ok(),
                Some(1),
                "{kind:?}"
            );
            assert!(read.catalog().is_ok(), "{kind:?}");
        }
    }

    /// Past its ceiling the write stops and says so.
    #[test]
    fn a_document_larger_than_the_ceiling_is_refused() {
        let mut document = with_string(vec![b'a'; 4096]);
        let error = save_within(&mut document, 1024).expect_err("refused");
        assert!(error.contains("would be larger than"), "got {error}");
        assert!(save_within(&mut document, 1024 * 1024).is_ok());
    }
}
