//! The document being authored, held on the Rust side between calls.
//!
//! Every call is synchronous: a layout answers at once, because the caller
//! places the next block where the last one ended. A failure comes back as
//! `CODE: reason`, which the TypeScript side turns into a coded error.

use std::panic::{catch_unwind, AssertUnwindSafe};

use napi::bindgen_prelude::*;
use napi_derive::napi;
use vellum_engine::compose::graphics::{
    Colour, FillRule, Gradient, GradientShape, GradientStop, LineCap, LineJoin, Matrix, Paint,
    Segment,
};
use vellum_engine::compose::links::LinkTarget;
use vellum_engine::compose::text::{Align, Baseline, Step, TextOptions};
use vellum_engine::compose::{
    ComposeError, Composer, DocumentSettings, Info, Instant, PageSetup, Sides,
};

#[napi(object)]
pub struct DocumentInfoInput {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    pub creator: Option<String>,
    pub producer: Option<String>,
    /// Milliseconds since the Unix epoch.
    pub creation_date: Option<f64>,
    pub mod_date: Option<f64>,
    /// Any other key, as `[key, value]` pairs.
    pub custom: Option<Vec<Vec<String>>>,
}

#[napi(object)]
pub struct DocumentSettingsInput {
    pub lang: Option<String>,
    pub display_title: Option<bool>,
    pub page_layout: Option<String>,
    pub pdf_version: Option<String>,
    pub compress: Option<bool>,
    pub buffer_pages: Option<bool>,
}

#[napi(object)]
pub struct PageSetupInput {
    pub width: f64,
    pub height: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
    pub user_unit: Option<f64>,
}

#[napi(object)]
pub struct ImageInfo {
    pub handle: u32,
    /// Pixels, as stored.
    pub width: u32,
    pub height: u32,
    /// EXIF orientation, 1 to 8.
    pub orientation: u32,
}

#[napi(object)]
pub struct TextBounds {
    pub width: f64,
    pub height: f64,
}

#[napi(object)]
pub struct PageRange {
    pub start: u32,
    pub count: u32,
}

/// `text()`'s options once the caller has resolved them.
#[napi(object)]
pub struct TextOptionsInput {
    pub width: Option<f64>,
    pub height: Option<f64>,
    /// `left`, `center`, `right`, `justify`; absent follows the paragraph's
    /// direction.
    pub align: Option<String>,
    pub line_gap: Option<f64>,
    pub paragraph_gap: Option<f64>,
    pub indent: Option<f64>,
    pub indent_all_lines: Option<bool>,
    pub columns: Option<u32>,
    pub column_gap: Option<f64>,
    pub ellipsis: Option<String>,
    pub continued: Option<bool>,
    pub character_spacing: Option<f64>,
    pub word_spacing: Option<f64>,
    pub horizontal_scaling: Option<f64>,
    pub features: Option<Vec<String>>,
    /// A named baseline: `top`, `alphabetic`, `middle`, `svg-middle`…
    pub baseline: Option<String>,
    /// A numeric baseline, in points.
    pub baseline_offset: Option<f64>,
    /// Degrees, counter-clockwise.
    pub rotation: Option<f64>,
    /// The skew of a fake italic.
    pub oblique: Option<f64>,
    pub fill: Option<bool>,
    pub stroke: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    /// A URL to link the text to.
    pub link: Option<String>,
    /// A page to link the text to, from zero.
    pub link_page: Option<u32>,
    pub go_to: Option<String>,
    pub destination: Option<String>,
}

fn failure(error: ComposeError) -> Error {
    Error::from_reason(error.to_string())
}

fn options_error(message: impl Into<String>) -> Error {
    failure(ComposeError::new("INVALID_OPTIONS", message))
}

fn finite(value: f64, name: &str) -> Result<f32> {
    if value.is_finite() {
        Ok(value as f32)
    } else {
        Err(options_error(format!("{name} must be a finite number")))
    }
}

fn optional(value: Option<f64>, name: &str) -> Result<Option<f32>> {
    value.map(|value| finite(value, name)).transpose()
}

fn align(name: Option<&str>) -> Result<Align> {
    match name {
        None => Ok(Align::Start),
        Some("left") => Ok(Align::Left),
        Some("center") => Ok(Align::Center),
        Some("right") => Ok(Align::Right),
        Some("justify") => Ok(Align::Justify),
        Some(other) => Err(options_error(format!(
            "align {other:?} is not one of left, center, right, justify"
        ))),
    }
}

fn baseline(name: Option<&str>, offset: Option<f64>) -> Result<Baseline> {
    if let Some(offset) = offset {
        return Ok(Baseline::Offset(finite(offset, "baseline")?));
    }
    match name {
        None | Some("top") => Ok(Baseline::Top),
        Some("alphabetic") => Ok(Baseline::Alphabetic),
        Some("middle") | Some("svg-central") => Ok(Baseline::Middle),
        Some("svg-middle") => Ok(Baseline::SvgMiddle),
        Some("bottom") | Some("ideographic") => Ok(Baseline::Bottom),
        Some("mathematical") => Ok(Baseline::Mathematical),
        Some("hanging") => Ok(Baseline::Hanging),
        Some(other) => Err(options_error(format!(
            "baseline {other:?} is not one pdfkit knows"
        ))),
    }
}

impl TryFrom<DocumentInfoInput> for Info {
    type Error = Error;

    fn try_from(info: DocumentInfoInput) -> Result<Self> {
        let custom = info
            .custom
            .unwrap_or_default()
            .into_iter()
            .map(|pair| match <[String; 2]>::try_from(pair) {
                Ok([key, value]) => Ok((key, value)),
                Err(_) => Err(options_error("an info entry is a [key, value] pair")),
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Info {
            title: info.title,
            author: info.author,
            subject: info.subject,
            keywords: info.keywords,
            creator: info.creator,
            producer: info.producer,
            creation_date: info.creation_date.map(|unix_ms| Instant { unix_ms }),
            mod_date: info.mod_date.map(|unix_ms| Instant { unix_ms }),
            custom,
        })
    }
}

impl TryFrom<TextOptionsInput> for TextOptions {
    type Error = Error;

    fn try_from(input: TextOptionsInput) -> Result<Self> {
        let horizontal_scaling =
            optional(input.horizontal_scaling, "horizontalScaling")?.unwrap_or(100.0);
        if !(horizontal_scaling > 0.0 && horizontal_scaling <= 1000.0) {
            return Err(options_error(
                "horizontalScaling must be above 0 and at most 1000",
            ));
        }
        let columns = input.columns.unwrap_or(1);
        if columns > 64 {
            return Err(options_error("columns must be at most 64"));
        }
        Ok(TextOptions {
            width: optional(input.width, "width")?,
            height: optional(input.height, "height")?,
            align: align(input.align.as_deref())?,
            line_gap: optional(input.line_gap, "lineGap")?,
            paragraph_gap: optional(input.paragraph_gap, "paragraphGap")?.unwrap_or(0.0),
            indent: optional(input.indent, "indent")?,
            indent_all_lines: input.indent_all_lines.unwrap_or(false),
            columns,
            column_gap: optional(input.column_gap, "columnGap")?.unwrap_or(18.0),
            ellipsis: input.ellipsis,
            continued: input.continued.unwrap_or(false),
            character_spacing: optional(input.character_spacing, "characterSpacing")?,
            word_spacing: optional(input.word_spacing, "wordSpacing")?,
            horizontal_scaling,
            features: input.features,
            baseline: baseline(input.baseline.as_deref(), input.baseline_offset)?,
            rotation: optional(input.rotation, "rotation")?.unwrap_or(0.0),
            oblique: optional(input.oblique, "oblique")?,
            fill: input.fill.unwrap_or(false),
            stroke: input.stroke.unwrap_or(false),
            underline: input.underline.unwrap_or(false),
            strike: input.strike.unwrap_or(false),
            link: match (input.link, input.link_page) {
                (Some(uri), _) => Some(LinkTarget::Uri(uri)),
                (None, Some(page)) => Some(LinkTarget::Page(page as usize)),
                (None, None) => None,
            },
            go_to: input.go_to,
            destination: input.destination,
        })
    }
}

fn step(step: Step) -> &'static str {
    match step {
        Step::Done => "done",
        Step::NeedPage => "needPage",
    }
}

#[napi]
pub struct NativeDocument {
    inner: Composer,
    /// Set once a call panicked: the state it left cannot be trusted.
    broken: bool,
}

impl NativeDocument {
    fn with<T>(&mut self, work: impl FnOnce(&mut Composer) -> Result<T>) -> Result<T> {
        if self.broken {
            return Err(failure(ComposeError::new(
                "DOCUMENT_BROKEN",
                "an earlier call failed inside the engine; start a new document",
            )));
        }
        match catch_unwind(AssertUnwindSafe(|| work(&mut self.inner))) {
            Ok(result) => result,
            Err(_) => {
                self.broken = true;
                Err(failure(ComposeError::new(
                    "INTERNAL",
                    "internal panic in the vellum engine",
                )))
            }
        }
    }
}

#[napi]
impl NativeDocument {
    #[napi(constructor)]
    pub fn new(settings: DocumentSettingsInput) -> Result<Self> {
        let defaults = DocumentSettings::default();
        let inner = Composer::new(DocumentSettings {
            info: Info::default(),
            lang: settings.lang,
            display_title: settings.display_title.unwrap_or(false),
            page_layout: settings.page_layout,
            pdf_version: settings.pdf_version,
            compress: settings.compress.unwrap_or(defaults.compress),
            buffer_pages: settings.buffer_pages.unwrap_or(false),
        })
        .map_err(failure)?;
        Ok(Self {
            inner,
            broken: false,
        })
    }

    #[napi]
    pub fn add_page(&mut self, setup: PageSetupInput) -> Result<()> {
        let setup = PageSetup {
            width: finite(setup.width, "width")?,
            height: finite(setup.height, "height")?,
            margins: Sides {
                top: finite(setup.top, "margin")?,
                right: finite(setup.right, "margin")?,
                bottom: finite(setup.bottom, "margin")?,
                left: finite(setup.left, "margin")?,
            },
            user_unit: optional(setup.user_unit, "userUnit")?.unwrap_or(1.0),
        };
        self.with(|document| document.add_page(setup).map_err(failure))
    }

    #[napi]
    pub fn set_margins(
        &mut self,
        page: u32,
        top: f64,
        right: f64,
        bottom: f64,
        left: f64,
    ) -> Result<()> {
        let margins = Sides {
            top: finite(top, "margin")?,
            right: finite(right, "margin")?,
            bottom: finite(bottom, "margin")?,
            left: finite(left, "margin")?,
        };
        self.with(|document| {
            document
                .set_margins(page as usize, margins)
                .map_err(failure)
        })
    }

    #[napi]
    pub fn get_x(&self) -> f64 {
        f64::from(self.inner.x)
    }

    #[napi]
    pub fn set_x(&mut self, x: f64) -> Result<()> {
        self.inner.x = finite(x, "x")?;
        Ok(())
    }

    #[napi]
    pub fn get_y(&self) -> f64 {
        f64::from(self.inner.y)
    }

    #[napi]
    pub fn set_y(&mut self, y: f64) -> Result<()> {
        self.inner.y = finite(y, "y")?;
        Ok(())
    }

    #[napi]
    pub fn buffered_page_range(&self) -> PageRange {
        let (start, count) = self.inner.buffered_page_range();
        PageRange {
            start: start as u32,
            count: count as u32,
        }
    }

    /// The page drawing goes to, counted from zero.
    #[napi]
    pub fn current_page(&self) -> Option<u32> {
        self.inner.current_page_index().map(|index| index as u32)
    }

    #[napi]
    pub fn switch_to_page(&mut self, index: u32) -> Result<()> {
        self.with(|document| document.switch_to_page(index as usize).map_err(failure))
    }

    #[napi]
    pub fn flush_pages(&mut self) -> Result<()> {
        self.with(|document| document.flush_pages().map_err(failure))
    }

    /// Load a font and answer its handle; `standard` names the standard
    /// face the bytes stand in for.
    #[napi]
    pub fn load_font(
        &mut self,
        bytes: Buffer,
        family: Option<String>,
        standard: Option<String>,
    ) -> Result<u32> {
        let bytes = bytes.to_vec();
        self.with(|document| {
            document
                .load_font(bytes, family.as_deref(), standard.as_deref())
                .map(|index| index as u32)
                .map_err(failure)
        })
    }

    #[napi]
    pub fn set_font(&mut self, handle: u32) -> Result<()> {
        self.with(|document| document.set_font(handle as usize).map_err(failure))
    }

    #[napi]
    pub fn get_font_size(&self) -> f64 {
        f64::from(self.inner.font_size())
    }

    #[napi]
    pub fn set_font_size(&mut self, size: f64) -> Result<()> {
        let size = finite(size, "fontSize")?;
        self.with(|document| document.set_font_size(size).map_err(failure))
    }

    #[napi]
    pub fn get_line_gap(&self) -> f64 {
        f64::from(self.inner.line_gap)
    }

    #[napi]
    pub fn set_line_gap(&mut self, gap: f64) -> Result<()> {
        self.inner.line_gap = finite(gap, "lineGap")?;
        Ok(())
    }

    /// The current font's ascender, in thousandths of an em.
    #[napi]
    pub fn ascender(&mut self) -> Result<f64> {
        self.with(|document| {
            document
                .current_font()
                .map(|font| f64::from(font.vertical.ascender))
                .map_err(failure)
        })
    }

    #[napi]
    pub fn current_line_height(&mut self, include_gap: Option<bool>) -> Result<f64> {
        let include_gap = include_gap.unwrap_or(false);
        self.with(|document| {
            document
                .current_line_height(include_gap)
                .map(f64::from)
                .map_err(failure)
        })
    }

    #[napi]
    pub fn width_of_string(
        &mut self,
        text: String,
        character_spacing: Option<f64>,
        horizontal_scaling: Option<f64>,
        features: Option<Vec<String>>,
    ) -> Result<f64> {
        let spacing = optional(character_spacing, "characterSpacing")?.unwrap_or(0.0);
        let scaling = optional(horizontal_scaling, "horizontalScaling")?.unwrap_or(100.0);
        self.with(|document| {
            let features =
                vellum_engine::compose::fonts::parse_features(&features.unwrap_or_default())
                    .map_err(failure)?;
            document
                .width_of_string(&text, spacing, scaling, &features)
                .map(f64::from)
                .map_err(failure)
        })
    }

    /// `done`, or `needPage`: add a page, then [`Self::resume_text`].
    #[napi]
    pub fn text(&mut self, text: String, options: TextOptionsInput) -> Result<&'static str> {
        let options = TextOptions::try_from(options)?;
        self.with(|document| document.text(&text, &options).map(step).map_err(failure))
    }

    #[napi]
    pub fn resume_text(&mut self) -> Result<&'static str> {
        self.with(|document| document.resume_text().map(step).map_err(failure))
    }

    #[napi]
    pub fn height_of_string(&mut self, text: String, options: TextOptionsInput) -> Result<f64> {
        let options = TextOptions::try_from(options)?;
        self.with(|document| {
            document
                .height_of_string(&text, &options)
                .map(f64::from)
                .map_err(failure)
        })
    }

    /// Width and height of what `text()` would draw, before any rotation.
    #[napi]
    pub fn bounds_of_string(
        &mut self,
        text: String,
        options: TextOptionsInput,
    ) -> Result<TextBounds> {
        let options = TextOptions::try_from(options)?;
        self.with(|document| {
            document
                .bounds_of_string(&text, &options)
                .map(|(width, height)| TextBounds {
                    width: f64::from(width),
                    height: f64::from(height),
                })
                .map_err(failure)
        })
    }

    #[napi]
    pub fn move_down(&mut self, lines: f64) -> Result<()> {
        let lines = finite(lines, "lines")?;
        self.with(|document| document.move_down(lines).map_err(failure))
    }

    #[napi]
    pub fn move_up(&mut self, lines: f64) -> Result<()> {
        let lines = finite(lines, "lines")?;
        self.with(|document| document.move_up(lines).map_err(failure))
    }

    #[napi]
    pub fn set_info(&mut self, info: DocumentInfoInput) -> Result<()> {
        let info = Info::try_from(info)?;
        self.with(|document| document.set_info(info).map_err(failure))
    }

    #[napi]
    pub fn save(&mut self) -> Result<()> {
        self.with(|document| document.save().map_err(failure))
    }

    #[napi]
    pub fn restore(&mut self) -> Result<()> {
        self.with(|document| document.restore().map_err(failure))
    }

    #[napi]
    pub fn transform(&mut self, values: Vec<f64>) -> Result<()> {
        let matrix = matrix(&values)?;
        self.with(|document| document.transform(matrix).map_err(failure))
    }

    #[napi]
    pub fn get_ctm(&self) -> Vec<f64> {
        self.inner
            .ctm()
            .iter()
            .map(|value| f64::from(*value))
            .collect()
    }

    #[napi]
    pub fn set_fill_paint(&mut self, input: PaintInput) -> Result<()> {
        let paint = paint(input)?;
        self.with(|document| document.set_fill(paint).map_err(failure))
    }

    #[napi]
    pub fn set_stroke_paint(&mut self, input: PaintInput) -> Result<()> {
        let paint = paint(input)?;
        self.with(|document| document.set_stroke(paint).map_err(failure))
    }

    #[napi]
    pub fn set_fill_opacity(&mut self, value: f64) {
        self.inner.set_fill_opacity(value as f32);
    }

    #[napi]
    pub fn set_stroke_opacity(&mut self, value: f64) {
        self.inner.set_stroke_opacity(value as f32);
    }

    #[napi]
    pub fn set_line_width(&mut self, width: f64) -> Result<()> {
        let width = finite(width, "lineWidth")?;
        self.with(|document| document.set_line_width(width).map_err(failure))
    }

    /// `butt`, `round`, `square`.
    #[napi]
    pub fn set_line_cap(&mut self, cap: String) -> Result<()> {
        let cap = match cap.as_str() {
            "butt" => LineCap::Butt,
            "round" => LineCap::Round,
            "square" => LineCap::Square,
            other => return Err(options_error(format!("{other:?} is not a line cap"))),
        };
        self.inner.set_line_cap(cap);
        Ok(())
    }

    /// `miter`, `round`, `bevel`.
    #[napi]
    pub fn set_line_join(&mut self, join: String) -> Result<()> {
        let join = match join.as_str() {
            "miter" => LineJoin::Miter,
            "round" => LineJoin::Round,
            "bevel" => LineJoin::Bevel,
            other => return Err(options_error(format!("{other:?} is not a line join"))),
        };
        self.inner.set_line_join(join);
        Ok(())
    }

    #[napi]
    pub fn set_miter_limit(&mut self, limit: f64) -> Result<()> {
        let limit = finite(limit, "miterLimit")?;
        self.with(|document| document.set_miter_limit(limit).map_err(failure))
    }

    /// No lengths: a solid line.
    #[napi]
    pub fn set_dash(&mut self, lengths: Option<Vec<f64>>, phase: Option<f64>) -> Result<()> {
        let dash = match lengths {
            None => None,
            Some(lengths) => Some((
                lengths
                    .iter()
                    .map(|length| finite(*length, "dash"))
                    .collect::<Result<Vec<_>>>()?,
                optional(phase, "phase")?.unwrap_or(0.0),
            )),
        };
        self.with(|document| document.set_dash(dash).map_err(failure))
    }

    #[napi]
    pub fn add_gradient(&mut self, input: GradientInput) -> Result<u32> {
        let coords = input
            .coords
            .iter()
            .map(|value| finite(*value, "gradient coordinates"))
            .collect::<Result<Vec<_>>>()?;
        let shape = match (input.kind.as_str(), coords.as_slice()) {
            ("linear", &[x1, y1, x2, y2]) => GradientShape::Linear { x1, y1, x2, y2 },
            ("radial", &[x1, y1, r1, x2, y2, r2]) => GradientShape::Radial {
                x1,
                y1,
                r1,
                x2,
                y2,
                r2,
            },
            (kind, _) => return Err(options_error(format!("{kind:?} is not a gradient"))),
        };
        let stops = input
            .stops
            .into_iter()
            .map(|stop| {
                Ok(GradientStop {
                    offset: finite(stop.offset, "a stop's position")?,
                    colour: colour(&stop.kind, &stop.values)?,
                    opacity: finite(stop.opacity, "a stop's opacity")?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let gradient = Gradient {
            shape,
            stops,
            transform: matrix(&input.transform)?,
        };
        self.with(|document| {
            document
                .add_gradient(gradient)
                .map(|index| index as u32)
                .map_err(failure)
        })
    }

    /// A spot colour falling back to CMYK (each 0–255).
    #[napi]
    pub fn add_spot_colour(&mut self, name: String, cmyk: Vec<f64>) -> Result<u32> {
        let Colour::Cmyk(cyan, magenta, yellow, black) = colour("cmyk", &cmyk)? else {
            return Err(options_error("a spot colour falls back to CMYK"));
        };
        self.with(|document| {
            document
                .add_spot(&name, [cyan, magenta, yellow, black])
                .map(|index| index as u32)
                .map_err(failure)
        })
    }

    /// `fill`: a fill rule, or absent to only stroke.
    #[napi]
    pub fn paint_path(
        &mut self,
        verbs: String,
        coords: Vec<f64>,
        fill: Option<String>,
        stroke: bool,
    ) -> Result<()> {
        let path = segments(&verbs, &coords)?;
        let rule = fill
            .as_deref()
            .map(|rule| fill_rule(Some(rule)))
            .transpose()?;
        self.with(|document| document.paint_path(&path, rule, stroke).map_err(failure))
    }

    #[napi]
    pub fn clip_path(
        &mut self,
        verbs: String,
        coords: Vec<f64>,
        rule: Option<String>,
    ) -> Result<()> {
        let path = segments(&verbs, &coords)?;
        let rule = fill_rule(rule.as_deref())?;
        self.with(|document| document.clip(&path, rule).map_err(failure))
    }

    /// A link over a rectangle: to a URL, a page (from zero), or a named
    /// destination.
    #[napi]
    #[allow(clippy::too_many_arguments)]
    pub fn link(
        &mut self,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        url: Option<String>,
        page: Option<u32>,
        name: Option<String>,
    ) -> Result<()> {
        let target = match (url, page, name) {
            (Some(url), None, None) => LinkTarget::Uri(url),
            (None, Some(page), None) => LinkTarget::Page(page as usize),
            (None, None, Some(name)) => LinkTarget::Named(name),
            _ => return Err(options_error("a link goes to one URL, page or destination")),
        };
        let (x, y) = (finite(x, "x")?, finite(y, "y")?);
        let (width, height) = (finite(width, "width")?, finite(height, "height")?);
        self.with(|document| document.link(x, y, width, height, target).map_err(failure))
    }

    #[napi]
    pub fn add_named_destination(&mut self, name: String, x: f64, y: f64) -> Result<()> {
        let (x, y) = (finite(x, "x")?, finite(y, "y")?);
        self.with(|document| document.add_named_destination(&name, x, y).map_err(failure))
    }

    #[napi]
    pub fn load_image(&mut self, bytes: Buffer) -> Result<ImageInfo> {
        let bytes = bytes.to_vec();
        self.with(|document| {
            let handle = document.load_image(bytes).map_err(failure)?;
            let image = document.image(handle).map_err(failure)?;
            Ok(ImageInfo {
                handle: handle as u32,
                width: image.width,
                height: image.height,
                orientation: u32::from(image.orientation),
            })
        })
    }

    /// Draw a loaded image into a box, turned by an EXIF orientation.
    #[napi]
    #[allow(clippy::too_many_arguments)]
    pub fn draw_image(
        &mut self,
        handle: u32,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        orientation: u32,
        opacity: Option<f64>,
    ) -> Result<()> {
        let (x, y) = (finite(x, "x")?, finite(y, "y")?);
        let (width, height) = (finite(width, "width")?, finite(height, "height")?);
        let opacity = optional(opacity, "opacity")?.unwrap_or(1.0);
        let orientation = u8::try_from(orientation)
            .ok()
            .filter(|value| (1..=8).contains(value))
            .ok_or_else(|| options_error("an orientation is 1 to 8"))?;
        self.with(|document| {
            document
                .draw_image(handle as usize, x, y, width, height, orientation, opacity)
                .map_err(failure)
        })
    }

    /// Write the document. Nothing can be added afterwards.
    #[napi]
    pub fn end(&mut self) -> Result<Buffer> {
        self.with(|document| document.end().map(Buffer::from).map_err(failure))
    }
}

/// The file in the package's `fonts/` directory a standard name stands for.
#[napi]
pub fn standard_font_file(name: String) -> Option<String> {
    vellum_engine::compose::standard::standard(&name).map(|face| face.file.to_string())
}

/// A colour or paint: `rgb` (0–255 each), `cmyk` (0–255 each), or a
/// `spot` / `gradient` handle.
#[napi(object)]
pub struct PaintInput {
    pub kind: String,
    pub values: Option<Vec<f64>>,
    pub handle: Option<u32>,
}

#[napi(object)]
pub struct GradientStopInput {
    pub offset: f64,
    /// `rgb` or `cmyk`.
    pub kind: String,
    pub values: Vec<f64>,
    pub opacity: f64,
}

#[napi(object)]
pub struct GradientInput {
    /// `linear` (`x1 y1 x2 y2`) or `radial` (`x1 y1 r1 x2 y2 r2`).
    pub kind: String,
    pub coords: Vec<f64>,
    pub stops: Vec<GradientStopInput>,
    pub transform: Vec<f64>,
}

fn byte(value: f64, name: &str) -> Result<u8> {
    if value.is_finite() && (0.0..=255.0).contains(&value) {
        Ok(value.round() as u8)
    } else {
        Err(options_error(format!(
            "{name} components are 0 to 255, got {value}"
        )))
    }
}

fn colour(kind: &str, values: &[f64]) -> Result<Colour> {
    match (kind, values) {
        ("rgb", [red, green, blue]) => Ok(Colour::Rgb(
            byte(*red, "rgb")?,
            byte(*green, "rgb")?,
            byte(*blue, "rgb")?,
        )),
        ("cmyk", [cyan, magenta, yellow, black]) => Ok(Colour::Cmyk(
            byte(*cyan, "cmyk")?,
            byte(*magenta, "cmyk")?,
            byte(*yellow, "cmyk")?,
            byte(*black, "cmyk")?,
        )),
        _ => Err(options_error(format!(
            "{kind:?} with {} values is not a colour",
            values.len()
        ))),
    }
}

fn paint(input: PaintInput) -> Result<Paint> {
    match (input.kind.as_str(), input.handle) {
        ("spot", Some(handle)) => Ok(Paint::Spot(handle as usize)),
        ("gradient", Some(handle)) => Ok(Paint::Gradient(handle as usize)),
        (kind, _) => colour(kind, &input.values.unwrap_or_default()).map(Paint::Colour),
    }
}

fn matrix(values: &[f64]) -> Result<Matrix> {
    let [a, b, c, d, e, f] =
        <[f64; 6]>::try_from(values).map_err(|_| options_error("a transform has six numbers"))?;
    Ok([
        finite(a, "transform")?,
        finite(b, "transform")?,
        finite(c, "transform")?,
        finite(d, "transform")?,
        finite(e, "transform")?,
        finite(f, "transform")?,
    ])
}

/// A path as verbs (`M L C Q Z`) and the numbers they take, in order.
fn segments(verbs: &str, coords: &[f64]) -> Result<Vec<Segment>> {
    let mut numbers = coords.iter();
    let mut take = |count: usize| -> Result<Vec<f32>> {
        (0..count)
            .map(|_| {
                numbers
                    .next()
                    .ok_or_else(|| options_error("a path has fewer numbers than its verbs take"))
                    .and_then(|value| finite(*value, "path coordinates"))
            })
            .collect()
    };
    let mut out = Vec::with_capacity(verbs.len());
    for verb in verbs.chars() {
        out.push(match verb {
            'M' => {
                let p = take(2)?;
                Segment::Move(p[0], p[1])
            }
            'L' => {
                let p = take(2)?;
                Segment::Line(p[0], p[1])
            }
            'C' => {
                let p = take(6)?;
                Segment::Cubic(p[0], p[1], p[2], p[3], p[4], p[5])
            }
            'Q' => {
                let p = take(4)?;
                Segment::Quad(p[0], p[1], p[2], p[3])
            }
            'Z' => Segment::Close,
            other => return Err(options_error(format!("{other:?} is not a path verb"))),
        });
    }
    if numbers.next().is_some() {
        return Err(options_error("a path has more numbers than its verbs take"));
    }
    Ok(out)
}

fn fill_rule(rule: Option<&str>) -> Result<FillRule> {
    match rule {
        None | Some("non-zero") | Some("nonzero") => Ok(FillRule::NonZero),
        Some("even-odd") | Some("evenodd") => Ok(FillRule::EvenOdd),
        Some(other) => Err(options_error(format!("{other:?} is not a fill rule"))),
    }
}
