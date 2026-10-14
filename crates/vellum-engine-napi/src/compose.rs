//! The document being authored, held on the Rust side between calls.
//!
//! Every call is synchronous: a layout answers at once, because the caller
//! places the next block where the last one ended. A failure comes back as
//! `CODE: reason`, which the TypeScript side turns into a coded error.

use std::panic::{catch_unwind, AssertUnwindSafe};

use napi::bindgen_prelude::*;
use napi_derive::napi;
use vellum_engine::compose::annotations::{AnnotationKind, Attachment, Fit, Relationship};
use vellum_engine::compose::forms::{Encryption, FieldSpec, FieldType, Format, Rights};
use vellum_engine::compose::graphics::{
    Colour, FillRule, Gradient, GradientShape, GradientStop, LineCap, LineJoin, Matrix, Paint,
    Segment,
};
use vellum_engine::compose::links::{LinkTarget, OutlineItem};
use vellum_engine::compose::tagging::{ArtifactKind, Child, Element, Mark};
use vellum_engine::compose::text::{Align, Baseline, ListItem, ListMarker, Step, TextOptions};
use vellum_engine::compose::{
    ComposeError, Composer, DocumentSettings, Info, Instant, PageSetup, Sides, Subset,
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
    pub tagged: Option<bool>,
    /// `PDF/A-1b`, `PDF/A-2a`, `PDF/UA`…
    pub subset: Option<String>,
    pub lang: Option<String>,
    pub display_title: Option<bool>,
    pub page_layout: Option<String>,
    pub pdf_version: Option<String>,
    pub compress: Option<bool>,
    pub buffer_pages: Option<bool>,
    /// The largest file `file()` embeds, in bytes. Default 64 MiB.
    pub max_file_bytes: Option<u32>,
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

/// An item of `list()`: a bullet (`radius`, `midLine`) or a `label`.
#[napi(object)]
pub struct ListItemInput {
    pub level: u32,
    pub radius: Option<f64>,
    pub mid_line: Option<f64>,
    pub label: Option<String>,
    pub indent: f64,
    pub item_indent: f64,
}

/// What a marked sequence is: `artifact` (with `type`), `span`, or
/// `structure`.
#[napi(object)]
pub struct MarkInput {
    pub kind: String,
    /// An artifact's type: `Pagination`, `Layout`, `Page`, `Header`…
    #[napi(js_name = "type")]
    pub artifact: Option<String>,
    pub lang: Option<String>,
    pub alt: Option<String>,
    pub expanded: Option<String>,
    pub actual: Option<String>,
}

/// An outline entry; `parent` is the index of its parent entry, absent at
/// the top.
#[napi(object)]
pub struct OutlineInput {
    pub title: String,
    pub page: u32,
    pub x: f64,
    pub y: f64,
    pub open: bool,
    pub parent: Option<u32>,
}

/// One structure element; `parent` is the index of its parent among the
/// elements, or absent for a root.
#[napi(object)]
pub struct StructureNodeInput {
    pub kind: String,
    pub title: Option<String>,
    pub lang: Option<String>,
    pub alt: Option<String>,
    pub expanded: Option<String>,
    pub actual: Option<String>,
    pub scope: Option<String>,
    pub row_span: Option<u32>,
    pub col_span: Option<u32>,
}

/// A child of element `parent`, in order: an element, marked content, or a
/// link.
#[napi(object)]
pub struct StructureChildInput {
    pub parent: u32,
    pub node: Option<u32>,
    pub content: Option<u32>,
    pub annotation: Option<u32>,
}

/// A file to embed.
#[napi(object)]
pub struct FileInput {
    pub name: String,
    pub data: Buffer,
    pub mime: Option<String>,
    pub description: Option<String>,
    /// `Alternative`, `Data`, `Source`, `Supplement`, `Unspecified`.
    pub relationship: Option<String>,
    /// Milliseconds since the Unix epoch.
    pub created: Option<f64>,
    pub modified: Option<f64>,
    pub hidden: Option<bool>,
}

/// An annotation other than a link: `note`, `highlight`, `underline`,
/// `strike`, `line`, `square`, `circle`, `text`, `file`.
#[napi(object)]
pub struct AnnotationInput {
    pub kind: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub contents: Option<String>,
    /// `[r, g, b]`, 0 to 255.
    pub colour: Option<Vec<f64>>,
    /// A note's icon.
    pub icon: Option<String>,
    /// A line's end, the start being `x`, `y`.
    pub x2: Option<f64>,
    pub y2: Option<f64>,
    /// A file annotation's file.
    pub file: Option<u32>,
}

/// pdfkit's `format` of a form field.
#[napi(object)]
pub struct FormatInput {
    /// `zip`, `zipPlus4`, `zip4`, `phone`, `ssn`, `date`, `time`, `number`,
    /// `percent`.
    #[napi(js_name = "type")]
    pub kind: String,
    pub param: Option<String>,
    pub n_dec: Option<u32>,
    pub sep_comma: Option<bool>,
    pub neg_style: Option<String>,
    pub currency: Option<String>,
    pub currency_prepend: Option<bool>,
}

#[napi(object)]
pub struct FieldInput {
    pub name: String,
    /// `text`, `pushButton`, `radioButton`, `checkbox`, `combo`, `list`;
    /// absent groups fields.
    pub kind: Option<String>,
    pub parent: Option<u32>,
    pub flags: Option<u32>,
    /// `left`, `center`, `right`.
    pub align: Option<String>,
    pub value: Option<String>,
    pub default_value: Option<String>,
    pub choices: Option<Vec<String>>,
    pub label: Option<String>,
    pub background: Option<Vec<f64>>,
    pub border: Option<Vec<f64>>,
    pub font_size: Option<f64>,
    pub format: Option<FormatInput>,
    pub on_state: Option<String>,
}

#[napi(object)]
pub struct RectInput {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[napi(object)]
pub struct PermissionsInput {
    /// `lowResolution` or `highResolution`.
    pub printing: Option<String>,
    pub modifying: Option<bool>,
    pub copying: Option<bool>,
    pub annotating: Option<bool>,
    pub filling_forms: Option<bool>,
    pub content_accessibility: Option<bool>,
    pub document_assembly: Option<bool>,
}

#[napi(object)]
pub struct EncryptionInput {
    pub user_password: Option<String>,
    pub owner_password: Option<String>,
    pub permissions: Option<PermissionsInput>,
}

fn rgb(values: Option<Vec<f64>>, name: &str) -> Result<Option<[u8; 3]>> {
    match values.as_deref() {
        None => Ok(None),
        Some([red, green, blue]) => Ok(Some([
            byte(*red, name)?,
            byte(*green, name)?,
            byte(*blue, name)?,
        ])),
        Some(_) => Err(options_error(format!("{name} is [r, g, b]"))),
    }
}

fn format(input: FormatInput) -> Result<Format> {
    Ok(match input.kind.as_str() {
        "zip" => Format::Zip,
        "zipPlus4" | "zip4" => Format::ZipPlus4,
        "phone" => Format::Phone,
        "ssn" => Format::Ssn,
        "date" => Format::Date(input.param.unwrap_or_else(|| "dd/mm/yyyy".into())),
        "time" => Format::Time(
            input
                .param
                .as_deref()
                .unwrap_or("0")
                .parse()
                .map_err(|_| options_error("a time format's param is 0 to 3"))?,
        ),
        "number" => Format::Number {
            decimals: u8::try_from(input.n_dec.unwrap_or(0))
                .map_err(|_| options_error("nDec is too large"))?,
            separate_thousands: input.sep_comma.unwrap_or(false),
            negative: input.neg_style.unwrap_or_else(|| "MinusBlack".into()),
            currency: input.currency.unwrap_or_default(),
            currency_first: input.currency_prepend.unwrap_or(true),
        },
        "percent" => Format::Percent {
            decimals: u8::try_from(input.n_dec.unwrap_or(0))
                .map_err(|_| options_error("nDec is too large"))?,
            separate_thousands: input.sep_comma.unwrap_or(false),
        },
        other => return Err(options_error(format!("{other:?} is not a field format"))),
    })
}

#[napi(object)]
pub struct SvgInfo {
    pub handle: u32,
    /// Points.
    pub width: f64,
    pub height: f64,
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
    /// Mark each paragraph as structure content.
    pub structure: Option<bool>,
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
            structure: input.structure.unwrap_or(false),
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
            tagged: settings.tagged.unwrap_or(false),
            subset: settings
                .subset
                .as_deref()
                .map(Subset::parse)
                .transpose()
                .map_err(failure)?,
            max_file_bytes: settings
                .max_file_bytes
                .map_or(defaults.max_file_bytes, |limit| limit as usize),
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

    /// One item of a list: `done`, or `needPage` as for [`Self::text`].
    #[napi]
    pub fn list_item(
        &mut self,
        text: String,
        options: TextOptionsInput,
        item: ListItemInput,
    ) -> Result<&'static str> {
        let options = TextOptions::try_from(options)?;
        if item.level == 0 || item.level > 64 {
            return Err(options_error("a list nests 1 to 64 levels deep"));
        }
        let marker = match (item.label, item.radius, item.mid_line) {
            (Some(label), _, _) => ListMarker::Label(label),
            (None, Some(radius), Some(mid_line)) => ListMarker::Bullet {
                radius: finite(radius, "bulletRadius")?,
                mid_line: finite(mid_line, "midLine")?,
            },
            _ => return Err(options_error("a list item has a label or a bullet")),
        };
        let item = ListItem {
            level: item.level,
            marker,
            indent: finite(item.indent, "textIndent")?,
            item_indent: finite(item.item_indent, "bulletIndent")?,
        };
        self.with(|document| {
            document
                .list_item(&text, &options, item)
                .map(step)
                .map_err(failure)
        })
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
    ) -> Result<u32> {
        let target = match (url, page, name) {
            (Some(url), None, None) => LinkTarget::Uri(url),
            (None, Some(page), None) => LinkTarget::Page(page as usize),
            (None, None, Some(name)) => LinkTarget::Named(name),
            _ => return Err(options_error("a link goes to one URL, page or destination")),
        };
        let (x, y) = (finite(x, "x")?, finite(y, "y")?);
        let (width, height) = (finite(width, "width")?, finite(height, "height")?);
        self.with(|document| {
            document
                .link(x, y, width, height, target)
                .map(|id| id as u32)
                .map_err(failure)
        })
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

    /// Begin a marked sequence and answer its handle.
    #[napi]
    pub fn begin_mark(&mut self, input: MarkInput) -> Result<u32> {
        let mark = match input.kind.as_str() {
            "artifact" => Mark::Artifact(match input.artifact.as_deref() {
                None => None,
                Some("Pagination") => Some(ArtifactKind::Pagination),
                Some("Layout") => Some(ArtifactKind::Layout),
                Some("Page") => Some(ArtifactKind::Page),
                Some("Header") => Some(ArtifactKind::Header),
                Some("Footer") => Some(ArtifactKind::Footer),
                Some("Watermark") => Some(ArtifactKind::Watermark),
                Some("PageNumber") => Some(ArtifactKind::PageNumber),
                Some(other) => {
                    return Err(options_error(format!("{other:?} is not an artifact type")))
                }
            }),
            "span" => Mark::Span {
                lang: input.lang,
                alt: input.alt,
                expanded: input.expanded,
                actual: input.actual,
            },
            "structure" => Mark::Structure,
            other => {
                return Err(options_error(format!(
                    "{other:?} is not a kind of marked content"
                )))
            }
        };
        self.with(|document| {
            document
                .begin_mark(mark)
                .map(|id| id as u32)
                .map_err(failure)
        })
    }

    #[napi]
    pub fn continue_mark(&mut self, id: u32) -> Result<()> {
        self.with(|document| document.continue_mark(id as usize).map_err(failure))
    }

    #[napi]
    pub fn end_mark(&mut self) -> Result<()> {
        self.with(|document| document.end_mark().map_err(failure))
    }

    #[napi]
    pub fn get_open_mark(&self) -> Option<u32> {
        self.inner.open_mark().map(|id| id as u32)
    }

    #[napi]
    pub fn take_marked_contents(&mut self) -> Vec<u32> {
        self.inner
            .take_marked_contents()
            .into_iter()
            .map(|id| id as u32)
            .collect()
    }

    /// The structure tree, as elements and their children in order.
    #[napi]
    pub fn set_structure(
        &mut self,
        nodes: Vec<StructureNodeInput>,
        children: Vec<StructureChildInput>,
    ) -> Result<()> {
        let roots = structure(nodes, children)?;
        self.with(|document| document.set_structure(roots).map_err(failure))
    }

    /// The outline, entries in order, each after its parent.
    #[napi]
    pub fn set_outline(&mut self, entries: Vec<OutlineInput>) -> Result<()> {
        let mut items: Vec<OutlineItem> = Vec::with_capacity(entries.len());
        let mut parents: Vec<Option<usize>> = Vec::with_capacity(entries.len());
        for (index, entry) in entries.iter().enumerate() {
            let parent = entry.parent.map(|parent| parent as usize);
            if parent.is_some_and(|parent| parent >= index) {
                return Err(options_error("an outline entry comes after its parent"));
            }
            items.push(OutlineItem {
                title: entry.title.clone(),
                page: entry.page as usize,
                x: finite(entry.x, "outline x")?,
                y: finite(entry.y, "outline y")?,
                open: entry.open,
                children: Vec::new(),
            });
            parents.push(parent);
        }
        // Children last to first, so each lands in its parent before the
        // parent itself moves into its own.
        let mut roots = Vec::new();
        for index in (0..items.len()).rev() {
            let item = items[index].clone();
            let taken = items_take(&mut items, index, item);
            match parents[index] {
                Some(parent) => items[parent].children.insert(0, taken),
                None => roots.insert(0, taken),
            }
        }
        self.with(|document| document.set_outline(roots).map_err(failure))
    }

    #[napi]
    pub fn attach_file(&mut self, input: FileInput) -> Result<u32> {
        let attachment = Attachment {
            name: input.name,
            data: std::sync::Arc::new(input.data.to_vec()),
            mime: input.mime,
            description: input.description,
            relationship: input
                .relationship
                .as_deref()
                .map(Relationship::parse)
                .transpose()
                .map_err(failure)?
                .unwrap_or_default(),
            created: input.created.map(|unix_ms| Instant { unix_ms }),
            modified: input.modified.map(|unix_ms| Instant { unix_ms }),
            hidden: input.hidden.unwrap_or(false),
        };
        self.with(|document| {
            document
                .attach(attachment)
                .map(|id| id as u32)
                .map_err(failure)
        })
    }

    #[napi]
    pub fn annotate(&mut self, input: AnnotationInput) -> Result<()> {
        let point = |value: Option<f64>, name: &str| -> Result<f32> {
            finite(
                value.ok_or_else(|| options_error(format!("{name} is missing")))?,
                name,
            )
        };
        let (x, y) = (finite(input.x, "x")?, finite(input.y, "y")?);
        let kind = match input.kind.as_str() {
            "note" => AnnotationKind::Note {
                icon: input.icon.unwrap_or_else(|| "Comment".into()),
            },
            "highlight" => AnnotationKind::Highlight,
            "underline" => AnnotationKind::Underline,
            "strike" => AnnotationKind::StrikeOut,
            "line" => AnnotationKind::Line {
                from: (x, y),
                to: (point(input.x2, "x2")?, point(input.y2, "y2")?),
            },
            "square" => AnnotationKind::Square,
            "circle" => AnnotationKind::Circle,
            "text" => AnnotationKind::FreeText,
            "file" => AnnotationKind::File {
                attachment: input
                    .file
                    .ok_or_else(|| options_error("a file annotation needs its file"))?
                    as usize,
            },
            other => {
                return Err(options_error(format!(
                    "{other:?} is not an annotation this document writes"
                )))
            }
        };
        if let AnnotationKind::Note { icon } = &kind {
            let known = [
                "Comment",
                "Key",
                "Note",
                "Help",
                "NewParagraph",
                "Paragraph",
                "Insert",
            ];
            if !known.contains(&icon.as_str()) {
                return Err(options_error(format!("{icon:?} is not a note icon")));
            }
        }
        let colour = match input.colour.as_deref() {
            None => None,
            Some([red, green, blue]) => Some([
                byte(*red, "colour")?,
                byte(*green, "colour")?,
                byte(*blue, "colour")?,
            ]),
            Some(_) => return Err(options_error("an annotation's colour is [r, g, b]")),
        };
        let (width, height) = (
            finite(input.width, "width")?,
            finite(input.height, "height")?,
        );
        let contents = input.contents;
        self.with(|document| {
            document
                .annotate(kind, x, y, width, height, contents, colour)
                .map_err(failure)
        })
    }

    /// Open a named destination with another view: `Fit`, `FitB`, `FitH`,
    /// `FitBH` (top), `FitV`, `FitBV` (left), `FitR` (left, top, right,
    /// bottom), in user space.
    #[napi]
    pub fn set_destination_fit(
        &mut self,
        name: String,
        fit: String,
        values: Vec<Option<f64>>,
    ) -> Result<()> {
        let value = |index: usize| -> Result<Option<f32>> {
            values
                .get(index)
                .copied()
                .flatten()
                .map(|value| finite(value, "a destination's position"))
                .transpose()
        };
        let fit = match fit.as_str() {
            "Fit" => Fit::Fit,
            "FitB" => Fit::FitB,
            "FitH" => Fit::FitH(value(0)?),
            "FitBH" => Fit::FitBH(value(0)?),
            "FitV" => Fit::FitV(value(0)?),
            "FitBV" => Fit::FitBV(value(0)?),
            "FitR" => {
                let corners = [value(0)?, value(1)?, value(2)?, value(3)?];
                let [Some(left), Some(top), Some(right), Some(bottom)] = corners else {
                    return Err(options_error("FitR takes left, top, right and bottom"));
                };
                Fit::FitR([left, top, right, bottom])
            }
            other => {
                return Err(options_error(format!(
                    "{other:?} is not a destination view"
                )))
            }
        };
        self.with(|document| document.set_destination_fit(&name, fit).map_err(failure))
    }

    /// A form field, shown on the current page when `rect` is given.
    #[napi]
    pub fn form_field(&mut self, input: FieldInput, rect: Option<RectInput>) -> Result<u32> {
        let kind = match input.kind.as_deref() {
            None => None,
            Some("text") => Some(FieldType::Text),
            Some("pushButton") => Some(FieldType::PushButton),
            Some("radioButton") => Some(FieldType::RadioButton),
            Some("checkbox") => Some(FieldType::Checkbox),
            Some("combo") => Some(FieldType::Combo),
            Some("list") => Some(FieldType::List),
            Some(other) => {
                return Err(options_error(format!(
                    "Invalid form annotation type '{other}'"
                )))
            }
        };
        let align = match input.align.as_deref() {
            None | Some("left") => 0,
            Some("center") => 1,
            Some("right") => 2,
            Some(other) => {
                return Err(options_error(format!("{other:?} is not a field alignment")))
            }
        };
        let spec = FieldSpec {
            name: input.name,
            kind,
            parent: input.parent.map(|parent| parent as usize),
            flags: input.flags.unwrap_or(0),
            align,
            value: input.value,
            default_value: input.default_value,
            choices: input.choices.unwrap_or_default(),
            label: input.label,
            background: rgb(input.background, "backgroundColor")?,
            border: rgb(input.border, "borderColor")?,
            font_size: optional(input.font_size, "fontSize")?.unwrap_or(0.0),
            format: input.format.map(format).transpose()?,
            on_state: input.on_state,
            widget: None,
        };
        let rect = match rect {
            None => None,
            Some(rect) => Some((
                finite(rect.x, "x")?,
                finite(rect.y, "y")?,
                finite(rect.width, "width")?,
                finite(rect.height, "height")?,
            )),
        };
        self.with(|document| {
            document
                .form_field(spec, rect)
                .map(|id| id as u32)
                .map_err(failure)
        })
    }

    #[napi]
    pub fn set_encryption(&mut self, input: EncryptionInput) -> Result<()> {
        let permissions = input.permissions.unwrap_or(PermissionsInput {
            printing: None,
            modifying: None,
            copying: None,
            annotating: None,
            filling_forms: None,
            content_accessibility: None,
            document_assembly: None,
        });
        let printing = match permissions.printing.as_deref() {
            None => None,
            Some("lowResolution") => Some(false),
            Some("highResolution") => Some(true),
            Some(other) => {
                return Err(options_error(format!(
                    "printing {other:?} is lowResolution or highResolution"
                )))
            }
        };
        let encryption = Encryption {
            user_password: input.user_password.unwrap_or_default(),
            owner_password: input.owner_password.unwrap_or_default(),
            rights: Rights {
                printing,
                modifying: permissions.modifying.unwrap_or(false),
                copying: permissions.copying.unwrap_or(false),
                annotating: permissions.annotating.unwrap_or(false),
                filling_forms: permissions.filling_forms.unwrap_or(false),
                content_accessibility: permissions.content_accessibility.unwrap_or(false),
                document_assembly: permissions.document_assembly.unwrap_or(false),
            },
        };
        self.with(|document| document.set_encryption(encryption).map_err(failure))
    }

    /// Parse an SVG: its handle and its size in points.
    #[napi]
    pub fn load_svg(&mut self, bytes: Buffer) -> Result<SvgInfo> {
        let bytes = bytes.to_vec();
        self.with(|document| {
            document
                .load_svg(&bytes)
                .map(|(handle, width, height)| SvgInfo {
                    handle: handle as u32,
                    width: f64::from(width),
                    height: f64::from(height),
                })
                .map_err(failure)
        })
    }

    #[napi]
    pub fn draw_svg(&mut self, handle: u32, x: f64, y: f64, width: f64, height: f64) -> Result<()> {
        let (x, y) = (finite(x, "x")?, finite(y, "y")?);
        let (width, height) = (finite(width, "width")?, finite(height, "height")?);
        self.with(|document| {
            document
                .draw_svg(handle as usize, x, y, width, height)
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

/// The tree from its flat description: elements, then each element's
/// children in order. An element listed as no one's child is a root.
fn structure(
    nodes: Vec<StructureNodeInput>,
    children: Vec<StructureChildInput>,
) -> Result<Vec<Element>> {
    let count = nodes.len();
    let mut lists: Vec<Vec<&StructureChildInput>> = vec![Vec::new(); count];
    let mut parent_of: Vec<Option<usize>> = vec![None; count];
    for child in &children {
        let parent = child.parent as usize;
        let list = lists
            .get_mut(parent)
            .ok_or_else(|| options_error("a structure child names no element"))?;
        if let Some(node) = child.node {
            let node = node as usize;
            if node >= count || node == parent {
                return Err(options_error("a structure child names no element"));
            }
            if parent_of[node].replace(parent).is_some() {
                return Err(options_error(
                    "a structure element has more than one parent",
                ));
            }
        }
        list.push(child);
    }
    fn build(
        index: usize,
        nodes: &[StructureNodeInput],
        lists: &[Vec<&StructureChildInput>],
        depth: usize,
    ) -> Result<Element> {
        if depth > vellum_engine::compose::tagging::MAX_DEPTH {
            return Err(options_error("the structure tree is too deep, or loops"));
        }
        let node = &nodes[index];
        let mut element = Element {
            kind: node.kind.clone(),
            title: node.title.clone(),
            lang: node.lang.clone(),
            alt: node.alt.clone(),
            expanded: node.expanded.clone(),
            actual: node.actual.clone(),
            scope: node.scope.clone(),
            row_span: node.row_span,
            col_span: node.col_span,
            children: Vec::new(),
        };
        for child in &lists[index] {
            element
                .children
                .push(match (child.node, child.content, child.annotation) {
                    (Some(node), None, None) => {
                        Child::Element(build(node as usize, nodes, lists, depth + 1)?)
                    }
                    (None, Some(content), None) => Child::Content(content as usize),
                    (None, None, Some(annotation)) => Child::Annotation(annotation as usize),
                    _ => {
                        return Err(options_error(
                            "a structure child is one element, content or link",
                        ))
                    }
                });
        }
        Ok(element)
    }
    (0..count)
        .filter(|index| parent_of[*index].is_none())
        .map(|index| build(index, &nodes, &lists, 0))
        .collect()
}

/// The entry at `index` with its children gathered so far.
fn items_take(items: &mut [OutlineItem], index: usize, mut item: OutlineItem) -> OutlineItem {
    item.children = std::mem::take(&mut items[index].children);
    item
}
