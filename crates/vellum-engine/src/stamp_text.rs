//! Writing text onto the pages of an existing document.
//!
//! The mention "PAID" on an invoice, a file number in a header, a draft
//! marking. Deliberately built on the 14 standard fonts, which a PDF may
//! REFERENCE without embedding (PDF 32000-1 §9.6.2.2): no font file to ship,
//! nothing added to the binary, and hayro already renders them.
//!
//! The cost of that choice, and it is a real one: text is encoded as WinAnsi.
//! French is covered — accents and typographic punctuation both — but Cyrillic
//! and CJK are not, and are refused rather than mangled. Custom fonts are a
//! separate piece of work, through krilla, whenever a caller needs one.

use lopdf::{dictionary, Dictionary, Document, Object, ObjectId};

use crate::edit::flatten_inheritance;
use crate::flatten::isolate_existing_contents;

/// One of the 14 fonts every PDF reader is required to have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandardFont {
    Helvetica,
    HelveticaBold,
    HelveticaOblique,
    TimesRoman,
    TimesBold,
    TimesItalic,
    Courier,
    CourierBold,
}

impl StandardFont {
    /// The `BaseFont` name the reader looks for.
    fn base_font(self) -> &'static str {
        match self {
            Self::Helvetica => "Helvetica",
            Self::HelveticaBold => "Helvetica-Bold",
            Self::HelveticaOblique => "Helvetica-Oblique",
            Self::TimesRoman => "Times-Roman",
            Self::TimesBold => "Times-Bold",
            Self::TimesItalic => "Times-Italic",
            Self::Courier => "Courier",
            Self::CourierBold => "Courier-Bold",
        }
    }

    /// Resolve a font name from a caller.
    pub fn parse(name: &str) -> Result<Self, String> {
        match name.to_ascii_lowercase().replace(['-', ' ', '_'], "").as_str() {
            "helvetica" => Ok(Self::Helvetica),
            "helveticabold" => Ok(Self::HelveticaBold),
            "helveticaoblique" | "helveticaitalic" => Ok(Self::HelveticaOblique),
            "timesroman" | "times" => Ok(Self::TimesRoman),
            "timesbold" => Ok(Self::TimesBold),
            "timesitalic" => Ok(Self::TimesItalic),
            "courier" => Ok(Self::Courier),
            "courierbold" => Ok(Self::CourierBold),
            other => Err(format!(
                "unknown font {other:?} — expected one of the 14 standard fonts, such as \"Helvetica\" or \"Times-Roman\""
            )),
        }
    }
}

/// Where and how a line of text is written onto a page.
#[derive(Debug, Clone, Copy)]
pub struct TextStampOptions {
    /// Which page, addressed from zero. `None` writes on every page.
    pub page: Option<u32>,
    /// Points from the left edge.
    pub x: f32,
    /// Points from the TOP edge, to the text's BASELINE — the line the
    /// letters sit on, not the top of their bounding box.
    pub y: f32,
    pub size: f32,
    pub font: StandardFont,
    pub color: [u8; 3],
    /// 0 is invisible, 1 is opaque.
    pub opacity: f32,
}

impl Default for TextStampOptions {
    fn default() -> Self {
        Self {
            page: None,
            x: 0.0,
            y: 0.0,
            size: 12.0,
            font: StandardFont::Helvetica,
            color: [0, 0, 0],
            opacity: 1.0,
        }
    }
}

/// Encode text as WinAnsi.
///
/// WinAnsi agrees with Latin-1 except over 0x80-0x9F, where Windows put the
/// typographic punctuation a French document actually uses: an em dash is
/// U+2014 but byte 0x97. Anything with no WinAnsi byte is REFUSED rather than
/// replaced — silently dropping a character from a contract is worse than
/// failing.
pub(crate) fn to_win_ansi(text: &str) -> Result<Vec<u8>, String> {
    text.chars()
        .map(|character| match character {
            '\u{20ac}' => Ok(0x80),
            '\u{201a}' => Ok(0x82),
            '\u{201e}' => Ok(0x84),
            '\u{0192}' => Ok(0x83),
            '\u{2026}' => Ok(0x85),
            '\u{2020}' => Ok(0x86),
            '\u{2021}' => Ok(0x87),
            '\u{02c6}' => Ok(0x88),
            '\u{2030}' => Ok(0x89),
            '\u{0160}' => Ok(0x8a),
            '\u{2039}' => Ok(0x8b),
            '\u{0152}' => Ok(0x8c),
            '\u{017d}' => Ok(0x8e),
            '\u{2018}' => Ok(0x91),
            '\u{2019}' => Ok(0x92),
            '\u{201c}' => Ok(0x93),
            '\u{201d}' => Ok(0x94),
            '\u{2022}' => Ok(0x95),
            '\u{2013}' => Ok(0x96),
            '\u{2014}' => Ok(0x97),
            '\u{02dc}' => Ok(0x98),
            '\u{2122}' => Ok(0x99),
            '\u{0161}' => Ok(0x9a),
            '\u{203a}' => Ok(0x9b),
            '\u{0153}' => Ok(0x9c),
            '\u{017e}' => Ok(0x9e),
            '\u{0178}' => Ok(0x9f),
            other => u8::try_from(other as u32).map_err(|_| {
                format!(
                    "{other:?} cannot be written with a standard font — WinAnsi covers Western European text only"
                )
            }),
        })
        .collect()
}

/// Escape a PDF literal string.
///
/// A backslash, an opening or closing parenthesis end the string early
/// otherwise — which does not merely corrupt the output: the remainder of the
/// caller's text would be read as CONTENT STREAM OPERATORS. Escaping here is
/// what stops a document title from injecting drawing commands.
pub(crate) fn escape_pdf_literal(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for byte in bytes {
        if matches!(byte, b'\\' | b'(' | b')') {
            out.push(b'\\');
        }
        out.push(*byte);
    }
    out
}

/// A name that will not collide with what the page already carries.
const FONT_KEY: &str = "VellumStampFont";
const STATE_KEY: &str = "VellumStampState";

/// Ensure the page's `Resources` hold our font and graphics state.
fn register_resources(
    document: &mut Document,
    page_id: ObjectId,
    font_id: ObjectId,
    state_id: Option<ObjectId>,
) -> Result<(), String> {
    // Resources may be an inline dictionary or a reference to one; both are
    // legal, and the page may carry none at all.
    let existing = document
        .get_dictionary(page_id)
        .ok()
        .and_then(|page| page.get(b"Resources").ok().cloned());

    let mut resources = match existing {
        Some(Object::Reference(id)) => document
            .get_dictionary(id)
            .cloned()
            .map_err(|error| format!("cannot read page resources: {error}"))?,
        Some(Object::Dictionary(dictionary)) => dictionary,
        _ => Dictionary::new(),
    };

    let mut fonts = match resources.get(b"Font") {
        Ok(Object::Reference(id)) => document.get_dictionary(*id).cloned().unwrap_or_default(),
        Ok(Object::Dictionary(dictionary)) => dictionary.clone(),
        _ => Dictionary::new(),
    };
    fonts.set(FONT_KEY, Object::Reference(font_id));
    resources.set("Font", Object::Dictionary(fonts));

    if let Some(state_id) = state_id {
        let mut states = match resources.get(b"ExtGState") {
            Ok(Object::Reference(id)) => document.get_dictionary(*id).cloned().unwrap_or_default(),
            Ok(Object::Dictionary(dictionary)) => dictionary.clone(),
            _ => Dictionary::new(),
        };
        states.set(STATE_KEY, Object::Reference(state_id));
        resources.set("ExtGState", Object::Dictionary(states));
    }

    // Written back inline so the page owns them, rather than mutating a
    // resource dictionary that other pages may share.
    let page = document
        .get_object_mut(page_id)
        .and_then(|object| object.as_dict_mut())
        .map_err(|error| format!("cannot update page: {error}"))?;
    page.set("Resources", Object::Dictionary(resources));
    Ok(())
}

/// The height of a page, needed to turn a top-down y into PDF's bottom-up one.
fn page_height(document: &Document, page_id: ObjectId) -> f32 {
    document
        .get_dictionary(page_id)
        .ok()
        .and_then(|page| page.get(b"MediaBox").ok())
        .and_then(|media_box| media_box.as_array().ok())
        .and_then(|values| {
            let bottom = values.get(1)?.as_float().ok()?;
            let top = values.get(3)?.as_float().ok()?;
            Some((top - bottom).abs())
        })
        .unwrap_or(841.89)
}

/// Write `text` onto the document.
pub fn stamp_text(pdf: &[u8], text: &str, options: &TextStampOptions) -> Result<Vec<u8>, String> {
    if text.is_empty() {
        return Err("there is no text to write".to_string());
    }
    if !(0.0..=1.0).contains(&options.opacity) {
        return Err(format!(
            "opacity must be between 0 and 1, got {}",
            options.opacity
        ));
    }
    if !(options.size.is_finite() && options.size > 0.0) {
        return Err(format!("text size must be positive, got {}", options.size));
    }
    if !options.x.is_finite() || !options.y.is_finite() {
        return Err("text position must be finite".to_string());
    }

    let encoded = escape_pdf_literal(&to_win_ansi(text)?);

    let mut document =
        Document::load_mem(pdf).map_err(|error| format!("cannot read PDF: {error}"))?;
    flatten_inheritance(&mut document);

    let pages = document.get_pages();
    let total = pages.len();
    if let Some(page) = options.page {
        if usize::try_from(page).map_or(true, |index| index >= total) {
            return Err(format!(
                "page {} does not exist — the document has {total}",
                page + 1
            ));
        }
    }

    let font_id = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => options.font.base_font(),
        "Encoding" => "WinAnsiEncoding",
    });
    let state_id = if options.opacity < 1.0 {
        Some(document.add_object(dictionary! {
            "Type" => "ExtGState",
            "ca" => f64::from(options.opacity),
            "CA" => f64::from(options.opacity),
        }))
    } else {
        None
    };

    let targets: Vec<(u32, ObjectId)> = pages
        .into_iter()
        .filter(|(number, _)| options.page.is_none_or(|wanted| wanted + 1 == *number))
        .collect();

    let [red, green, blue] = options.color;
    for (_, page_id) in targets {
        // PDF measures y from the BOTTOM; the option measures it from the top.
        let baseline = page_height(&document, page_id) - options.y;
        let state = state_id
            .map(|_| format!("/{STATE_KEY} gs\n"))
            .unwrap_or_default();

        // Assembled as BYTES, not through a String: WinAnsi bytes above 127
        // are not valid UTF-8, and routing them through a Rust string turns
        // every accent into a replacement character.
        //
        // Wrapped in q/Q: without it this inherits whatever graphics state the
        // page's own content stream happened to leave behind — a transform, a
        // clip, a colour — and the text lands somewhere unintended.
        let mut content: Vec<u8> = Vec::new();
        content.extend_from_slice(b"q\n");
        content.extend_from_slice(state.as_bytes());
        content.extend_from_slice(b"BT\n");
        content.extend_from_slice(
            format!(
                "/{FONT_KEY} {} Tf\n{} {} {} rg\n1 0 0 1 {} {} Tm\n",
                options.size,
                f32::from(red) / 255.0,
                f32::from(green) / 255.0,
                f32::from(blue) / 255.0,
                options.x,
                baseline,
            )
            .as_bytes(),
        );
        content.push(b'(');
        content.extend_from_slice(&encoded);
        content.extend_from_slice(b") Tj\nET\nQ\n");

        register_resources(&mut document, page_id, font_id, state_id)?;
        // The page is free to leave the graphics state transformed, so its own
        // content is balanced before ours is appended after it.
        isolate_existing_contents(&mut document, page_id)?;
        document
            .add_page_contents(page_id, content)
            .map_err(|error| format!("cannot write onto the page: {error}"))?;
    }

    let mut out = Vec::new();
    document
        .save_to(&mut out)
        .map_err(|error| format!("cannot write PDF: {error}"))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use hayro::vello_cpu::Pixmap;

    use super::*;
    use crate::{create_blank, extract_text, inspect, render_page, ImageFormat, RenderOptions};

    const A4: (f32, f32) = (595.28, 841.89);

    fn blank() -> Vec<u8> {
        create_blank(&[A4]).unwrap()
    }

    /// True when the rendered page has any pixel darker than white.
    fn has_ink(pdf: &[u8], page: u32) -> bool {
        let png = render_page(
            pdf,
            page,
            &RenderOptions {
                format: ImageFormat::Png,
                ..Default::default()
            },
        )
        .expect("rendering should succeed");
        let pixmap = Pixmap::from_png(Cursor::new(png)).expect("the render decodes");
        pixmap.data().iter().any(|pixel| pixel.r < 200)
    }

    /// A page may leave the graphics state transformed — a `cm` outside any
    /// `q`/`Q` pair is legal and never restored — and a stamp appended after
    /// it would inherit that transform. Here the page doubles everything, so
    /// an unbalanced stamp would land at twice the distance and fall off.
    #[test]
    fn a_page_that_left_its_transform_open_does_not_move_the_stamp() {
        let mut document = Document::load_mem(&blank()).expect("the blank parses");
        let page_id = *document
            .get_pages()
            .values()
            .next()
            .expect("the document has a page");
        document
            .add_page_contents(page_id, b"2 0 0 2 0 0 cm\n".to_vec())
            .expect("the fixture takes contents");
        let mut source = Vec::new();
        document.save_to(&mut source).expect("the fixture saves");

        let stamped = stamp_text(
            &source,
            "PAID",
            &TextStampOptions {
                x: 400.0,
                y: 700.0,
                size: 24.0,
                ..Default::default()
            },
        )
        .expect("stamping should succeed");

        // Doubled, the baseline would sit at x 800 on a 595pt-wide page.
        assert!(has_ink(&stamped, 0), "the stamp belongs on the page");
    }

    /// The strongest check available: write text, then read it back with our
    /// own extractor. It exercises the font, the encoding and the content
    /// stream in one go.
    #[test]
    fn writes_text_that_can_be_read_back() {
        let stamped = stamp_text(
            &blank(),
            "Mandat de gestion",
            &TextStampOptions {
                x: 60.0,
                y: 100.0,
                size: 18.0,
                ..Default::default()
            },
        )
        .expect("stamping should succeed");

        assert_eq!(extract_text(&stamped, 0).unwrap(), "Mandat de gestion");
        assert!(has_ink(&stamped, 0), "the text should actually be painted");
    }

    /// The reason WinAnsi is mapped explicitly rather than truncated to
    /// Latin-1: French uses both accents and typographic punctuation.
    /// WinAnsi defines every code between 0x80 and 0x9F but five, and the
    /// encoder has to reach all of them: a trademark sign or a low quotation
    /// mark is ordinary text, not something to refuse.
    #[test]
    fn encodes_the_whole_of_the_windows_block() {
        let expected = [
            ('\u{20ac}', 0x80),
            ('\u{201a}', 0x82),
            ('\u{0192}', 0x83),
            ('\u{201e}', 0x84),
            ('\u{2026}', 0x85),
            ('\u{2020}', 0x86),
            ('\u{2021}', 0x87),
            ('\u{02c6}', 0x88),
            ('\u{2030}', 0x89),
            ('\u{0160}', 0x8a),
            ('\u{2039}', 0x8b),
            ('\u{0152}', 0x8c),
            ('\u{017d}', 0x8e),
            ('\u{2018}', 0x91),
            ('\u{2019}', 0x92),
            ('\u{201c}', 0x93),
            ('\u{201d}', 0x94),
            ('\u{2022}', 0x95),
            ('\u{2013}', 0x96),
            ('\u{2014}', 0x97),
            ('\u{02dc}', 0x98),
            ('\u{2122}', 0x99),
            ('\u{0161}', 0x9a),
            ('\u{203a}', 0x9b),
            ('\u{0153}', 0x9c),
            ('\u{017e}', 0x9e),
            ('\u{0178}', 0x9f),
        ];
        for (character, code) in expected {
            assert_eq!(
                to_win_ansi(&character.to_string()).map(|bytes| bytes[0]),
                Ok(code),
                "{character:?}"
            );
        }
    }

    #[test]
    fn writes_accented_french() {
        let text = "Prévoyance — décès à 65 ans (n°42)";
        let stamped = stamp_text(
            &blank(),
            text,
            &TextStampOptions {
                x: 40.0,
                y: 100.0,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(extract_text(&stamped, 0).unwrap(), text);
    }

    /// Parentheses and backslashes end a PDF literal string early. Unescaped,
    /// the rest of the caller's text would be read as CONTENT STREAM
    /// OPERATORS — a document title able to issue drawing commands.
    #[test]
    fn text_cannot_inject_content_stream_operators() {
        let hostile = r"Total (net) \ ET Q 1 0 0 rg 0 0 600 800 re f";

        let stamped = stamp_text(
            &blank(),
            hostile,
            &TextStampOptions {
                x: 20.0,
                y: 100.0,
                size: 9.0,
                ..Default::default()
            },
        )
        .expect("hostile text should still be written, as text");

        // It comes back as the literal it was, not as instructions.
        assert_eq!(extract_text(&stamped, 0).unwrap(), hostile);
        // And the page was not flooded by the injected fill.
        let png = render_page(&stamped, 0, &RenderOptions::default()).unwrap();
        let pixmap = Pixmap::from_png(Cursor::new(png)).unwrap();
        let corner = pixmap.data()[pixmap.data().len() - 1];
        assert!(
            corner.r > 240 && corner.g > 240,
            "the bottom-right corner should still be blank: {corner:?}"
        );
    }

    #[test]
    fn writes_on_every_page_by_default() {
        let stamped = stamp_text(
            &create_blank(&[A4, A4]).unwrap(),
            "BROUILLON",
            &TextStampOptions {
                x: 50.0,
                y: 120.0,
                ..Default::default()
            },
        )
        .unwrap();

        for page in 0..2 {
            assert_eq!(extract_text(&stamped, page).unwrap(), "BROUILLON");
        }
    }

    #[test]
    fn writes_only_on_the_page_asked_for() {
        let stamped = stamp_text(
            &create_blank(&[A4, A4]).unwrap(),
            "COPIE",
            &TextStampOptions {
                page: Some(1),
                x: 50.0,
                y: 120.0,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(extract_text(&stamped, 0).unwrap(), "");
        assert_eq!(extract_text(&stamped, 1).unwrap(), "COPIE");
    }

    /// Adding a stream must not erase what the page already carries.
    #[test]
    fn keeps_the_existing_content() {
        let first = stamp_text(
            &blank(),
            "Premier",
            &TextStampOptions {
                x: 40.0,
                y: 100.0,
                ..Default::default()
            },
        )
        .unwrap();
        let second = stamp_text(
            &first,
            "Second",
            &TextStampOptions {
                x: 40.0,
                y: 140.0,
                ..Default::default()
            },
        )
        .unwrap();

        let text = extract_text(&second, 0).unwrap();
        assert!(text.contains("Premier"), "got: {text}");
        assert!(text.contains("Second"), "got: {text}");
        assert_eq!(inspect(&second).unwrap().page_count, 1);
    }

    #[test]
    fn a_translucent_stamp_is_lighter_than_an_opaque_one() {
        let options = TextStampOptions {
            x: 40.0,
            y: 100.0,
            size: 40.0,
            ..Default::default()
        };
        let opaque = stamp_text(&blank(), "PAYE", &options).unwrap();
        let faint = stamp_text(
            &blank(),
            "PAYE",
            &TextStampOptions {
                opacity: 0.15,
                ..options
            },
        )
        .unwrap();

        let darkest = |pdf: &[u8]| -> u8 {
            let png = render_page(pdf, 0, &RenderOptions::default()).unwrap();
            let pixmap = Pixmap::from_png(Cursor::new(png)).unwrap();
            pixmap
                .data()
                .iter()
                .map(|pixel| pixel.r)
                .min()
                .unwrap_or(255)
        };

        assert!(
            darkest(&faint) > darkest(&opaque),
            "a 15% stamp should be lighter than an opaque one"
        );
    }

    #[test]
    fn parses_font_names() {
        assert_eq!(
            StandardFont::parse("Helvetica"),
            Ok(StandardFont::Helvetica)
        );
        assert_eq!(
            StandardFont::parse("helvetica-bold"),
            Ok(StandardFont::HelveticaBold)
        );
        assert_eq!(StandardFont::parse("times"), Ok(StandardFont::TimesRoman));
        assert!(StandardFont::parse("Comic Sans").is_err());
    }

    /// Refused, not replaced: silently dropping a character from a contract is
    /// worse than failing.
    #[test]
    fn refuses_text_a_standard_font_cannot_carry() {
        let error = stamp_text(&blank(), "договор", &TextStampOptions::default())
            .expect_err("Cyrillic has no WinAnsi byte");

        assert!(error.contains("WinAnsi"), "got: {error}");
    }

    #[test]
    fn refuses_nonsense_options() {
        assert!(stamp_text(&blank(), "", &TextStampOptions::default()).is_err());
        for opacity in [-0.5, 2.0] {
            let options = TextStampOptions {
                opacity,
                ..Default::default()
            };
            assert!(stamp_text(&blank(), "x", &options).is_err());
        }
        for size in [0.0, -12.0, f32::NAN] {
            let options = TextStampOptions {
                size,
                ..Default::default()
            };
            assert!(stamp_text(&blank(), "x", &options).is_err());
        }
    }

    #[test]
    fn refuses_a_page_that_does_not_exist() {
        let options = TextStampOptions {
            page: Some(4),
            ..Default::default()
        };
        let error = stamp_text(&create_blank(&[A4, A4]).unwrap(), "x", &options)
            .expect_err("page index 4 should not exist");

        assert!(error.contains("page 5 does not exist"), "got: {error}");
    }

    #[test]
    fn refuses_bytes_that_are_not_a_pdf() {
        assert!(stamp_text(b"not a PDF", "x", &TextStampOptions::default()).is_err());
    }
}
