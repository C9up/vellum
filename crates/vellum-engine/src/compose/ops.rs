//! What a page draws, kept until the document is written.
//!
//! krilla writes pages one after the other and cannot go back to one, while
//! a document may: a footer saying "page 3 of 12" is written on page 3 once
//! page 12 exists. So each page keeps its drawing as a list, laid out and
//! positioned already, each operation with the state it was drawn in, and
//! the list is played into krilla when the document ends.

use std::sync::Arc;

use krilla::geom::Point;
use krilla::num::NormalizedF32;
use krilla::paint::{Fill, FillRule, Stroke};
use krilla::surface::Surface;
use krilla::text::{GlyphId, KrillaGlyph};

use super::fonts::LoadedFont;
use super::graphics::{
    build_path, krilla_transform, Clip, Matrix, Paint, PaintBook, Segment, StrokeStyle,
};

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

/// How a shape is filled.
#[derive(Debug, Clone, PartialEq)]
pub struct FillSpec {
    pub paint: Paint,
    pub opacity: f32,
    pub rule: FillRule,
}

/// How a shape is stroked.
#[derive(Debug, Clone, PartialEq)]
pub struct StrokeSpec {
    pub paint: Paint,
    pub opacity: f32,
    pub style: StrokeStyle,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// Glyphs of one font, one size, drawn from `origin` (the baseline, in
    /// the user space `ctm` maps to the page).
    Glyphs {
        font: usize,
        size: f32,
        origin: (f32, f32),
        /// What the glyphs say, so the document can be searched and copied
        /// from.
        text: String,
        glyphs: Vec<PlacedGlyph>,
        fill: Option<FillSpec>,
        stroke: Option<StrokeSpec>,
        ctm: Matrix,
        clips: Arc<Vec<Clip>>,
    },
    /// A path in user space, filled, stroked or both.
    Path {
        segments: Arc<[Segment]>,
        fill: Option<FillSpec>,
        stroke: Option<StrokeSpec>,
        ctm: Matrix,
        clips: Arc<Vec<Clip>>,
    },
}

impl Op {
    fn clips(&self) -> &Arc<Vec<Clip>> {
        match self {
            Op::Glyphs { clips, .. } | Op::Path { clips, .. } => clips,
        }
    }
}

fn fill(spec: &FillSpec, paints: &PaintBook) -> Fill {
    Fill {
        paint: paints.krilla(spec.paint),
        opacity: NormalizedF32::new(spec.opacity).unwrap_or(NormalizedF32::ONE),
        rule: spec.rule,
    }
}

fn stroke(spec: &StrokeSpec, paints: &PaintBook) -> Stroke {
    Stroke {
        paint: paints.krilla(spec.paint),
        width: spec.style.width,
        miter_limit: spec.style.miter_limit,
        line_cap: spec.style.cap,
        line_join: spec.style.join,
        opacity: NormalizedF32::new(spec.opacity).unwrap_or(NormalizedF32::ONE),
        dash: spec.style.krilla_dash(),
    }
}

pub fn replay(surface: &mut Surface, ops: &[Op], fonts: &[LoadedFont], paints: &PaintBook) {
    let mut index = 0;
    while index < ops.len() {
        // Operations under the same clips share one push of them.
        let clips = ops[index].clips().clone();
        let mut end = index + 1;
        while end < ops.len() && Arc::ptr_eq(ops[end].clips(), &clips) {
            end += 1;
        }
        let mut pushed = 0;
        for clip in clips.iter() {
            // Checked when the clip was taken; an empty one clips
            // everything, a point away from the page standing for it.
            let path = build_path(&clip.segments).ok().flatten().or_else(|| {
                build_path(&[
                    Segment::Move(-1.0, -1.0),
                    Segment::Line(-1.0, -1.0),
                    Segment::Close,
                ])
                .ok()
                .flatten()
            });
            if let Some(path) = path {
                surface.push_clip_path(&path, &clip.rule);
                pushed += 1;
            }
        }
        for op in &ops[index..end] {
            draw(surface, op, fonts, paints);
        }
        for _ in 0..pushed {
            surface.pop();
        }
        index = end;
    }
}

fn draw(surface: &mut Surface, op: &Op, fonts: &[LoadedFont], paints: &PaintBook) {
    match op {
        Op::Glyphs {
            font,
            size,
            origin,
            text,
            glyphs,
            fill: fill_spec,
            stroke: stroke_spec,
            ctm,
            ..
        } => {
            let Some(font) = fonts.get(*font) else {
                return;
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
            surface.push_transform(&krilla_transform(*ctm));
            surface.set_fill(fill_spec.as_ref().map(|spec| fill(spec, paints)));
            surface.set_stroke(stroke_spec.as_ref().map(|spec| stroke(spec, paints)));
            surface.draw_glyphs(
                Point::from_xy(origin.0, origin.1),
                &glyphs,
                font.krilla.clone(),
                text,
                *size,
                false,
            );
            surface.pop();
        }
        Op::Path {
            segments,
            fill: fill_spec,
            stroke: stroke_spec,
            ctm,
            ..
        } => {
            if fill_spec.is_none() && stroke_spec.is_none() {
                return;
            }
            let Ok(Some(path)) = build_path(segments) else {
                return;
            };
            surface.push_transform(&krilla_transform(*ctm));
            surface.set_fill(fill_spec.as_ref().map(|spec| fill(spec, paints)));
            surface.set_stroke(stroke_spec.as_ref().map(|spec| stroke(spec, paints)));
            surface.draw_path(&path);
            surface.pop();
        }
    }
}
