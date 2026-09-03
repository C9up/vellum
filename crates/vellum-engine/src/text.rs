//! Extracting the text of a page.
//!
//! Goes through hayro's interpreter rather than a dedicated crate. The
//! interpreter already resolves fonts, encodings and `/ToUnicode` maps, and it
//! is already a dependency — `pdf-extract` would have been the obvious choice
//! but it pins `lopdf ^0.42` against our 0.44, which would put two copies of
//! the parser in the binary.
//!
//! A `Device` normally rasterises what the interpreter walks over. This one
//! discards every drawing operation and keeps only the glyphs' text.

use hayro::hayro_interpret::font::Glyph;
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret::hayro_syntax::page::Page;
use hayro::hayro_interpret::hayro_syntax::Pdf;
use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_interpret::{
    interpret_page, BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache,
    InterpreterSettings, Paint, PathDrawMode, SoftMask,
};
use hayro::vello_cpu::kurbo::{Affine, BezPath, Rect};

/// Vertical distance, in points, beyond which two glyphs are taken to be on
/// different lines.
///
/// Glyphs on one line share a baseline to within rounding, while a line break
/// moves by the leading — typically 10pt or more. Three points sits between
/// the two. A superscript can still trip it, which is the known cost of not
/// tracking font size.
const LINE_TOLERANCE: f64 = 3.0;

/// Collects glyph text, discarding everything that would be painted.
#[derive(Default)]
struct TextCollector {
    /// Baseline y and text, in the order the content stream emits them.
    glyphs: Vec<(f64, String)>,
}

impl TextCollector {
    /// Join the glyphs into text.
    ///
    /// Kept in content-stream order rather than sorted by position: that order
    /// is the reading order in practice, and reordering by coordinates needs
    /// column detection to not make multi-column pages worse. No spaces are
    /// invented either — a PDF encodes its own, and guessing from gaps
    /// duplicates them.
    fn into_text(self) -> String {
        let mut out = String::new();
        let mut previous_y: Option<f64> = None;

        for (y, text) in self.glyphs {
            if previous_y.is_some_and(|previous| (y - previous).abs() > LINE_TOLERANCE) {
                out.push('\n');
            }
            out.push_str(&text);
            previous_y = Some(y);
        }

        out.trim().to_string()
    }
}

impl<'a> Device<'a> for TextCollector {
    fn draw_glyph(
        &mut self,
        glyph: &Glyph<'a>,
        transform: Affine,
        glyph_transform: Affine,
        _paint: &Paint<'a>,
        _draw_mode: &GlyphDrawMode,
    ) {
        // A glyph with no unicode mapping cannot be turned into text: the font
        // has neither a /ToUnicode map nor a recognisable encoding. Skipped
        // rather than guessed at.
        let Some(text) = glyph.as_unicode() else {
            return;
        };

        // The position is the COMPOSITION of the two transforms — hayro's own
        // renderer fills a glyph at `transform * glyph_transform`. Reading
        // `transform` alone gives the text-object origin, identical for every
        // line, so every baseline change went undetected.
        self.glyphs.push((
            (transform * glyph_transform).translation().y,
            match text {
                BfString::Char(character) => character.to_string(),
                BfString::String(string) => string,
            },
        ));
    }

    fn set_soft_mask(&mut self, _mask: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _blend_mode: BlendMode) {}
    fn draw_path(
        &mut self,
        _path: &BezPath,
        _transform: Affine,
        _paint: &Paint<'a>,
        _draw_mode: &PathDrawMode,
    ) {
    }
    fn push_clip_path(&mut self, _clip_path: &ClipPath) {}
    fn push_transparency_group(
        &mut self,
        _opacity: f32,
        _mask: Option<SoftMask<'a>>,
        _blend_mode: BlendMode,
    ) {
    }
    fn draw_image(&mut self, _image: Image<'a, '_>, _transform: Affine) {}
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

fn page_text(page: &Page<'_>) -> String {
    let (width, height) = page.render_dimensions();
    let cache = InterpreterCache::new();
    // `initial_transform(true)` flips the y axis, so y grows downwards as it
    // does on screen — which makes "further down the page" mean "later", and
    // the line-break check above work in reading order.
    let mut context = Context::new(
        page.initial_transform(true).to_kurbo(),
        Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
        &cache,
        page.xref(),
        InterpreterSettings::default(),
    );

    let mut collector = TextCollector::default();
    interpret_page(page, &mut context, &mut collector);
    collector.into_text()
}

fn load(bytes: &[u8]) -> Result<Pdf, String> {
    Pdf::new(bytes.to_vec()).map_err(|error| format!("cannot read PDF: {error:?}"))
}

/// The text of one page, addressed from zero.
pub fn extract_text(bytes: &[u8], page_index: u32) -> Result<String, String> {
    let pdf = load(bytes)?;
    let pages = pdf.pages();
    let index = usize::try_from(page_index).map_err(|_| "page index out of range".to_string())?;
    let page = pages.get(index).ok_or_else(|| {
        format!(
            "page {} does not exist — the document has {}",
            page_index,
            pages.len()
        )
    })?;

    Ok(page_text(page))
}

/// The text of every page, in document order.
pub fn extract_text_all(bytes: &[u8]) -> Result<Vec<String>, String> {
    let pdf = load(bytes)?;
    Ok(pdf.pages().iter().map(page_text).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::content::{Content, Operation};
    use lopdf::{dictionary, Document, Object, Stream};

    /// Encode text as WinAnsi.
    ///
    /// WinAnsi agrees with Latin-1 EXCEPT over 0x80-0x9F, where it places the
    /// typographic punctuation a French document actually uses — an em dash is
    /// U+2014 but byte 0x97. Mapping those explicitly is what lets the fixture
    /// carry real punctuation instead of only plain accents.
    fn win_ansi(text: &str) -> Vec<u8> {
        text.chars()
            .map(|character| match character {
                '\u{20ac}' => 0x80,
                '\u{2018}' => 0x91,
                '\u{2019}' => 0x92,
                '\u{201c}' => 0x93,
                '\u{201d}' => 0x94,
                '\u{2013}' => 0x96,
                '\u{2014}' => 0x97,
                other => u8::try_from(other as u32)
                    .unwrap_or_else(|_| panic!("{other:?} has no WinAnsi byte in this fixture")),
            })
            .collect()
    }

    /// Build a one-page document showing `lines`, each at its own baseline.
    ///
    /// Written by hand rather than with krilla because krilla needs a font
    /// file, while Helvetica is one of the 14 standard fonts hayro embeds —
    /// so the fixture is deterministic and depends on nothing installed.
    fn text_document(lines: &[(&str, i64)]) -> Vec<u8> {
        let mut document = Document::with_version("1.7");

        let font_id = document.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
            "Encoding" => "WinAnsiEncoding",
        });

        let mut operations = Vec::new();
        for (text, baseline) in lines {
            operations.push(Operation::new("BT", vec![]));
            operations.push(Operation::new(
                "Tf",
                vec![Object::Name(b"F1".to_vec()), 12.into()],
            ));
            operations.push(Operation::new("Td", vec![50.into(), (*baseline).into()]));
            operations.push(Operation::new(
                "Tj",
                vec![Object::string_literal(win_ansi(text))],
            ));
            operations.push(Operation::new("ET", vec![]));
        }

        let content = Content { operations }.encode().expect("content encodes");
        let contents_id = document.add_object(Stream::new(dictionary! {}, content));

        let pages_id = document.new_object_id();
        let page_id = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => contents_id,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
            "Resources" => dictionary! {
                "Font" => dictionary! { "F1" => font_id },
            },
        });
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );
        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        document.trailer.set("Root", catalog_id);

        let mut out = Vec::new();
        document.save_to(&mut out).expect("document saves");
        out
    }

    #[test]
    fn extracts_plain_text() {
        let pdf = text_document(&[("Mandat de gestion", 700)]);

        assert_eq!(extract_text(&pdf, 0).unwrap(), "Mandat de gestion");
    }

    /// The case fluveo actually has: accented French. The glyph's unicode
    /// comes from the font's encoding, so a wrong mapping shows up here.
    #[test]
    fn extracts_accented_french() {
        let pdf = text_document(&[("Prevoyance: resiliation a echeance", 700)]);
        assert_eq!(
            extract_text(&pdf, 0).unwrap(),
            "Prevoyance: resiliation a echeance"
        );

        let accented = text_document(&[("Prévoyance — décès à 65 ans", 700)]);
        assert_eq!(
            extract_text(&accented, 0).unwrap(),
            "Prévoyance — décès à 65 ans"
        );
    }

    #[test]
    fn separates_baselines_with_a_line_break() {
        let pdf = text_document(&[("Première ligne", 700), ("Deuxième ligne", 680)]);

        assert_eq!(
            extract_text(&pdf, 0).unwrap(),
            "Première ligne\nDeuxième ligne"
        );
    }

    /// Two runs on the SAME baseline are one line — a PDF splits a line into
    /// several show operations all the time, and breaking on each would
    /// shred every paragraph.
    #[test]
    fn keeps_one_baseline_on_one_line() {
        let pdf = text_document(&[("Contrat ", 700), ("n°42", 700)]);

        assert_eq!(extract_text(&pdf, 0).unwrap(), "Contrat n°42");
    }

    #[test]
    fn returns_empty_text_for_a_page_with_no_text() {
        let pdf = crate::create_blank(&[(595.28, 841.89)]).unwrap();

        assert_eq!(extract_text(&pdf, 0).unwrap(), "");
    }

    #[test]
    fn extracts_every_page() {
        let pdf = text_document(&[("Une seule page", 700)]);

        let pages = extract_text_all(&pdf).unwrap();
        assert_eq!(pages, vec!["Une seule page".to_string()]);
    }

    #[test]
    fn refuses_a_page_that_does_not_exist() {
        let pdf = text_document(&[("Page unique", 700)]);

        let error = extract_text(&pdf, 5).expect_err("page 5 should not exist");
        assert!(error.contains("does not exist"), "got: {error}");
    }

    #[test]
    fn refuses_bytes_that_are_not_a_pdf() {
        assert!(extract_text(b"not a PDF", 0).is_err());
    }
}
