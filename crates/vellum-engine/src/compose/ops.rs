//! What a page draws, kept until the document is written.
//!
//! krilla writes pages one after the other and cannot go back to one, while
//! a document may: a footer saying "page 3 of 12" is written on page 3 once
//! page 12 exists. So each page keeps its drawing as a list, laid out and
//! positioned already, and the list is played into krilla when the document
//! ends.

use krilla::color::rgb;
use krilla::geom::Point;
use krilla::num::NormalizedF32;
use krilla::paint::Fill;
use krilla::surface::Surface;
use krilla::text::{GlyphId, KrillaGlyph};

use super::fonts::LoadedFont;

/// An sRGB colour with its opacity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Colour {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    /// 0 to 1.
    pub opacity: f32,
}

impl Colour {
    pub const BLACK: Self = Self {
        red: 0,
        green: 0,
        blue: 0,
        opacity: 1.0,
    };

    fn fill(self) -> Fill {
        Fill {
            paint: rgb::Color::new(self.red, self.green, self.blue).into(),
            opacity: NormalizedF32::new(self.opacity).unwrap_or(NormalizedF32::ONE),
            rule: Default::default(),
        }
    }
}

/// One glyph of a run, as shaping placed it, in thousandths of an em.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedGlyph {
    pub glyph: u32,
    /// The bytes of the run's text this glyph stands for.
    pub text_start: usize,
    pub text_end: usize,
    pub advance: f32,
    pub x_offset: f32,
    pub y_offset: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// Glyphs of one font, one size, one colour, drawn from `origin` (the
    /// baseline, points from the top-left corner).
    Glyphs {
        font: usize,
        size: f32,
        origin: (f32, f32),
        colour: Colour,
        /// What the glyphs say, so the document can be searched and copied
        /// from.
        text: String,
        glyphs: Vec<PlacedGlyph>,
    },
}

pub fn replay(surface: &mut Surface, ops: &[Op], fonts: &[LoadedFont]) {
    for op in ops {
        match op {
            Op::Glyphs {
                font,
                size,
                origin,
                colour,
                text,
                glyphs,
            } => {
                let Some(font) = fonts.get(*font) else {
                    continue;
                };
                let glyphs: Vec<KrillaGlyph> = glyphs
                    .iter()
                    .map(|glyph| {
                        KrillaGlyph::new(
                            GlyphId::new(glyph.glyph),
                            glyph.advance / 1000.0,
                            glyph.x_offset / 1000.0,
                            glyph.y_offset / 1000.0,
                            0.0,
                            glyph.text_start..glyph.text_end,
                            None,
                        )
                    })
                    .collect();
                surface.set_fill(Some(colour.fill()));
                surface.draw_glyphs(
                    Point::from_xy(origin.0, origin.1),
                    &glyphs,
                    font.krilla.clone(),
                    text,
                    *size,
                    false,
                );
            }
        }
    }
}
