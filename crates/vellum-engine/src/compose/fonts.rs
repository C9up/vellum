//! Fonts a document draws with: loaded once, measured, shaped, embedded.
//!
//! A font file is input like any other upload, so it is bounded before it is
//! parsed and refused unless every parser the document relies on accepts it:
//! rustybuzz measures and shapes, krilla embeds. A file one of them rejects
//! would otherwise fail later, at a point where nothing says which font did.

use std::sync::Arc;

use rustybuzz::ttf_parser::name_id;
use rustybuzz::{Face, Feature, UnicodeBuffer};

use super::standard::StandardFace;
use super::ComposeError;

/// Larger than any font a document has a use for: a full CJK family is about
/// 20 MiB.
pub const MAX_FONT_BYTES: usize = 64 * 1024 * 1024;

/// What sits above and below the baseline, in thousandths of an em.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertical {
    pub ascender: f32,
    pub descender: f32,
    pub line_gap: f32,
    pub cap_height: f32,
    pub x_height: f32,
}

/// A glyph as shaping placed it, in thousandths of an em.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shaped {
    pub glyph: u32,
    /// Byte offset, in the shaped text, of the cluster the glyph belongs to.
    pub cluster: usize,
    pub advance: f32,
    pub x_offset: f32,
    pub y_offset: f32,
}

pub struct LoadedFont {
    data: Arc<Vec<u8>>,
    index: u32,
    pub krilla: krilla::text::Font,
    pub vertical: Vertical,
    /// The name the file gives itself, to tell two loads of one font apart.
    pub postscript_name: Option<String>,
}

fn invalid(reason: impl Into<String>) -> ComposeError {
    ComposeError::new("INVALID_FONT", reason)
}

/// The face of a collection whose PostScript name is `family`.
fn face_named(data: &[u8], family: &str) -> Result<u32, ComposeError> {
    let count = rustybuzz::ttf_parser::fonts_in_collection(data).unwrap_or(1);
    (0..count)
        .find(|&index| {
            Face::from_slice(data, index)
                .and_then(|face| postscript_name(&face))
                .is_some_and(|name| name == family)
        })
        .ok_or_else(|| invalid(format!("the font file has no face named {family:?}")))
}

fn postscript_name(face: &Face) -> Option<String> {
    face.names()
        .into_iter()
        .filter(|name| name.name_id == name_id::POST_SCRIPT_NAME)
        .find_map(|name| name.to_string())
}

impl LoadedFont {
    /// `family` picks a face of a collection (TTC) by PostScript name, as
    /// it does for pdfkit; a single font ignores it. `standard` replaces the
    /// vertical metrics with a standard face's.
    pub fn load(
        bytes: Vec<u8>,
        family: Option<&str>,
        standard: Option<&StandardFace>,
    ) -> Result<Self, ComposeError> {
        if bytes.len() > MAX_FONT_BYTES {
            return Err(invalid(format!(
                "a font file of {} bytes is over the limit of {MAX_FONT_BYTES}",
                bytes.len()
            )));
        }
        let index = match (family, rustybuzz::ttf_parser::fonts_in_collection(&bytes)) {
            (Some(family), Some(_)) => face_named(&bytes, family)?,
            _ => 0,
        };
        let face = Face::from_slice(&bytes, index)
            .ok_or_else(|| invalid("the file is not a font this engine can read"))?;
        let units = face.units_per_em() as f32;
        if units <= 0.0 {
            return Err(invalid("the font declares no units per em"));
        }
        let scale = 1000.0 / units;
        // Read as fontkit reads them, from `hhea`, so a supplied font lines
        // up exactly where it does with pdfkit.
        let hhea = face.tables().hhea;
        let vertical = match standard {
            Some(standard) => Vertical {
                ascender: standard.ascender,
                descender: standard.descender,
                line_gap: standard.line_gap(),
                cap_height: standard.cap_height,
                x_height: standard.x_height,
            },
            None => Vertical {
                ascender: f32::from(hhea.ascender) * scale,
                descender: f32::from(hhea.descender) * scale,
                line_gap: f32::from(hhea.line_gap) * scale,
                cap_height: face
                    .capital_height()
                    .map_or(f32::from(hhea.ascender), f32::from)
                    * scale,
                x_height: face.x_height().map_or(0.0, f32::from) * scale,
            },
        };
        let tables = face.tables();
        let outlines = tables.glyf.is_some() || tables.cff.is_some() || tables.cff2.is_some();
        if !outlines || tables.hmtx.is_none() || tables.cmap.is_none() {
            return Err(invalid(
                "the font lacks the outline, metric or character tables a document needs",
            ));
        }
        // The table directory can point past the end of the file and still
        // parse. Subsetting reads the glyphs themselves, as krilla will when
        // the document is written: a font that fails here would fail there.
        let mut probe = subsetter::GlyphRemapper::new();
        for character in ['0', 'A', 'a', ' '] {
            if let Some(glyph) = face.glyph_index(character) {
                probe.remap(glyph.0);
            }
        }
        subsetter::subset(&bytes, index, &probe)
            .map_err(|error| invalid(format!("the font cannot be subset: {error:?}")))?;
        let postscript_name = postscript_name(&face);
        drop(face);

        let data = Arc::new(bytes);
        let krilla = krilla::text::Font::new(data.clone().into(), index)
            .ok_or_else(|| invalid("the font cannot be embedded"))?;
        Ok(Self {
            data,
            index,
            krilla,
            vertical,
            postscript_name,
        })
    }

    fn face(&self) -> Result<Face<'_>, ComposeError> {
        // Parsed once already in `load`: this cannot fail on the same bytes,
        // but a refusal is still an answer rather than a panic.
        Face::from_slice(&self.data, self.index).ok_or_else(|| invalid("the font cannot be read"))
    }

    pub fn line_height(&self, size: f32, include_gap: bool) -> f32 {
        let gap = if include_gap {
            self.vertical.line_gap
        } else {
            0.0
        };
        (self.vertical.ascender + gap - self.vertical.descender) / 1000.0 * size
    }

    /// Shape one run of text in one direction.
    pub fn shape(
        &self,
        text: &str,
        features: &[Feature],
        rtl: bool,
    ) -> Result<Vec<Shaped>, ComposeError> {
        if text.is_empty() {
            return Ok(Vec::new());
        }
        let face = self.face()?;
        let scale = 1000.0 / face.units_per_em() as f32;
        let mut buffer = UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.guess_segment_properties();
        buffer.set_direction(if rtl {
            rustybuzz::Direction::RightToLeft
        } else {
            rustybuzz::Direction::LeftToRight
        });
        let shaped = rustybuzz::shape(&face, features, buffer);
        Ok(shaped
            .glyph_infos()
            .iter()
            .zip(shaped.glyph_positions())
            .map(|(info, position)| Shaped {
                glyph: info.glyph_id,
                cluster: info.cluster as usize,
                advance: position.x_advance as f32 * scale,
                x_offset: position.x_offset as f32 * scale,
                y_offset: position.y_offset as f32 * scale,
            })
            .collect())
    }

    /// The advance of `text`, in points at `size`.
    pub fn width(&self, text: &str, size: f32, features: &[Feature]) -> Result<f32, ComposeError> {
        let rtl = text.chars().any(is_strong_rtl);
        let advance: f32 = self
            .shape(text, features, rtl)?
            .iter()
            .map(|glyph| glyph.advance)
            .sum();
        Ok(advance / 1000.0 * size)
    }

    /// Same file, same face: loading it again would embed it twice.
    pub fn is_same(&self, other: &LoadedFont) -> bool {
        self.index == other.index
            && (Arc::ptr_eq(&self.data, &other.data) || self.data == other.data)
    }
}

/// Hebrew, Arabic and the scripts around them, which run right to left.
pub fn is_strong_rtl(character: char) -> bool {
    matches!(
        unicode_bidi::bidi_class(character),
        unicode_bidi::BidiClass::R | unicode_bidi::BidiClass::AL
    )
}

/// OpenType feature tags, as pdfkit takes them: each one switched on.
pub fn parse_features(tags: &[String]) -> Result<Vec<Feature>, ComposeError> {
    tags.iter()
        .map(|tag| {
            if tag.len() != 4 || !tag.bytes().all(|byte| byte.is_ascii_graphic()) {
                return Err(ComposeError::new(
                    "INVALID_OPTIONS",
                    format!("{tag:?} is not an OpenType feature tag"),
                ));
            }
            Ok(Feature::new(
                rustybuzz::ttf_parser::Tag::from_bytes_lossy(tag.as_bytes()),
                1,
                ..,
            ))
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::compose::standard::standard;

    pub fn liberation_sans() -> Vec<u8> {
        std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fonts/LiberationSans-Regular.ttf"
        ))
        .expect("the package ships Liberation Sans")
    }

    #[test]
    fn a_stand_in_takes_the_standard_vertical_metrics() {
        let helvetica = LoadedFont::load(liberation_sans(), None, standard("Helvetica")).unwrap();
        // pdfkit: (718 - (-207)) / 1000 * 12
        assert!((helvetica.line_height(12.0, false) - 11.1).abs() < 1e-4);

        let own = LoadedFont::load(liberation_sans(), None, None).unwrap();
        assert!(
            own.line_height(12.0, false) > 13.0,
            "Liberation's own hhea is taller"
        );
    }

    #[test]
    fn widths_match_helvetica() {
        let font = LoadedFont::load(liberation_sans(), None, standard("Helvetica")).unwrap();
        // Helvetica AFM: H 722, e 556, l 222, o 556.
        let width = font.width("Hello", 1000.0, &[]).unwrap();
        assert!(
            (width - (722.0 + 556.0 + 222.0 + 222.0 + 556.0)).abs() < 1.0,
            "got {width}"
        );
    }

    #[test]
    fn refuses_what_is_not_a_font() {
        assert!(LoadedFont::load(b"not a font at all".to_vec(), None, None).is_err());
        assert!(LoadedFont::load(Vec::new(), None, None).is_err());
        let mut truncated = liberation_sans();
        truncated.truncate(2000);
        assert!(LoadedFont::load(truncated, None, None).is_err());
    }

    const CFF: &[u8] = include_bytes!("../../tests/fixtures/Montserrat-Bold-Subset.otf");
    const COLLECTION: &[u8] = include_bytes!("../../tests/fixtures/Pair.ttc");

    #[test]
    fn reads_an_opentype_font_with_cff_outlines() {
        let font = LoadedFont::load(CFF.to_vec(), None, None).unwrap();
        assert!(font.width("Devis", 12.0, &[]).unwrap() > 20.0);
        assert_eq!(font.postscript_name.as_deref(), Some("Montserrat-Bold"));
    }

    #[test]
    fn picks_a_face_of_a_collection_by_postscript_name() {
        let first = LoadedFont::load(COLLECTION.to_vec(), None, None).unwrap();
        assert_eq!(
            first.postscript_name.as_deref(),
            Some("NotoSansArabic-Regular")
        );
        let second =
            LoadedFont::load(COLLECTION.to_vec(), Some("VellumTestSans-Regular"), None).unwrap();
        assert_eq!(
            second.postscript_name.as_deref(),
            Some("VellumTestSans-Regular")
        );
        assert!(!first.is_same(&second));
        assert!(LoadedFont::load(COLLECTION.to_vec(), Some("Nope"), None).is_err());
        // A single font ignores the family, as fontkit does.
        assert!(LoadedFont::load(liberation_sans(), Some("Nope"), None).is_ok());
    }

    #[test]
    fn refuses_a_feature_that_is_not_a_tag() {
        assert!(parse_features(&["liga".into(), "kern".into()]).is_ok());
        assert!(parse_features(&["ligature".into()]).is_err());
        assert!(parse_features(&["li a".into()]).is_err());
    }
}
