//! The graphics state: transformation, paints, opacities, stroke style and
//! clipping, with pdfkit's `save()` / `restore()` around it.
//!
//! pdfkit writes each change straight into the page's content stream. Here
//! the state is kept on the Rust side and every drawing operation records
//! the state it was drawn with, so a page can be replayed into krilla at the
//! end without a `q`/`Q` nesting that krilla caps at 28.
//!
//! One difference follows, named: the state carries over to the next page.
//! pdfkit keeps the colour it last set but starts each page's content stream
//! from the defaults, so a line width or an opacity set before a page break
//! silently stops applying after it.

use std::sync::Arc;

use krilla::color::{cmyk, rgb, separation, Color, RegularColor, SpecialColor};
use krilla::geom::{Path, PathBuilder, Transform};
use krilla::num::NormalizedF32;
/// How a path is filled, its ends and corners stroked: krilla's own.
pub use krilla::paint::{FillRule, LineCap, LineJoin};
use krilla::paint::{LinearGradient, RadialGradient, SpreadMethod, Stop, StrokeDash};

use super::ComposeError;

/// `[a, b, c, d, e, f]`, PDF's order.
pub type Matrix = [f32; 6];

pub const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `outer × inner`: what pdfkit's `transform()` does to its CTM.
pub fn concat(outer: Matrix, inner: Matrix) -> Matrix {
    let [a, b, c, d, e, f] = outer;
    let [m11, m12, m21, m22, dx, dy] = inner;
    [
        a * m11 + c * m12,
        b * m11 + d * m12,
        a * m21 + c * m22,
        b * m21 + d * m22,
        a * dx + c * dy + e,
        b * dx + d * dy + f,
    ]
}

pub fn krilla_transform(matrix: Matrix) -> Transform {
    let [a, b, c, d, e, f] = matrix;
    Transform::from_row(a, b, c, d, e, f)
}

/// A colour model a paint or a gradient stop is in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Colour {
    Rgb(u8, u8, u8),
    /// Each component 0 to 255.
    Cmyk(u8, u8, u8, u8),
}

impl Colour {
    fn krilla(self) -> Color {
        match self {
            Self::Rgb(red, green, blue) => {
                Color::Regular(RegularColor::Rgb(rgb::Color::new(red, green, blue)))
            }
            Self::Cmyk(cyan, magenta, yellow, black) => Color::Regular(RegularColor::Cmyk(
                cmyk::Color::new(cyan, magenta, yellow, black),
            )),
        }
    }

    fn same_space(self, other: Colour) -> bool {
        matches!(
            (self, other),
            (Self::Rgb(..), Self::Rgb(..)) | (Self::Cmyk(..), Self::Cmyk(..))
        )
    }
}

/// What a fill or a stroke is painted with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Paint {
    Colour(Colour),
    /// A spot colour, by its index in the document, at full tint.
    Spot(usize),
    /// A gradient, by its index in the document.
    Gradient(usize),
}

impl Paint {
    pub const BLACK: Self = Self::Colour(Colour::Rgb(0, 0, 0));
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpotColour {
    pub name: String,
    /// The CMYK the colour falls back to, each 0 to 255.
    pub fallback: [u8; 4],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GradientShape {
    Linear {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
    },
    Radial {
        x1: f32,
        y1: f32,
        r1: f32,
        x2: f32,
        y2: f32,
        r2: f32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct GradientStop {
    pub offset: f32,
    pub colour: Colour,
    pub opacity: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Gradient {
    pub shape: GradientShape,
    pub stops: Vec<GradientStop>,
    pub transform: Matrix,
}

fn normalized(value: f32, name: &str) -> Result<NormalizedF32, ComposeError> {
    NormalizedF32::new(value).ok_or_else(|| {
        ComposeError::new(
            "INVALID_OPTIONS",
            format!("{name} must be between 0 and 1, got {value}"),
        )
    })
}

impl Gradient {
    pub fn check(&self) -> Result<(), ComposeError> {
        let invalid = |message: &str| Err(ComposeError::new("INVALID_OPTIONS", message));
        if self.stops.is_empty() {
            return invalid("a gradient needs at least one stop");
        }
        if self.stops.len() > 256 {
            return invalid("a gradient takes at most 256 stops");
        }
        let first = self.stops[0].colour;
        if self.stops.iter().any(|stop| !stop.colour.same_space(first)) {
            return invalid("every stop of a gradient must be in the same colour space");
        }
        for stop in &self.stops {
            normalized(stop.offset, "a stop's position")?;
            normalized(stop.opacity, "a stop's opacity")?;
        }
        let finite = match self.shape {
            GradientShape::Linear { x1, y1, x2, y2 } => {
                [x1, y1, x2, y2].iter().all(|v| v.is_finite())
            }
            GradientShape::Radial {
                x1,
                y1,
                r1,
                x2,
                y2,
                r2,
            } => [x1, y1, r1, x2, y2, r2].iter().all(|v| v.is_finite()) && r1 >= 0.0 && r2 >= 0.0,
        };
        if !finite || !self.transform.iter().all(|value| value.is_finite()) {
            return invalid("a gradient's coordinates must be finite, its radii positive");
        }
        Ok(())
    }

    fn krilla(&self) -> krilla::paint::Paint {
        let mut stops: Vec<Stop> = self
            .stops
            .iter()
            .map(|stop| Stop {
                offset: NormalizedF32::new(stop.offset).unwrap_or(NormalizedF32::ZERO),
                color: stop.colour.krilla(),
                opacity: NormalizedF32::new(stop.opacity).unwrap_or(NormalizedF32::ONE),
            })
            .collect();
        stops.sort_by(|a, b| a.offset.get().total_cmp(&b.offset.get()));
        let transform = krilla_transform(self.transform);
        match self.shape {
            GradientShape::Linear { x1, y1, x2, y2 } => LinearGradient {
                x1,
                y1,
                x2,
                y2,
                transform,
                spread_method: SpreadMethod::Pad,
                stops,
                anti_alias: true,
            }
            .into(),
            GradientShape::Radial {
                x1,
                y1,
                r1,
                x2,
                y2,
                r2,
            } => RadialGradient {
                fx: x1,
                fy: y1,
                fr: r1,
                cx: x2,
                cy: y2,
                cr: r2,
                transform,
                spread_method: SpreadMethod::Pad,
                stops,
                anti_alias: true,
            }
            .into(),
        }
    }
}

/// The document's gradients and spot colours, which paints refer to.
#[derive(Debug, Default)]
pub struct PaintBook {
    pub gradients: Vec<Gradient>,
    pub spots: Vec<SpotColour>,
}

impl PaintBook {
    pub fn krilla(&self, paint: Paint) -> krilla::paint::Paint {
        match paint {
            Paint::Colour(colour) => colour.krilla().into(),
            Paint::Spot(index) => match self.spots.get(index) {
                Some(spot) => {
                    let [cyan, magenta, yellow, black] = spot.fallback;
                    let space = separation::SeparationSpace::new(
                        separation::SeparationColorant::Custom(spot.name.clone()),
                        RegularColor::Cmyk(cmyk::Color::new(cyan, magenta, yellow, black)),
                    );
                    Color::Special(SpecialColor::Separation(separation::Color::new(255, space)))
                        .into()
                }
                None => Colour::Rgb(0, 0, 0).krilla().into(),
            },
            Paint::Gradient(index) => match self.gradients.get(index) {
                Some(gradient) => gradient.krilla(),
                None => Colour::Rgb(0, 0, 0).krilla().into(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StrokeStyle {
    pub width: f32,
    pub cap: LineCap,
    pub join: LineJoin,
    pub miter_limit: f32,
    pub dash: Option<(Vec<f32>, f32)>,
}

impl Default for StrokeStyle {
    fn default() -> Self {
        Self {
            width: 1.0,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter_limit: 10.0,
            dash: None,
        }
    }
}

impl StrokeStyle {
    pub fn krilla_dash(&self) -> Option<StrokeDash> {
        self.dash.as_ref().map(|(array, offset)| StrokeDash {
            array: array.clone(),
            offset: *offset,
        })
    }
}

/// A clip, already in page space.
#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    pub segments: Vec<Segment>,
    pub rule: FillRule,
}

/// The segments with `matrix` applied to every point.
pub fn transform_segments(segments: &[Segment], matrix: Matrix) -> Vec<Segment> {
    let [a, b, c, d, e, f] = matrix;
    let map = |x: f32, y: f32| (a * x + c * y + e, b * x + d * y + f);
    segments
        .iter()
        .map(|segment| match *segment {
            Segment::Move(x, y) => {
                let (x, y) = map(x, y);
                Segment::Move(x, y)
            }
            Segment::Line(x, y) => {
                let (x, y) = map(x, y);
                Segment::Line(x, y)
            }
            Segment::Cubic(x1, y1, x2, y2, x, y) => {
                let (x1, y1) = map(x1, y1);
                let (x2, y2) = map(x2, y2);
                let (x, y) = map(x, y);
                Segment::Cubic(x1, y1, x2, y2, x, y)
            }
            Segment::Quad(x1, y1, x, y) => {
                let (x1, y1) = map(x1, y1);
                let (x, y) = map(x, y);
                Segment::Quad(x1, y1, x, y)
            }
            Segment::Close => Segment::Close,
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct GraphicsState {
    pub ctm: Matrix,
    pub fill: Paint,
    pub fill_opacity: f32,
    pub stroke: Paint,
    pub stroke_opacity: f32,
    pub stroke_style: StrokeStyle,
    pub clips: Arc<Vec<Clip>>,
}

impl Default for GraphicsState {
    fn default() -> Self {
        Self {
            ctm: IDENTITY,
            fill: Paint::BLACK,
            fill_opacity: 1.0,
            stroke: Paint::BLACK,
            stroke_opacity: 1.0,
            stroke_style: StrokeStyle::default(),
            clips: Arc::new(Vec::new()),
        }
    }
}

/// A path construction command, as pdfkit's path operators take them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Segment {
    Move(f32, f32),
    Line(f32, f32),
    Cubic(f32, f32, f32, f32, f32, f32),
    Quad(f32, f32, f32, f32),
    Close,
}

/// At most this many segments in one path.
pub const MAX_SEGMENTS: usize = 1_000_000;

/// The path, or `None` when it draws nothing.
pub fn build_path(segments: &[Segment]) -> Result<Option<Path>, ComposeError> {
    if segments.len() > MAX_SEGMENTS {
        return Err(ComposeError::new(
            "INVALID_OPTIONS",
            format!(
                "a path of {} segments is over {MAX_SEGMENTS}",
                segments.len()
            ),
        ));
    }
    let mut builder = PathBuilder::new();
    let mut open = false;
    for segment in segments {
        let finite = match *segment {
            Segment::Move(x, y) | Segment::Line(x, y) => x.is_finite() && y.is_finite(),
            Segment::Cubic(a, b, c, d, e, f) => [a, b, c, d, e, f].iter().all(|v| v.is_finite()),
            Segment::Quad(a, b, c, d) => [a, b, c, d].iter().all(|v| v.is_finite()),
            Segment::Close => true,
        };
        if !finite {
            return Err(ComposeError::new(
                "INVALID_OPTIONS",
                "path coordinates must be finite numbers",
            ));
        }
        match *segment {
            Segment::Move(x, y) => {
                builder.move_to(x, y);
                open = true;
            }
            // A line or curve with no current point starts one, as PDF
            // readers do with a path that does not begin with a move.
            Segment::Line(x, y) => {
                if !open {
                    builder.move_to(x, y);
                    open = true;
                } else {
                    builder.line_to(x, y);
                }
            }
            Segment::Cubic(x1, y1, x2, y2, x, y) => {
                if !open {
                    builder.move_to(x1, y1);
                    open = true;
                }
                builder.cubic_to(x1, y1, x2, y2, x, y);
            }
            Segment::Quad(x1, y1, x, y) => {
                if !open {
                    builder.move_to(x1, y1);
                    open = true;
                }
                builder.quad_to(x1, y1, x, y);
            }
            Segment::Close => builder.close(),
        }
    }
    Ok(builder.finish())
}

pub fn opacity(value: f32) -> f32 {
    if value.is_nan() {
        1.0
    } else {
        value.clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concat_applies_the_inner_matrix_first() {
        let translate = [1.0, 0.0, 0.0, 1.0, 10.0, 20.0];
        let scale = [2.0, 0.0, 0.0, 2.0, 0.0, 0.0];
        // pdfkit: translate(10, 20) then scale(2) maps (1, 1) to (12, 22).
        let [a, b, c, d, e, f] = concat(translate, scale);
        assert_eq!((a + c + e, b + d + f), (12.0, 22.0));
    }

    #[test]
    fn a_gradient_keeps_to_one_colour_space() {
        let gradient = |colours: [Colour; 2]| Gradient {
            shape: GradientShape::Linear {
                x1: 0.0,
                y1: 0.0,
                x2: 100.0,
                y2: 0.0,
            },
            stops: colours
                .iter()
                .enumerate()
                .map(|(index, &colour)| GradientStop {
                    offset: index as f32,
                    colour,
                    opacity: 1.0,
                })
                .collect(),
            transform: IDENTITY,
        };
        assert!(gradient([Colour::Rgb(255, 0, 0), Colour::Rgb(0, 0, 255)])
            .check()
            .is_ok());
        assert!(
            gradient([Colour::Rgb(255, 0, 0), Colour::Cmyk(0, 0, 0, 255)])
                .check()
                .is_err()
        );
    }

    #[test]
    fn refuses_a_path_that_is_not_finite() {
        assert!(build_path(&[Segment::Move(0.0, 0.0), Segment::Line(f32::NAN, 1.0)]).is_err());
        assert!(build_path(&[]).unwrap().is_none());
    }
}
