//! pdfkit's vector mixin on the composer: the graphics state, paints, and
//! painting paths built on the TypeScript side.

use std::sync::Arc;

use krilla::paint::{FillRule, LineCap, LineJoin};

use super::graphics::{
    build_path, concat, opacity, transform_segments, Clip, Gradient, Matrix, Paint, Segment,
    SpotColour,
};
use super::ops::{FillSpec, Op, StrokeSpec};
use super::{ComposeError, Composer};

/// At most this many saves deep: pdfkit has no limit, a document has no use
/// for more, and each one is a copy of the state.
pub const MAX_SAVE_DEPTH: usize = 1024;
/// Gradients or spot colours, each, in one document.
pub const MAX_PAINTS: usize = 65_536;

fn invalid(message: impl Into<String>) -> ComposeError {
    ComposeError::new("INVALID_OPTIONS", message)
}

fn finite(values: &[f32], name: &str) -> Result<(), ComposeError> {
    if values.iter().all(|value| value.is_finite()) {
        Ok(())
    } else {
        Err(invalid(format!("{name} must be finite numbers")))
    }
}

impl Composer {
    pub fn save(&mut self) -> Result<(), ComposeError> {
        if self.saved.len() >= MAX_SAVE_DEPTH {
            return Err(invalid(format!(
                "save() is nested more than {MAX_SAVE_DEPTH} deep"
            )));
        }
        self.saved.push(self.state.clone());
        Ok(())
    }

    /// pdfkit writes an unmatched `Q`, which breaks the page; here it is
    /// refused.
    pub fn restore(&mut self) -> Result<(), ComposeError> {
        self.state = self.saved.pop().ok_or_else(|| {
            ComposeError::new("UNBALANCED_RESTORE", "restore() without a matching save()")
        })?;
        Ok(())
    }

    pub fn transform(&mut self, matrix: Matrix) -> Result<(), ComposeError> {
        finite(&matrix, "a transform")?;
        self.state.ctm = concat(self.state.ctm, matrix);
        Ok(())
    }

    pub fn ctm(&self) -> Matrix {
        self.state.ctm
    }

    fn check_paint(&self, paint: Paint) -> Result<(), ComposeError> {
        let known = match paint {
            Paint::Colour(_) => true,
            Paint::Spot(index) => index < self.paints.spots.len(),
            Paint::Gradient(index) => index < self.paints.gradients.len(),
        };
        if known {
            Ok(())
        } else {
            Err(invalid("no paint has this handle"))
        }
    }

    pub fn set_fill(&mut self, paint: Paint) -> Result<(), ComposeError> {
        self.check_paint(paint)?;
        self.state.fill = paint;
        Ok(())
    }

    pub fn set_stroke(&mut self, paint: Paint) -> Result<(), ComposeError> {
        self.check_paint(paint)?;
        self.state.stroke = paint;
        Ok(())
    }

    /// Clamped to 0–1, as pdfkit clamps it.
    pub fn set_fill_opacity(&mut self, value: f32) {
        self.state.fill_opacity = opacity(value);
    }

    pub fn set_stroke_opacity(&mut self, value: f32) {
        self.state.stroke_opacity = opacity(value);
    }

    pub fn set_line_width(&mut self, width: f32) -> Result<(), ComposeError> {
        if !width.is_finite() || width < 0.0 {
            return Err(invalid("lineWidth must be a finite number, 0 or more"));
        }
        self.state.stroke_style.width = width;
        Ok(())
    }

    pub fn set_line_cap(&mut self, cap: LineCap) {
        self.state.stroke_style.cap = cap;
    }

    pub fn set_line_join(&mut self, join: LineJoin) {
        self.state.stroke_style.join = join;
    }

    pub fn set_miter_limit(&mut self, limit: f32) -> Result<(), ComposeError> {
        if !limit.is_finite() || limit < 1.0 {
            return Err(invalid("miterLimit must be a finite number, 1 or more"));
        }
        self.state.stroke_style.miter_limit = limit;
        Ok(())
    }

    /// `None` is a solid line. pdfkit refuses a length of 0 or less.
    pub fn set_dash(&mut self, dash: Option<(Vec<f32>, f32)>) -> Result<(), ComposeError> {
        if let Some((array, phase)) = &dash {
            if array.is_empty() || array.len() > 64 {
                return Err(invalid("a dash pattern has 1 to 64 lengths"));
            }
            if array
                .iter()
                .any(|length| !length.is_finite() || *length <= 0.0)
            {
                return Err(invalid(format!(
                    "dash({array:?}) is invalid: every length must be above 0"
                )));
            }
            finite(&[*phase], "a dash phase")?;
        }
        self.state.stroke_style.dash = dash;
        Ok(())
    }

    pub fn add_gradient(&mut self, gradient: Gradient) -> Result<usize, ComposeError> {
        gradient.check()?;
        if self.paints.gradients.len() >= MAX_PAINTS {
            return Err(invalid(format!(
                "a document takes at most {MAX_PAINTS} gradients"
            )));
        }
        self.paints.gradients.push(gradient);
        Ok(self.paints.gradients.len() - 1)
    }

    /// A spot colour by name; the same name answers the same handle.
    pub fn add_spot(&mut self, name: &str, fallback: [u8; 4]) -> Result<usize, ComposeError> {
        if name.is_empty() || name.len() > 127 {
            return Err(invalid("a spot colour's name is 1 to 127 bytes"));
        }
        if let Some(index) = self.paints.spots.iter().position(|spot| spot.name == name) {
            self.paints.spots[index].fallback = fallback;
            return Ok(index);
        }
        if self.paints.spots.len() >= MAX_PAINTS {
            return Err(invalid(format!(
                "a document takes at most {MAX_PAINTS} spot colours"
            )));
        }
        self.paints.spots.push(SpotColour {
            name: name.to_string(),
            fallback,
        });
        Ok(self.paints.spots.len() - 1)
    }

    /// Fill the path with `rule`, stroke it, or both.
    pub fn paint_path(
        &mut self,
        segments: &[Segment],
        fill: Option<FillRule>,
        stroke: bool,
    ) -> Result<(), ComposeError> {
        if build_path(segments)?.is_none() {
            return Ok(());
        }
        let state = &self.state;
        let op = Op::Path {
            segments: segments.into(),
            fill: fill.map(|rule| FillSpec {
                paint: state.fill,
                opacity: state.fill_opacity,
                rule,
            }),
            stroke: stroke.then(|| StrokeSpec {
                paint: state.stroke,
                opacity: state.stroke_opacity,
                style: state.stroke_style.clone(),
            }),
            ctm: state.ctm,
            clips: state.clips.clone(),
        };
        self.page_mut()?.ops.push(op);
        Ok(())
    }

    /// Intersect the clip with the path, until the next `restore()`.
    pub fn clip(&mut self, segments: &[Segment], rule: FillRule) -> Result<(), ComposeError> {
        self.page()?;
        build_path(segments)?;
        let mut clips = self.state.clips.as_ref().clone();
        clips.push(Clip {
            segments: transform_segments(segments, self.state.ctm),
            rule,
        });
        if clips.len() > 24 {
            return Err(invalid("at most 24 clips may be in effect at once"));
        }
        self.state.clips = Arc::new(clips);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::graphics::{Colour, GradientShape, GradientStop, IDENTITY};
    use crate::compose::{DocumentSettings, PageSetup, Sides};
    use crate::{render_page, ImageFormat, RenderOptions};
    use hayro::vello_cpu::color::PremulRgba8;
    use hayro::vello_cpu::Pixmap;
    use std::io::Cursor;

    const PAGE: PageSetup = PageSetup {
        width: 200.0,
        height: 200.0,
        margins: Sides {
            top: 10.0,
            right: 10.0,
            bottom: 10.0,
            left: 10.0,
        },
        user_unit: 1.0,
    };

    fn document() -> Composer {
        let mut document = Composer::new(DocumentSettings::default()).unwrap();
        document.add_page(PAGE).unwrap();
        document
    }

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Vec<Segment> {
        vec![
            Segment::Move(x, y),
            Segment::Line(x + width, y),
            Segment::Line(x + width, y + height),
            Segment::Line(x, y + height),
            Segment::Close,
        ]
    }

    /// The page rendered at one pixel per point.
    fn pixels(document: &mut Composer) -> Pixmap {
        let bytes = document.end().unwrap();
        let png = render_page(
            &bytes,
            0,
            &RenderOptions {
                format: ImageFormat::Png,
                ..Default::default()
            },
        )
        .unwrap();
        Pixmap::from_png(Cursor::new(png)).unwrap()
    }

    fn at(pixmap: &Pixmap, x: u32, y: u32) -> PremulRgba8 {
        pixmap.data()[(y * u32::from(pixmap.width()) + x) as usize]
    }

    fn is(pixel: PremulRgba8, red: u8, green: u8, blue: u8) -> bool {
        pixel.r.abs_diff(red) < 20 && pixel.g.abs_diff(green) < 20 && pixel.b.abs_diff(blue) < 20
    }

    #[test]
    fn fills_and_strokes_where_it_is_told() {
        let mut document = document();
        document
            .set_fill(Paint::Colour(Colour::Rgb(255, 0, 0)))
            .unwrap();
        document
            .paint_path(
                &rect(20.0, 20.0, 50.0, 50.0),
                Some(FillRule::NonZero),
                false,
            )
            .unwrap();
        document
            .set_stroke(Paint::Colour(Colour::Rgb(0, 0, 255)))
            .unwrap();
        document.set_line_width(10.0).unwrap();
        document
            .paint_path(
                &[Segment::Move(100.0, 150.0), Segment::Line(190.0, 150.0)],
                None,
                true,
            )
            .unwrap();
        let pixmap = pixels(&mut document);
        assert!(is(at(&pixmap, 45, 45), 255, 0, 0), "the square is red");
        assert!(
            is(at(&pixmap, 10, 10), 255, 255, 255),
            "outside it is blank"
        );
        assert!(is(at(&pixmap, 150, 150), 0, 0, 255), "the line is blue");
        assert!(
            is(at(&pixmap, 150, 160), 255, 255, 255),
            "and 10 points thick"
        );
    }

    #[test]
    fn transforms_apply_to_what_follows_until_restore() {
        let mut document = document();
        document
            .set_fill(Paint::Colour(Colour::Rgb(0, 128, 0)))
            .unwrap();
        document.save().unwrap();
        document
            .transform([1.0, 0.0, 0.0, 1.0, 100.0, 100.0])
            .unwrap();
        document
            .paint_path(&rect(0.0, 0.0, 20.0, 20.0), Some(FillRule::NonZero), false)
            .unwrap();
        document.restore().unwrap();
        document
            .paint_path(&rect(0.0, 0.0, 20.0, 20.0), Some(FillRule::NonZero), false)
            .unwrap();
        let pixmap = pixels(&mut document);
        assert!(
            is(at(&pixmap, 110, 110), 0, 128, 0),
            "moved by the transform"
        );
        assert!(is(at(&pixmap, 10, 10), 0, 128, 0), "and back once restored");
        assert!(is(at(&pixmap, 50, 50), 255, 255, 255));
    }

    #[test]
    fn a_clip_keeps_drawing_inside_it() {
        let mut document = document();
        document.save().unwrap();
        document
            .clip(&rect(0.0, 0.0, 100.0, 200.0), FillRule::NonZero)
            .unwrap();
        document
            .set_fill(Paint::Colour(Colour::Rgb(255, 0, 0)))
            .unwrap();
        document
            .paint_path(
                &rect(0.0, 0.0, 200.0, 200.0),
                Some(FillRule::NonZero),
                false,
            )
            .unwrap();
        document.restore().unwrap();
        let pixmap = pixels(&mut document);
        assert!(is(at(&pixmap, 50, 100), 255, 0, 0));
        assert!(is(at(&pixmap, 150, 100), 255, 255, 255), "clipped away");
    }

    #[test]
    fn even_odd_leaves_the_hole() {
        let mut document = document();
        let mut ring = rect(20.0, 20.0, 160.0, 160.0);
        ring.extend(rect(60.0, 60.0, 80.0, 80.0));
        document
            .paint_path(&ring, Some(FillRule::EvenOdd), false)
            .unwrap();
        let pixmap = pixels(&mut document);
        assert!(is(at(&pixmap, 30, 100), 0, 0, 0));
        assert!(is(at(&pixmap, 100, 100), 255, 255, 255), "the hole");
    }

    #[test]
    fn opacity_and_gradients() {
        let mut document = document();
        document
            .set_fill(Paint::Colour(Colour::Rgb(0, 0, 0)))
            .unwrap();
        document.set_fill_opacity(0.5);
        document
            .paint_path(&rect(0.0, 0.0, 100.0, 50.0), Some(FillRule::NonZero), false)
            .unwrap();
        document.set_fill_opacity(1.0);
        let gradient = document
            .add_gradient(Gradient {
                shape: GradientShape::Linear {
                    x1: 0.0,
                    y1: 0.0,
                    x2: 200.0,
                    y2: 0.0,
                },
                stops: vec![
                    GradientStop {
                        offset: 0.0,
                        colour: Colour::Rgb(255, 0, 0),
                        opacity: 1.0,
                    },
                    GradientStop {
                        offset: 1.0,
                        colour: Colour::Rgb(0, 0, 255),
                        opacity: 1.0,
                    },
                ],
                transform: IDENTITY,
            })
            .unwrap();
        document.set_fill(Paint::Gradient(gradient)).unwrap();
        document
            .paint_path(
                &rect(0.0, 100.0, 200.0, 50.0),
                Some(FillRule::NonZero),
                false,
            )
            .unwrap();
        let pixmap = pixels(&mut document);
        let grey = at(&pixmap, 50, 25);
        assert!(
            (100..160).contains(&grey.r),
            "half opaque black is grey: {grey:?}"
        );
        let left = at(&pixmap, 5, 125);
        let right = at(&pixmap, 195, 125);
        assert!(left.r > 200 && left.b < 60, "red on the left: {left:?}");
        assert!(
            right.b > 200 && right.r < 60,
            "blue on the right: {right:?}"
        );
    }

    #[test]
    fn refuses_what_would_break_the_page() {
        let mut document = document();
        assert_eq!(document.restore().unwrap_err().code, "UNBALANCED_RESTORE");
        assert!(document.set_dash(Some((vec![0.0], 0.0))).is_err());
        assert!(document.set_dash(Some((vec![-1.0, 2.0], 0.0))).is_err());
        assert!(document.set_line_width(f32::NAN).is_err());
        assert!(document
            .transform([f32::INFINITY, 0.0, 0.0, 1.0, 0.0, 0.0])
            .is_err());
        assert!(document.set_fill(Paint::Gradient(3)).is_err());
        for _ in 0..MAX_SAVE_DEPTH {
            document.save().unwrap();
        }
        assert!(document.save().is_err());
    }

    #[test]
    fn a_new_page_starts_untransformed_but_keeps_its_colours() {
        let mut document = document();
        document
            .set_fill(Paint::Colour(Colour::Rgb(255, 0, 0)))
            .unwrap();
        document.save().unwrap();
        document.transform([2.0, 0.0, 0.0, 2.0, 0.0, 0.0]).unwrap();
        document.add_page(PAGE).unwrap();
        assert_eq!(document.ctm(), IDENTITY);
        assert_eq!(document.restore().unwrap_err().code, "UNBALANCED_RESTORE");
        assert_eq!(document.state.fill, Paint::Colour(Colour::Rgb(255, 0, 0)));
    }

    #[test]
    fn a_spot_colour_is_named_once() {
        let mut document = document();
        let first = document
            .add_spot("PANTONE 185 C", [0, 230, 194, 0])
            .unwrap();
        let again = document
            .add_spot("PANTONE 185 C", [0, 230, 194, 0])
            .unwrap();
        assert_eq!(first, again);
        document.set_fill(Paint::Spot(first)).unwrap();
        document
            .paint_path(&rect(0.0, 0.0, 50.0, 50.0), Some(FillRule::NonZero), false)
            .unwrap();
        let bytes = document.end().unwrap();
        let parsed = lopdf::Document::load_mem(&bytes).unwrap();
        let has_separation = parsed.objects.values().any(|object| {
            object.as_array().is_ok_and(|array| {
                array.first().and_then(|o| o.as_name().ok()) == Some(b"Separation")
            })
        });
        assert!(has_separation, "the colour is written as a Separation");
    }
}
