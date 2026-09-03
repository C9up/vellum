//! Reading a document's `/Info` dictionary.
//!
//! The text strings are decoded by lopdf's `decode_text_string`, which handles
//! all three encodings PDF 2.0 allows (UTF-16BE, UTF-8, PDFDocEncoding with
//! the real table). Writing that by hand was the reason metadata was left out
//! of the first pass — on a French document, an approximation eats the accents.

use lopdf::Document;

/// What the `/Info` dictionary says about a document.
///
/// Every field is optional because every field genuinely is: a PDF is valid
/// with no `/Info` at all, and producers fill in whichever ones they like.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocumentMetadata {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    /// The application that authored the content.
    pub creator: Option<String>,
    /// The application that wrote the PDF.
    pub producer: Option<String>,
    /// ISO 8601 when the producer wrote a conforming date, otherwise the raw
    /// string it did write — reporting nothing would discard information we
    /// were handed.
    pub created_at: Option<String>,
    pub modified_at: Option<String>,
}

pub fn metadata(bytes: &[u8]) -> Result<DocumentMetadata, String> {
    // Reads the trailer and the Info dictionary rather than parsing every
    // object, which is why it is a separate call from `inspect`.
    let info = Document::load_metadata_mem(bytes)
        .map_err(|error| format!("cannot read PDF metadata: {error}"))?;

    Ok(DocumentMetadata {
        title: clean(info.title),
        author: clean(info.author),
        subject: clean(info.subject),
        keywords: clean(info.keywords),
        creator: clean(info.creator),
        producer: clean(info.producer),
        created_at: clean(info.creation_date).map(|date| pdf_date_to_iso(&date).unwrap_or(date)),
        modified_at: clean(info.modification_date)
            .map(|date| pdf_date_to_iso(&date).unwrap_or(date)),
    })
}

/// Drop empty strings, and the byte-order mark lopdf leaves in place when it
/// decodes a UTF-8 BOM string: it decodes from the BOM onwards rather than
/// past it, so the value arrives with a leading U+FEFF.
fn clean(value: Option<String>) -> Option<String> {
    let value = value?;
    let trimmed = value.trim_start_matches('\u{feff}').trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Convert a PDF date to ISO 8601.
///
/// PDF 32000-1 §7.9.4: `D:YYYYMMDDHHmmSSOHH'mm'`, where everything after the
/// year is optional and `O` is `+`, `-` or `Z`. Returns `None` for anything
/// that does not parse, so the caller can fall back to the raw string instead
/// of publishing a date it invented.
fn pdf_date_to_iso(value: &str) -> Option<String> {
    let rest = value.strip_prefix("D:").unwrap_or(value);
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.len() < 4 {
        return None;
    }

    let field = |at: usize, fallback: &str| -> String {
        digits
            .get(at..at + 2)
            .map(str::to_string)
            .unwrap_or_else(|| fallback.to_string())
    };

    let year = &digits[0..4];
    let month = field(4, "01");
    let day = field(6, "01");
    let hour = field(8, "00");
    let minute = field(10, "00");
    let second = field(12, "00");

    let offset = match rest[digits.len()..].chars().next() {
        Some('Z') => "Z".to_string(),
        Some(sign @ ('+' | '-')) => {
            // The zone is written HH'mm' — apostrophes and the minutes are
            // both optional in the wild.
            let zone: String = rest[digits.len() + 1..]
                .chars()
                .filter(char::is_ascii_digit)
                .take(4)
                .collect();
            let zone_hour = zone.get(0..2).unwrap_or("00");
            let zone_minute = zone.get(2..4).unwrap_or("00");
            format!("{sign}{zone_hour}:{zone_minute}")
        }
        // No zone means local time in an unknown zone, so none is emitted
        // rather than pretending it is UTC.
        _ => String::new(),
    };

    Some(format!(
        "{year}-{month}-{day}T{hour}:{minute}:{second}{offset}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_acrobat_date() {
        assert_eq!(
            pdf_date_to_iso("D:20260903154500+02'00'").as_deref(),
            Some("2026-09-03T15:45:00+02:00")
        );
    }

    #[test]
    fn parses_utc_and_missing_zones() {
        assert_eq!(
            pdf_date_to_iso("D:20260903154500Z").as_deref(),
            Some("2026-09-03T15:45:00Z")
        );
        // No zone: local time in an unknown offset. Emitting nothing is
        // honest; appending "Z" would claim UTC we were never told.
        assert_eq!(
            pdf_date_to_iso("D:20260903154500").as_deref(),
            Some("2026-09-03T15:45:00")
        );
    }

    #[test]
    fn fills_in_optional_fields() {
        // Everything after the year is optional per the spec.
        assert_eq!(
            pdf_date_to_iso("D:2026").as_deref(),
            Some("2026-01-01T00:00:00")
        );
        assert_eq!(
            pdf_date_to_iso("D:202609").as_deref(),
            Some("2026-09-01T00:00:00")
        );
    }

    #[test]
    fn tolerates_a_zone_without_apostrophes_or_minutes() {
        assert_eq!(
            pdf_date_to_iso("D:20260903154500-0500").as_deref(),
            Some("2026-09-03T15:45:00-05:00")
        );
        assert_eq!(
            pdf_date_to_iso("D:20260903154500+02").as_deref(),
            Some("2026-09-03T15:45:00+02:00")
        );
    }

    #[test]
    fn accepts_a_date_without_the_prefix() {
        // Some producers omit "D:" even though the spec asks for it.
        assert_eq!(
            pdf_date_to_iso("20260903154500Z").as_deref(),
            Some("2026-09-03T15:45:00Z")
        );
    }

    #[test]
    fn refuses_what_is_not_a_date() {
        for value in ["", "D:", "not a date", "D:202"] {
            assert_eq!(pdf_date_to_iso(value), None, "{value:?} should not parse");
        }
    }

    #[test]
    fn strips_a_leading_byte_order_mark() {
        // lopdf decodes a UTF-8 BOM string from the BOM onwards, so the value
        // arrives with U+FEFF still attached.
        assert_eq!(
            clean(Some("\u{feff}Contrat".to_string())).as_deref(),
            Some("Contrat")
        );
        assert_eq!(clean(Some("   ".to_string())), None);
        assert_eq!(clean(None), None);
    }

    #[test]
    fn reads_metadata_from_a_document_without_an_info_dictionary() {
        // krilla writes no /Info, and that is valid. Absent must not be an
        // error, or every generated document would fail to introspect.
        let pdf = crate::create_blank(&[(595.28, 841.89)]).unwrap();

        let info = metadata(&pdf).expect("a document with no /Info should still read");
        assert_eq!(info, DocumentMetadata::default());
    }

    /// Inject an `/Info` dictionary into a document so the decoding path can
    /// be exercised end to end. krilla writes none, so without this the
    /// accented-text case — the whole reason this module waited for a real
    /// decoder — would never be covered.
    fn with_info(entries: &[(&[u8], lopdf::Object)]) -> Vec<u8> {
        let pdf = crate::create_blank(&[(595.28, 841.89)]).unwrap();
        let mut document = Document::load_mem(&pdf).unwrap();

        let mut info = lopdf::Dictionary::new();
        for (key, value) in entries {
            info.set(key.to_vec(), value.clone());
        }
        let id = document.add_object(lopdf::Object::Dictionary(info));
        document.trailer.set("Info", lopdf::Object::Reference(id));

        let mut out = Vec::new();
        document.save_to(&mut out).unwrap();
        out
    }

    #[test]
    fn reads_a_plain_ascii_title() {
        let pdf = with_info(&[
            (b"Title", lopdf::text_string("Mandat de gestion")),
            (b"Author", lopdf::text_string("fluveo")),
        ]);

        let info = metadata(&pdf).expect("metadata should read");
        assert_eq!(info.title.as_deref(), Some("Mandat de gestion"));
        assert_eq!(info.author.as_deref(), Some("fluveo"));
        assert_eq!(info.subject, None);
    }

    /// The case the whole module was waiting for: accented French text. A
    /// PDFDocEncoding approximation would mangle every one of these.
    #[test]
    fn reads_accented_french_text() {
        let title = "Prévoyance — Contrat n°42 (à jour)";
        let pdf = with_info(&[
            (b"Title", lopdf::text_string(title)),
            (b"Subject", lopdf::text_string("Résiliation à échéance")),
            (
                b"Keywords",
                lopdf::text_string("prévoyance, décès, invalidité"),
            ),
        ]);

        let info = metadata(&pdf).expect("metadata should read");
        assert_eq!(info.title.as_deref(), Some(title));
        assert_eq!(info.subject.as_deref(), Some("Résiliation à échéance"));
        assert_eq!(
            info.keywords.as_deref(),
            Some("prévoyance, décès, invalidité")
        );
    }

    #[test]
    fn converts_the_dates_it_reads_to_iso() {
        let pdf = with_info(&[(
            b"CreationDate",
            lopdf::Object::string_literal("D:20260903154500+02'00'"),
        )]);

        let info = metadata(&pdf).expect("metadata should read");
        assert_eq!(
            info.created_at.as_deref(),
            Some("2026-09-03T15:45:00+02:00")
        );
    }

    #[test]
    fn keeps_a_date_it_cannot_parse_rather_than_dropping_it() {
        let pdf = with_info(&[(
            b"CreationDate",
            lopdf::Object::string_literal("sometime last Tuesday"),
        )]);

        let info = metadata(&pdf).expect("metadata should read");
        assert_eq!(info.created_at.as_deref(), Some("sometime last Tuesday"));
    }

    #[test]
    fn refuses_bytes_that_are_not_a_pdf() {
        assert!(metadata(b"not a PDF").is_err());
    }
}
