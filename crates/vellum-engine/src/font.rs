//! Embedding a font a caller supplies.
//!
//! The 14 standard fonts are referenced without being embedded, which is why
//! they cost nothing — and why they are limited to WinAnsi. Anything else has
//! to be carried inside the document, and a PDF only accepts that as a
//! composite font: a `Type0` in `Identity-H` over a `CIDFontType2` descendant,
//! whose glyphs the content stream addresses by two-byte identifier rather
//! than by character code.
//!
//! Two consequences follow, and they are the reason this module exists rather
//! than a flag on the standard-font path:
//!
//! - The font is **subsetted** to the glyphs actually used. Embedding a family
//!   whole would put megabytes into every stamped document.
//! - The subset has no character map of its own, so a `/ToUnicode` table is
//!   written alongside it. Without one the text is drawn correctly and cannot
//!   be selected, copied or searched — a silent loss that only shows up when
//!   someone tries to read the document back.

use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
use skrifa::instance::{LocationRef, Size};
use skrifa::metrics::GlyphMetrics;
use skrifa::string::StringId;
use skrifa::{FontRef, MetadataProvider};
use subsetter::GlyphRemapper;

/// A run of text, ready to be written with an embedded font.
#[derive(Debug)]
pub(crate) struct Embedded {
    /// The font object, to name in the page's resources.
    pub font_id: ObjectId,
    /// The glyphs to draw, as identifiers in the embedded subset.
    pub glyphs: Vec<u16>,
}

impl Embedded {
    /// The glyphs as the hex string a content stream shows them with.
    pub fn as_hex(&self) -> String {
        let mut out = String::with_capacity(self.glyphs.len() * 4 + 2);
        out.push('<');
        for glyph in &self.glyphs {
            out.push_str(&format!("{glyph:04X}"));
        }
        out.push('>');
        out
    }
}

/// The font's PostScript name, or a stand-in when it declares none.
fn font_name(font: &FontRef) -> String {
    let name = font
        .localized_strings(StringId::POSTSCRIPT_NAME)
        .next()
        .map(|string| string.chars().collect::<String>())
        .unwrap_or_default();

    // A PDF name may not carry spaces or delimiters, and an empty one would
    // make the font unnameable.
    let cleaned: String = name
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
        .collect();
    if cleaned.is_empty() {
        "EmbeddedFont".to_string()
    } else {
        cleaned
    }
}

/// The six-letter tag that marks a subset, per §9.6.4.
///
/// It has to differ between two subsets of the same font, or a reader is
/// entitled to treat them as the same one and show the wrong glyphs. Derived
/// from the glyphs kept, so the same text always produces the same document.
fn subset_tag(glyphs: &[u16]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for glyph in glyphs {
        for byte in glyph.to_be_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    (0..6)
        .map(|position| {
            let index = (hash >> (position * 5)) % 26;
            (b'A' + index as u8) as char
        })
        .collect()
}

/// The `/ToUnicode` map, so the text can still be selected and searched.
fn to_unicode(pairs: &[(u16, char)]) -> Stream {
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n\
         12 dict begin\n\
         begincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n\
         /CMapType 2 def\n\
         1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );

    // A bfchar block holds at most 100 entries; a longer one is not merely
    // discouraged, readers reject it.
    for block in pairs.chunks(100) {
        cmap.push_str(&format!("{} beginbfchar\n", block.len()));
        for (glyph, character) in block {
            let mut encoded = String::new();
            for unit in character.encode_utf16(&mut [0; 2]).iter() {
                encoded.push_str(&format!("{unit:04X}"));
            }
            cmap.push_str(&format!("<{glyph:04X}> <{encoded}>\n"));
        }
        cmap.push_str("endbfchar\n");
    }

    cmap.push_str(
        "endcmap\n\
         CMapName currentdict /CMap defineresource pop\n\
         end\nend\n",
    );

    let mut stream = Stream::new(Dictionary::new(), cmap.into_bytes());
    let _ = stream.compress();
    stream
}

/// The `/W` array: the advance of every glyph in the subset, in thousandths.
///
/// The identifiers run consecutively from zero, so one range covers them all.
fn widths(metrics: &GlyphMetrics, kept: &[(u16, u16)], units_per_em: f32) -> Vec<Object> {
    let mut ordered: Vec<(u16, u16)> = kept.to_vec();
    ordered.sort_by_key(|(new, _)| *new);

    let advances: Vec<Object> = ordered
        .iter()
        .map(|(_, old)| {
            let advance = metrics
                .advance_width(skrifa::GlyphId::from(*old))
                .unwrap_or(0.0);
            Object::Real(advance * 1000.0 / units_per_em)
        })
        .collect();

    let first = ordered.first().map_or(0, |(new, _)| *new);
    vec![Object::Integer(i64::from(first)), Object::Array(advances)]
}

/// Read `data`, keep only the glyphs `text` needs, and write the result into
/// the document as a composite font.
pub(crate) fn embed(document: &mut Document, data: &[u8], text: &str) -> Result<Embedded, String> {
    let font = FontRef::new(data).map_err(|error| format!("cannot read the font: {error}"))?;
    let charmap = font.charmap();

    // A character the font has no glyph for is refused rather than dropped or
    // replaced: a name missing a letter is worse than a stamp that failed.
    let mut remapper = GlyphRemapper::new();
    let mut glyphs = Vec::with_capacity(text.chars().count());
    let mut kept: Vec<(u16, u16)> = Vec::new();
    let mut unicode: Vec<(u16, char)> = Vec::new();

    for character in text.chars() {
        let Some(old) = charmap.map(character) else {
            return Err(format!(
                "the font has no glyph for {character:?} — supply one that covers the text"
            ));
        };
        let old = u16::try_from(old.to_u32())
            .map_err(|_| "the font has more glyphs than a PDF subset can address".to_string())?;
        let new = remapper.remap(old);
        glyphs.push(new);
        if !kept.iter().any(|(mapped, _)| *mapped == new) {
            kept.push((new, old));
            unicode.push((new, character));
        }
    }

    let subset = subsetter::subset(data, 0, &remapper)
        .map_err(|error| format!("cannot subset the font: {error}"))?;

    let metrics = font.metrics(Size::unscaled(), LocationRef::default());
    let units_per_em = f32::from(metrics.units_per_em);
    if units_per_em <= 0.0 {
        return Err("the font declares no units per em".to_string());
    }
    let scale = 1000.0 / units_per_em;
    let bounds = metrics.bounds.unwrap_or_default();

    let name = format!("{}+{}", subset_tag(&glyphs), font_name(&font));

    let mut file = Stream::new(dictionary! { "Length1" => subset.len() as i64 }, subset);
    file.compress()
        .map_err(|error| format!("cannot compress the font: {error}"))?;
    let file_id = document.add_object(Object::Stream(file));

    // Nonsymbolic, plus what the font says about itself. A reader uses these
    // to pick a substitute if the file is ever stripped.
    let mut flags = 32;
    if metrics.is_monospace {
        flags |= 1;
    }
    if metrics.italic_angle != 0.0 {
        flags |= 64;
    }

    let descriptor_id = document.add_object(dictionary! {
        "Type" => "FontDescriptor",
        "FontName" => Object::Name(name.as_bytes().to_vec()),
        "Flags" => flags,
        "FontBBox" => vec![
            Object::Real(bounds.x_min * scale),
            Object::Real(bounds.y_min * scale),
            Object::Real(bounds.x_max * scale),
            Object::Real(bounds.y_max * scale),
        ],
        "ItalicAngle" => Object::Real(metrics.italic_angle),
        "Ascent" => Object::Real(metrics.ascent * scale),
        "Descent" => Object::Real(metrics.descent * scale),
        "CapHeight" => Object::Real(metrics.cap_height.unwrap_or(metrics.ascent) * scale),
        // No table carries the vertical stem width; readers only use it to
        // choose a substitute, and this is the conventional stand-in.
        "StemV" => 80,
        "FontFile2" => Object::Reference(file_id),
    });

    let glyph_metrics = font.glyph_metrics(Size::unscaled(), LocationRef::default());
    let descendant_id = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "CIDFontType2",
        "BaseFont" => Object::Name(name.as_bytes().to_vec()),
        "CIDSystemInfo" => dictionary! {
            "Registry" => Object::string_literal("Adobe"),
            "Ordering" => Object::string_literal("Identity"),
            "Supplement" => 0,
        },
        "FontDescriptor" => Object::Reference(descriptor_id),
        "W" => widths(&glyph_metrics, &kept, units_per_em),
        "DW" => 1000,
        // The subsetter guarantees identifiers and glyph indices coincide.
        "CIDToGIDMap" => Object::Name(b"Identity".to_vec()),
    });

    let unicode_id = document.add_object(Object::Stream(to_unicode(&unicode)));
    let font_id = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type0",
        "BaseFont" => Object::Name(name.as_bytes().to_vec()),
        "Encoding" => Object::Name(b"Identity-H".to_vec()),
        "DescendantFonts" => vec![Object::Reference(descendant_id)],
        "ToUnicode" => Object::Reference(unicode_id),
    });

    Ok(Embedded { font_id, glyphs })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A real font, because these tests cannot be written without one: the
    /// character map, the advances and the descriptor metrics all have to come
    /// from somewhere, and a font assembled by hand in the test would prove
    /// only that our assembly matches our reader.
    pub(crate) const TEST_FONT: &[u8] = include_bytes!("../tests/fixtures/VellumTestSans.ttf");

    fn embedded(text: &str) -> (Document, Embedded) {
        let mut document = Document::with_version("1.7");
        let embedded = embed(&mut document, TEST_FONT, text).expect("embedding should succeed");
        (document, embedded)
    }

    #[test]
    fn writes_a_composite_font_a_reader_will_accept() {
        let (document, embedded) = embedded("Amelie");
        let font = document
            .get_dictionary(embedded.font_id)
            .expect("the font object exists");

        assert_eq!(font.get(b"Subtype").unwrap().as_name().unwrap(), b"Type0");
        // Identity-H is what makes the content stream's two-byte codes mean
        // glyph identifiers.
        assert_eq!(
            font.get(b"Encoding").unwrap().as_name().unwrap(),
            b"Identity-H"
        );
        assert!(
            font.get(b"ToUnicode").is_ok(),
            "without one the text is drawn but cannot be selected or searched"
        );

        let descendant = font
            .get(b"DescendantFonts")
            .and_then(|fonts| fonts.as_array())
            .expect("a descendant")[0]
            .as_reference()
            .expect("by reference");
        let descendant = document.get_dictionary(descendant).expect("it exists");
        assert_eq!(
            descendant.get(b"Subtype").unwrap().as_name().unwrap(),
            b"CIDFontType2"
        );
        assert_eq!(
            descendant.get(b"CIDToGIDMap").unwrap().as_name().unwrap(),
            b"Identity",
            "the subsetter guarantees identifiers and glyph indices coincide"
        );
        assert!(descendant.get(b"W").is_ok(), "the glyph advances");
    }

    /// The point of subsetting: what goes into the document is a fraction of
    /// the font, not the font.
    #[test]
    fn only_the_glyphs_used_are_carried() {
        let (document, embedded) = embedded("Ame");
        let descriptor = document
            .get_dictionary(embedded.font_id)
            .and_then(|font| font.get(b"DescendantFonts"))
            .and_then(|fonts| fonts.as_array())
            .map(|fonts| fonts[0].as_reference().unwrap())
            .and_then(|id| document.get_dictionary(id))
            .and_then(|descendant| descendant.get(b"FontDescriptor"))
            .and_then(|reference| reference.as_reference())
            .and_then(|id| document.get_dictionary(id))
            .expect("a font descriptor");

        let file = descriptor
            .get(b"FontFile2")
            .and_then(|reference| reference.as_reference())
            .and_then(|id| document.get_object(id))
            .and_then(|object| object.as_stream())
            .expect("the embedded file");

        assert!(
            file.content.len() < TEST_FONT.len() / 2,
            "the subset should be far smaller than the font, got {} against {}",
            file.content.len(),
            TEST_FONT.len()
        );
    }

    /// A character the font cannot draw is refused by name. Substituting a
    /// blank or a fallback would misspell someone's name in a contract.
    #[test]
    fn refuses_a_character_the_font_has_no_glyph_for() {
        let mut document = Document::with_version("1.7");
        let error =
            embed(&mut document, TEST_FONT, "文").expect_err("a glyph the font lacks is refused");
        assert!(
            error.contains('文'),
            "the message should name the character, got {error:?}"
        );
    }

    /// The same character is one glyph however often it appears, and the
    /// identifiers are what the content stream shows.
    #[test]
    fn repeats_reuse_one_glyph() {
        let (_, embedded) = embedded("aaa");
        assert_eq!(embedded.glyphs.len(), 3, "three glyphs are drawn");
        assert_eq!(
            embedded.glyphs[0], embedded.glyphs[2],
            "but they are the same one"
        );
        assert_eq!(embedded.as_hex().len(), 3 * 4 + 2);
    }

    /// Two different subsets of one font must not share a tag, or a reader is
    /// entitled to treat them as the same font and draw the wrong glyphs.
    #[test]
    fn different_subsets_get_different_tags() {
        let name_of = |text: &str| {
            let (document, embedded) = embedded(text);
            document
                .get_dictionary(embedded.font_id)
                .and_then(|font| font.get(b"BaseFont"))
                .and_then(|name| name.as_name())
                .map(|name| String::from_utf8_lossy(name).to_string())
                .expect("a base font name")
        };
        assert_ne!(name_of("Amelie"), name_of("Durand"));
        assert_eq!(name_of("Amelie"), name_of("Amelie"), "and it is stable");
    }
}
