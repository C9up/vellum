use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::collections::HashMap;
use std::panic::catch_unwind;

pub mod compose;

/// Run engine work with a panic net.
///
/// A malformed PDF is hostile input: a panic crossing the NAPI boundary takes
/// the whole Node process down, so it is turned into a rejected promise here.
fn wrap<T, F>(f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> std::result::Result<T, String> + std::panic::UnwindSafe,
{
    match catch_unwind(f) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(message)) => Err(Error::from_reason(message)),
        Err(_) => Err(Error::from_reason("Internal panic in vellum engine")),
    }
}

/// Run engine work on a worker thread with a panic net.
///
/// A panic on a libuv worker aborts the whole process, and every input here
/// arrives from an upload, so no task may let one escape.
fn guarded<T, F>(doing: &str, work: F) -> Result<T>
where
    F: FnOnce() -> std::result::Result<T, String>,
{
    match catch_unwind(std::panic::AssertUnwindSafe(work)) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(message)) => Err(Error::from_reason(message)),
        Err(_) => Err(Error::from_reason(format!("Internal panic while {doing}"))),
    }
}

#[napi(object)]
pub struct DocumentInfo {
    pub page_count: u32,
    pub version: String,
    pub encrypted: bool,
}

#[napi(object)]
pub struct PageSize {
    /// Width in points (72 per inch). A4 is 595.28.
    pub width: f64,
    /// Height in points. A4 is 841.89.
    pub height: f64,
}

/// Parsing a document is work proportional to its size, which the upload
/// chooses; it goes to the thread pool like rendering does, so a large file
/// never stalls the requests served alongside it.
pub struct InspectTask {
    bytes: Vec<u8>,
}

impl Task for InspectTask {
    type Output = vellum_engine::DocumentInfo;
    type JsValue = DocumentInfo;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("reading a PDF", || vellum_engine::inspect(&self.bytes))
    }

    fn resolve(&mut self, _env: Env, info: Self::Output) -> Result<Self::JsValue> {
        Ok(DocumentInfo {
            page_count: info.page_count,
            version: info.version,
            encrypted: info.encrypted,
        })
    }
}

#[napi(ts_return_type = "Promise<DocumentInfo>")]
pub fn inspect(bytes: Buffer) -> AsyncTask<InspectTask> {
    AsyncTask::new(InspectTask {
        bytes: bytes.to_vec(),
    })
}

#[napi]
pub fn create_blank(pages: Vec<PageSize>) -> Result<Buffer> {
    let sizes: Vec<(f32, f32)> = pages
        .iter()
        .map(|page| (page.width as f32, page.height as f32))
        .collect();
    wrap(move || vellum_engine::create_blank(&sizes)).map(Buffer::from)
}

/// A rectangle of a page, in points from the top-left corner.
#[napi(object)]
pub struct Band {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Rasterising options, as a plain JavaScript object.
#[napi(object)]
pub struct RenderOptions {
    /// Multiplier over the page's natural size, 1 being 72 DPI. Default 1.
    /// Ignored when `width` is given.
    pub scale: Option<f64>,
    /// Target width in pixels. Wins over `scale`.
    pub width: Option<u32>,
    /// `"png"` (default) or `"jpeg"`.
    pub format: Option<String>,
    /// JPEG quality, 1-100. Only valid alongside `format: "jpeg"`.
    pub quality: Option<u32>,
    /// `#rgb`, `#rrggbb`, `#rrggbbaa` or `"transparent"`. Default opaque white.
    pub background: Option<String>,
    /// Render only this rectangle, in points from the TOP-left corner — the
    /// same corner `stampText` measures from. A band that does not fit the
    /// page is an error, never a silent full page.
    pub band: Option<Band>,
    /// The most pixels one page may rasterise to. 50 million by default —
    /// room for A4 at 600 DPI. A page declares its own size, so without a
    /// ceiling a document alone could ask for gigabytes.
    pub max_pixels: Option<f64>,
    /// The most pixels `renderAll` may rasterise across every page. 500
    /// million by default; checked before the first page is drawn.
    pub max_total_pixels: Option<f64>,
    /// Bytes of decoded stream data one page may need. 256 MiB by default.
    pub max_decoded_bytes: Option<f64>,
    /// Content operators and glyphs one page may take to draw. Ten million
    /// by default.
    pub max_operations: Option<f64>,
}

/// What reading one page may cost, for the calls that do not rasterise.
#[napi(object)]
pub struct ReadLimits {
    /// Bytes of decoded stream data one page may need. 256 MiB by default.
    pub max_decoded_bytes: Option<f64>,
    /// Content operators and glyphs one page may take to draw. Ten million
    /// by default.
    pub max_operations: Option<f64>,
}

/// A ceiling as JavaScript hands it: a whole number from 1 to `u32::MAX`.
///
/// Taken as `f64` and checked here, because napi's `u32` conversion is
/// ECMAScript's ToUint32: `-1` would arrive as 4 294 967 295 and `1.5` as 1,
/// turning a typo in a configuration into no ceiling at all.
fn positive(name: &str, value: f64) -> std::result::Result<u64, String> {
    if value.fract() != 0.0 || !(1.0..=f64::from(u32::MAX)).contains(&value) {
        return Err(format!(
            "{name} must be a whole number from 1 to {}, got {value}",
            u32::MAX
        ));
    }
    Ok(value as u64)
}

fn to_engine_limits(
    max_decoded_bytes: Option<f64>,
    max_operations: Option<f64>,
) -> std::result::Result<vellum_engine::ReadLimits, String> {
    let mut limits = vellum_engine::ReadLimits::default();
    if let Some(value) = max_decoded_bytes {
        limits.max_decoded_bytes = positive("maxDecodedBytes", value)?;
    }
    if let Some(value) = max_operations {
        limits.max_operations = positive("maxOperations", value)?;
    }
    Ok(limits)
}

fn read_limits(limits: Option<ReadLimits>) -> Result<vellum_engine::ReadLimits> {
    let (bytes, operations) = limits.map_or((None, None), |limits| {
        (limits.max_decoded_bytes, limits.max_operations)
    });
    to_engine_limits(bytes, operations).map_err(Error::from_reason)
}

fn to_engine_options(
    options: Option<RenderOptions>,
) -> std::result::Result<vellum_engine::RenderOptions, String> {
    let mut engine = vellum_engine::RenderOptions::default();
    let Some(options) = options else {
        return Ok(engine);
    };

    if let Some(scale) = options.scale {
        engine.scale = scale as f32;
    }
    engine.width = options.width;
    engine.band = options.band.map(|band| vellum_engine::Band {
        x: band.x as f32,
        y: band.y as f32,
        width: band.width as f32,
        height: band.height as f32,
    });
    if let Some(max) = options.max_pixels {
        engine.max_pixels = u32::try_from(positive("maxPixels", max)?)
            .map_err(|_| "maxPixels is out of range".to_string())?;
    }
    if let Some(max) = options.max_total_pixels {
        engine.max_total_pixels = positive("maxTotalPixels", max)?;
    }
    engine.limits = to_engine_limits(options.max_decoded_bytes, options.max_operations)?;

    let quality = match options.quality {
        Some(value) => Some(
            u8::try_from(value).map_err(|_| format!("JPEG quality must be 1-100, got {value}"))?,
        ),
        None => None,
    };

    match (options.format.as_deref(), quality) {
        (Some(format), _) => {
            engine.format = vellum_engine::ImageFormat::parse(format, quality)?;
        }
        // Refused rather than ignored: a caller passing a quality clearly wants
        // a lossy image, and silently returning a multi-megabyte PNG is a
        // surprise nobody notices until the previews are slow.
        (None, Some(_)) => {
            return Err(
                "`quality` only applies to JPEG - set `format: \"jpeg\"` as well".to_string(),
            );
        }
        (None, None) => {}
    }

    if let Some(background) = options.background.as_deref() {
        engine.background = vellum_engine::parse_color(background)?;
    }

    Ok(engine)
}

/// Rasterising runs on the libuv thread pool.
///
/// Rendering an A4 page is tens of milliseconds of pure computation. Done on
/// the main thread it would stall every other request in the process for the
/// duration, which for a document-heavy application is the whole workload.
pub struct RenderPageTask {
    bytes: Vec<u8>,
    page_index: u32,
    options: vellum_engine::RenderOptions,
}

impl Task for RenderPageTask {
    type Output = Vec<u8>;
    type JsValue = Buffer;

    fn compute(&mut self) -> Result<Self::Output> {
        // A panic on a worker thread aborts the process, so a malformed
        // document must not be able to reach one.
        guarded("rendering a PDF page", || {
            vellum_engine::render_page(&self.bytes, self.page_index, &self.options)
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(Buffer::from(output))
    }
}

pub struct RenderAllTask {
    bytes: Vec<u8>,
    options: vellum_engine::RenderOptions,
}

impl Task for RenderAllTask {
    type Output = Vec<Vec<u8>>;
    type JsValue = Vec<Buffer>;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("rendering a PDF", || {
            vellum_engine::render_all(&self.bytes, &self.options)
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output.into_iter().map(Buffer::from).collect())
    }
}

#[napi(ts_return_type = "Promise<Buffer>")]
pub fn render_page(
    bytes: Buffer,
    page_index: u32,
    options: Option<RenderOptions>,
) -> Result<AsyncTask<RenderPageTask>> {
    let options = to_engine_options(options).map_err(Error::from_reason)?;
    Ok(AsyncTask::new(RenderPageTask {
        bytes: bytes.to_vec(),
        page_index,
        options,
    }))
}

#[napi(ts_return_type = "Promise<Buffer[]>")]
pub fn render_all(
    bytes: Buffer,
    options: Option<RenderOptions>,
) -> Result<AsyncTask<RenderAllTask>> {
    let options = to_engine_options(options).map_err(Error::from_reason)?;
    Ok(AsyncTask::new(RenderAllTask {
        bytes: bytes.to_vec(),
        options,
    }))
}

#[napi(object)]
pub struct PageDimensions {
    /// Width in points, before scaling.
    pub width: f64,
    /// Height in points, before scaling.
    pub height: f64,
}

pub struct PageDimensionsTask {
    bytes: Vec<u8>,
    limits: vellum_engine::ReadLimits,
}

impl Task for PageDimensionsTask {
    type Output = Vec<vellum_engine::PageDimensions>;
    type JsValue = Vec<PageDimensions>;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("reading a PDF", || {
            vellum_engine::page_dimensions_with(&self.bytes, &self.limits)
        })
    }

    fn resolve(&mut self, _env: Env, pages: Self::Output) -> Result<Self::JsValue> {
        Ok(pages
            .into_iter()
            .map(|page| PageDimensions {
                width: f64::from(page.width),
                height: f64::from(page.height),
            })
            .collect())
    }
}

#[napi(ts_return_type = "Promise<PageDimensions[]>")]
pub fn page_dimensions(
    bytes: Buffer,
    limits: Option<ReadLimits>,
) -> Result<AsyncTask<PageDimensionsTask>> {
    Ok(AsyncTask::new(PageDimensionsTask {
        bytes: bytes.to_vec(),
        limits: read_limits(limits)?,
    }))
}

/// What the `/Info` dictionary says about a document.
#[napi(object)]
pub struct DocumentMetadata {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    /// The application that authored the content.
    pub creator: Option<String>,
    /// The application that wrote the PDF.
    pub producer: Option<String>,
    /// ISO 8601 when the producer wrote a conforming date, otherwise the raw
    /// string it did write.
    pub created_at: Option<String>,
    pub modified_at: Option<String>,
}

pub struct MetadataTask {
    bytes: Vec<u8>,
}

impl Task for MetadataTask {
    type Output = vellum_engine::DocumentMetadata;
    type JsValue = DocumentMetadata;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("reading a PDF", || vellum_engine::metadata(&self.bytes))
    }

    fn resolve(&mut self, _env: Env, info: Self::Output) -> Result<Self::JsValue> {
        Ok(DocumentMetadata {
            title: info.title,
            author: info.author,
            subject: info.subject,
            keywords: info.keywords,
            creator: info.creator,
            producer: info.producer,
            created_at: info.created_at,
            modified_at: info.modified_at,
        })
    }
}

#[napi(ts_return_type = "Promise<DocumentMetadata>")]
pub fn metadata(bytes: Buffer) -> AsyncTask<MetadataTask> {
    AsyncTask::new(MetadataTask {
        bytes: bytes.to_vec(),
    })
}

/// Extracting text walks the whole content stream, so it goes to the thread
/// pool for the same reason rasterising does.
pub struct ExtractTextTask {
    bytes: Vec<u8>,
    page_index: u32,
    limits: vellum_engine::ReadLimits,
}

impl Task for ExtractTextTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("extracting text", || {
            vellum_engine::extract_text_with(&self.bytes, self.page_index, &self.limits)
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

pub struct ExtractTextAllTask {
    bytes: Vec<u8>,
    limits: vellum_engine::ReadLimits,
}

impl Task for ExtractTextAllTask {
    type Output = Vec<String>;
    type JsValue = Vec<String>;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("extracting text", || {
            vellum_engine::extract_text_all_with(&self.bytes, &self.limits)
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

#[napi(ts_return_type = "Promise<string>")]
pub fn extract_text(
    bytes: Buffer,
    page_index: u32,
    limits: Option<ReadLimits>,
) -> Result<AsyncTask<ExtractTextTask>> {
    Ok(AsyncTask::new(ExtractTextTask {
        bytes: bytes.to_vec(),
        page_index,
        limits: read_limits(limits)?,
    }))
}

#[napi(ts_return_type = "Promise<string[]>")]
pub fn extract_text_all(
    bytes: Buffer,
    limits: Option<ReadLimits>,
) -> Result<AsyncTask<ExtractTextAllTask>> {
    Ok(AsyncTask::new(ExtractTextAllTask {
        bytes: bytes.to_vec(),
        limits: read_limits(limits)?,
    }))
}

/// Rewriting a document parses and re-serialises the whole object tree, so it
/// belongs on the thread pool alongside rendering.
/// How an edit treats a document that is already signed.
#[napi(object)]
pub struct EditOptions {
    /// Rewrite a signed document anyway. Every edit writes the file anew,
    /// which invalidates its signatures; without this, a signed document is
    /// refused with `SIGNED_DOCUMENT`.
    pub discard_signatures: Option<bool>,
}

fn discards(options: &Option<EditOptions>) -> bool {
    options
        .as_ref()
        .and_then(|options| options.discard_signatures)
        .unwrap_or(false)
}

pub struct MergeTask {
    documents: Vec<Vec<u8>>,
    discard_signatures: bool,
}

impl Task for MergeTask {
    type Output = Vec<u8>;
    type JsValue = Buffer;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("merging documents", || {
            for document in &self.documents {
                vellum_engine::refuse_if_signed(document, self.discard_signatures)?;
            }
            vellum_engine::merge(&self.documents)
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(Buffer::from(output))
    }
}

pub struct SelectPagesTask {
    bytes: Vec<u8>,
    pages: Vec<u32>,
    discard_signatures: bool,
}

impl Task for SelectPagesTask {
    type Output = Vec<u8>;
    type JsValue = Buffer;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("selecting pages", || {
            vellum_engine::refuse_if_signed(&self.bytes, self.discard_signatures)?;
            vellum_engine::select_pages(&self.bytes, &self.pages)
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(Buffer::from(output))
    }
}

pub struct SplitTask {
    bytes: Vec<u8>,
    discard_signatures: bool,
}

impl Task for SplitTask {
    type Output = Vec<Vec<u8>>;
    type JsValue = Vec<Buffer>;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("splitting a document", || {
            vellum_engine::refuse_if_signed(&self.bytes, self.discard_signatures)?;
            vellum_engine::split(&self.bytes)
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output.into_iter().map(Buffer::from).collect())
    }
}

pub struct RotateTask {
    bytes: Vec<u8>,
    degrees: i64,
    pages: Option<Vec<u32>>,
    discard_signatures: bool,
}

impl Task for RotateTask {
    type Output = Vec<u8>;
    type JsValue = Buffer;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("rotating pages", || {
            vellum_engine::refuse_if_signed(&self.bytes, self.discard_signatures)?;
            vellum_engine::rotate(&self.bytes, self.degrees, self.pages.as_deref())
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(Buffer::from(output))
    }
}

#[napi(ts_return_type = "Promise<Buffer>")]
pub fn merge(documents: Vec<Buffer>, options: Option<EditOptions>) -> AsyncTask<MergeTask> {
    AsyncTask::new(MergeTask {
        documents: documents.iter().map(|document| document.to_vec()).collect(),
        discard_signatures: discards(&options),
    })
}

#[napi(ts_return_type = "Promise<Buffer>")]
pub fn select_pages(
    bytes: Buffer,
    pages: Vec<u32>,
    options: Option<EditOptions>,
) -> AsyncTask<SelectPagesTask> {
    AsyncTask::new(SelectPagesTask {
        bytes: bytes.to_vec(),
        pages,
        discard_signatures: discards(&options),
    })
}

#[napi(ts_return_type = "Promise<Buffer[]>")]
pub fn split(bytes: Buffer, options: Option<EditOptions>) -> AsyncTask<SplitTask> {
    AsyncTask::new(SplitTask {
        bytes: bytes.to_vec(),
        discard_signatures: discards(&options),
    })
}

#[napi(ts_return_type = "Promise<Buffer>")]
pub fn rotate(
    bytes: Buffer,
    degrees: i64,
    pages: Option<Vec<u32>>,
    options: Option<EditOptions>,
) -> AsyncTask<RotateTask> {
    AsyncTask::new(RotateTask {
        bytes: bytes.to_vec(),
        degrees,
        pages,
        discard_signatures: discards(&options),
    })
}

/// Where and how an image is laid onto a page.
#[napi(object)]
pub struct StampOptions {
    /// Which page, counting from zero. Absent stamps every page.
    pub page: Option<u32>,
    /// Points from the left edge. Default 0.
    pub x: Option<f64>,
    /// Points from the TOP edge. Default 0.
    pub y: Option<f64>,
    /// Drawn width in points. With `height` absent, the ratio is kept.
    pub width: Option<f64>,
    /// Drawn height in points. With `width` absent, the ratio is kept.
    pub height: Option<f64>,
    /// 0 is invisible, 1 is opaque. Default 1.
    pub opacity: Option<f64>,
    /// Stamp a signed document anyway, invalidating its signatures.
    pub discard_signatures: Option<bool>,
}

pub struct StampTask {
    pdf: Vec<u8>,
    image: Vec<u8>,
    options: vellum_engine::StampOptions,
    discard_signatures: bool,
}

impl Task for StampTask {
    type Output = Vec<u8>;
    type JsValue = Buffer;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("stamping a document", || {
            vellum_engine::refuse_if_signed(&self.pdf, self.discard_signatures)?;
            vellum_engine::stamp_image(&self.pdf, &self.image, &self.options)
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(Buffer::from(output))
    }
}

#[napi(ts_return_type = "Promise<Buffer>")]
pub fn stamp(pdf: Buffer, image: Buffer, options: Option<StampOptions>) -> AsyncTask<StampTask> {
    let discard_signatures = options
        .as_ref()
        .and_then(|options| options.discard_signatures)
        .unwrap_or(false);
    let defaults = vellum_engine::StampOptions::default();
    let options = options.map_or(defaults, |given| vellum_engine::StampOptions {
        page: given.page,
        x: given.x.map_or(defaults.x, |value| value as f32),
        y: given.y.map_or(defaults.y, |value| value as f32),
        width: given.width.map(|value| value as f32),
        height: given.height.map(|value| value as f32),
        opacity: given.opacity.map_or(defaults.opacity, |value| value as f32),
    });

    AsyncTask::new(StampTask {
        pdf: pdf.to_vec(),
        image: image.to_vec(),
        options,
        discard_signatures,
    })
}

/// Where and how a line of text is written onto a page.
#[napi(object)]
pub struct TextStampOptions {
    /// Which page, counting from zero. Absent writes on every page.
    pub page: Option<u32>,
    /// Points from the left edge. Default 0.
    pub x: Option<f64>,
    /// Points from the TOP edge, to the text's baseline. Default 0.
    pub y: Option<f64>,
    /// Type size in points. Default 12.
    pub size: Option<f64>,
    /// One of the 14 standard fonts, e.g. `"Helvetica"`, `"Times-Roman"`.
    /// Ignored when `fontData` is given.
    pub font: Option<String>,
    /// A TrueType or OpenType file to embed, subsetted to the text. Lifts the
    /// WinAnsi limit of the standard fonts, at the cost of carrying the glyphs
    /// in the document.
    pub font_data: Option<Buffer>,
    /// `#rgb` or `#rrggbb`. Default black.
    pub color: Option<String>,
    /// 0 is invisible, 1 is opaque. Default 1.
    pub opacity: Option<f64>,
    /// Write onto a signed document anyway, invalidating its signatures.
    pub discard_signatures: Option<bool>,
}

pub struct StampTextTask {
    pdf: Vec<u8>,
    text: String,
    options: vellum_engine::TextStampOptions,
    discard_signatures: bool,
}

impl Task for StampTextTask {
    type Output = Vec<u8>;
    type JsValue = Buffer;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("writing text onto a document", || {
            vellum_engine::refuse_if_signed(&self.pdf, self.discard_signatures)?;
            vellum_engine::stamp_text(&self.pdf, &self.text, &self.options)
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(Buffer::from(output))
    }
}

#[napi(ts_return_type = "Promise<Buffer>")]
pub fn stamp_text(
    pdf: Buffer,
    text: String,
    options: Option<TextStampOptions>,
) -> Result<AsyncTask<StampTextTask>> {
    let discard_signatures = options
        .as_ref()
        .and_then(|options| options.discard_signatures)
        .unwrap_or(false);
    let defaults = vellum_engine::TextStampOptions::default();
    let options = match options {
        None => defaults,
        Some(given) => {
            let font = match (&given.font_data, given.font.as_deref()) {
                // Supplied bytes win: a caller who passed a font meant it.
                (Some(data), _) => vellum_engine::FontChoice::Supplied(data.to_vec()),
                (None, Some(name)) => vellum_engine::FontChoice::Standard(
                    vellum_engine::StandardFont::parse(name).map_err(Error::from_reason)?,
                ),
                (None, None) => defaults.font.clone(),
            };
            // The colour parser is the engine's, so `#rgb` means the same
            // thing here as it does for a render background.
            let color = match given.color.as_deref() {
                Some(value) => {
                    let [red, green, blue, _] =
                        vellum_engine::parse_color(value).map_err(Error::from_reason)?;
                    [red, green, blue]
                }
                None => defaults.color,
            };
            vellum_engine::TextStampOptions {
                page: given.page,
                x: given.x.map_or(defaults.x, |value| value as f32),
                y: given.y.map_or(defaults.y, |value| value as f32),
                size: given.size.map_or(defaults.size, |value| value as f32),
                font,
                color,
                opacity: given.opacity.map_or(defaults.opacity, |value| value as f32),
            }
        }
    };

    Ok(AsyncTask::new(StampTextTask {
        pdf: pdf.to_vec(),
        text,
        options,
        discard_signatures,
    }))
}

/// One interactive field of a document's form.
#[napi(object)]
pub struct FormField {
    /// The fully qualified name — every ancestor's partial name joined with
    /// dots. This is the name used to fill the field in.
    pub name: String,
    /// `"text"`, `"checkbox"`, `"radio"`, `"pushButton"`, `"dropdown"`,
    /// `"listBox"` or `"signature"`.
    pub kind: String,
    pub value: Option<String>,
    /// What a choice field offers, or the states a checkbox and radio accept.
    /// A checkbox's "on" state is chosen by the document, so ticking it means
    /// writing one of these.
    pub options: Vec<String>,
    pub read_only: bool,
    pub required: bool,
    pub multiline: bool,
    pub password: bool,
    pub max_length: Option<u32>,
}

pub struct FormFieldsTask {
    bytes: Vec<u8>,
}

impl Task for FormFieldsTask {
    type Output = Vec<vellum_engine::FormField>;
    type JsValue = Vec<FormField>;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("reading a form", || vellum_engine::form_fields(&self.bytes))
    }

    fn resolve(&mut self, _env: Env, fields: Self::Output) -> Result<Self::JsValue> {
        Ok(fields
            .into_iter()
            .map(|field| FormField {
                name: field.name,
                kind: field.kind.as_str().to_string(),
                value: field.value,
                options: field.options,
                read_only: field.read_only,
                required: field.required,
                multiline: field.multiline,
                password: field.password,
                max_length: field.max_length,
            })
            .collect())
    }
}

#[napi(ts_return_type = "Promise<FormField[]>")]
pub fn form_fields(bytes: Buffer) -> AsyncTask<FormFieldsTask> {
    AsyncTask::new(FormFieldsTask {
        bytes: bytes.to_vec(),
    })
}

pub struct FillFormTask {
    pdf: Vec<u8>,
    values: Vec<vellum_engine::FieldValue>,
    discard_signatures: bool,
}

impl Task for FillFormTask {
    type Output = Vec<u8>;
    type JsValue = Buffer;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("filling a form", || {
            vellum_engine::refuse_if_signed(&self.pdf, self.discard_signatures)?;
            vellum_engine::fill_form(&self.pdf, &self.values)
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(Buffer::from(output))
    }
}

/// Fill the named fields. Keys are the fully qualified names `formFields`
/// reports.
#[napi(ts_return_type = "Promise<Buffer>")]
pub fn fill_form(
    pdf: Buffer,
    values: HashMap<String, String>,
    options: Option<EditOptions>,
) -> AsyncTask<FillFormTask> {
    AsyncTask::new(FillFormTask {
        pdf: pdf.to_vec(),
        values: values
            .into_iter()
            .map(|(name, value)| vellum_engine::FieldValue { name, value })
            .collect(),
        discard_signatures: discards(&options),
    })
}

pub struct FlattenFormTask {
    pdf: Vec<u8>,
    discard_signatures: bool,
}

impl Task for FlattenFormTask {
    type Output = Vec<u8>;
    type JsValue = Buffer;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("flattening a form", || {
            // Flattening drops the signature field itself, not only its
            // validity: refused unless the caller means it.
            vellum_engine::refuse_if_signed(&self.pdf, self.discard_signatures)?;
            vellum_engine::flatten_form(&self.pdf)
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(Buffer::from(output))
    }
}

/// Paint every field into the page and drop the interactive layer.
#[napi(ts_return_type = "Promise<Buffer>")]
pub fn flatten_form(pdf: Buffer, options: Option<EditOptions>) -> AsyncTask<FlattenFormTask> {
    AsyncTask::new(FlattenFormTask {
        pdf: pdf.to_vec(),
        discard_signatures: discards(&options),
    })
}

/// What the signature says about itself.
#[napi(object)]
pub struct SignatureOptions {
    /// Why the document was signed.
    pub reason: Option<String>,
    /// Where it was signed.
    pub location: Option<String>,
    /// How to reach the signatory.
    pub contact: Option<String>,
    /// Who signed, as it should be displayed.
    pub name: Option<String>,
    /// When, as an ISO 8601 instant.
    pub signed_at: Option<String>,
    /// Bytes reserved for the signature value. Default 16384.
    pub capacity: Option<u32>,
}

/// A document with room for a signature, and the digest to sign.
#[napi(object)]
pub struct PreparedSignature {
    pub document: Buffer,
    /// SHA-256 of everything the signature covers.
    pub digest: Buffer,
}

pub struct PrepareSignatureTask {
    pdf: Vec<u8>,
    options: vellum_engine::SignatureOptions,
}

impl Task for PrepareSignatureTask {
    type Output = vellum_engine::Prepared;
    type JsValue = PreparedSignature;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("preparing a document for signature", || {
            vellum_engine::prepare(&self.pdf, &self.options)
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(PreparedSignature {
            document: Buffer::from(output.document),
            digest: Buffer::from(output.digest.to_vec()),
        })
    }
}

/// Write a document with room for a signature, and say what has to be signed.
#[napi(ts_return_type = "Promise<PreparedSignature>")]
pub fn prepare_signature(
    pdf: Buffer,
    options: Option<SignatureOptions>,
) -> AsyncTask<PrepareSignatureTask> {
    let given = options.unwrap_or(SignatureOptions {
        reason: None,
        location: None,
        contact: None,
        name: None,
        signed_at: None,
        capacity: None,
    });
    AsyncTask::new(PrepareSignatureTask {
        pdf: pdf.to_vec(),
        options: vellum_engine::SignatureOptions {
            reason: given.reason,
            location: given.location,
            contact: given.contact,
            name: given.name,
            signed_at: given.signed_at,
            capacity: given.capacity.map_or(0, |value| value as usize),
        },
    })
}

pub struct EmbedSignatureTask {
    prepared: Vec<u8>,
    value: Vec<u8>,
}

impl Task for EmbedSignatureTask {
    type Output = Vec<u8>;
    type JsValue = Buffer;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("putting a signature into a document", || {
            vellum_engine::embed_signature(&self.prepared, &self.value)
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(Buffer::from(output))
    }
}

/// Put the signature value into the space that was reserved for it.
#[napi(ts_return_type = "Promise<Buffer>")]
pub fn embed_signature(prepared: Buffer, value: Buffer) -> AsyncTask<EmbedSignatureTask> {
    AsyncTask::new(EmbedSignatureTask {
        prepared: prepared.to_vec(),
        value: value.to_vec(),
    })
}

pub struct SignCmsTask {
    digest: Vec<u8>,
    key: Vec<u8>,
    certificates: Vec<Vec<u8>>,
    signed_at: String,
}

impl Task for SignCmsTask {
    type Output = Vec<u8>;
    type JsValue = Buffer;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("signing", || {
            vellum_engine::sign_cms(&self.digest, &self.key, &self.certificates, &self.signed_at)
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(Buffer::from(output))
    }
}

/// Turn a digest into the CMS a PDF signature carries, with a key we hold.
///
/// The key is PKCS#8 DER and the certificates are DER, the signer's first.
#[napi(ts_return_type = "Promise<Buffer>")]
pub fn sign_cms(
    digest: Buffer,
    key: Buffer,
    certificates: Vec<Buffer>,
    signed_at: String,
) -> AsyncTask<SignCmsTask> {
    AsyncTask::new(SignCmsTask {
        digest: digest.to_vec(),
        key: key.to_vec(),
        certificates: certificates
            .into_iter()
            .map(|certificate| certificate.to_vec())
            .collect(),
        signed_at,
    })
}

/// A query for a timestamp authority, and the nonce it has to echo back.
#[napi(object)]
pub struct TimestampQuery {
    /// The DER to post to the authority.
    pub query: Buffer,
    /// Opaque: hand it back to `attachTimestamp` unchanged.
    pub nonce: Buffer,
}

/// Build the query to post to a timestamp authority.
#[napi]
pub fn timestamp_query(cms: Buffer) -> Result<TimestampQuery> {
    let (query, nonce) = guarded("building a timestamp query", || {
        vellum_engine::timestamp_query(&cms)
    })?;
    Ok(TimestampQuery {
        query: Buffer::from(query),
        nonce: Buffer::from(nonce.to_be_bytes().to_vec()),
    })
}

/// Attach the authority's answer to the signature.
#[napi]
pub fn attach_timestamp(cms: Buffer, response: Buffer, nonce: Buffer) -> Result<Buffer> {
    let bytes: [u8; 8] = nonce
        .as_ref()
        .try_into()
        .map_err(|_| Error::from_reason("the nonce is not the one that was handed out"))?;
    let signed = guarded("attaching a timestamp", || {
        vellum_engine::attach_timestamp(&cms, &response, u64::from_be_bytes(bytes))
    })?;
    Ok(Buffer::from(signed))
}

/// What one signature on a document turns out to be.
#[napi(object)]
pub struct SignatureReport {
    /// The field the signature sits in.
    pub field: String,
    /// The signed range runs to the last byte, so nothing was appended after.
    pub covers_whole_document: bool,
    /// The document's bytes hash to what the signature committed to.
    pub digest_matches: bool,
    /// The signature verifies against the certificate its signer names.
    pub signature_verifies: bool,
    /// Who the certificate says signed.
    pub signer: Option<String>,
    /// When the signature says it was made, as the signer stated it. Shown,
    /// never used to judge anything.
    pub signed_at: Option<String>,
    /// A timestamp whose authority's signature checks out and chains to an
    /// anchor vouches for when, so the signature outlives the certificate.
    pub timestamped: bool,
    /// A path was found from the signer's certificate to a trusted anchor,
    /// every link valid at `momentAt`.
    pub trusted: bool,
    /// That path, the signer first and the anchor last.
    pub chain: Vec<String>,
    /// What the signature offers about when it was made: `"timestamp"` (a
    /// verified one), `"claimed"` (only the signer's own word) or `"unknown"`.
    /// Only a verified timestamp moves the instant judged at.
    pub moment: String,
    /// The instant the path was judged at, in seconds since the epoch: the
    /// timestamp's, or the time of the check.
    pub moment_at: Option<i64>,
    /// The certificate that signed, DER.
    pub signer_certificate: Option<Buffer>,
    /// The certificate that issued it, DER — who answers about revocation.
    pub issuer_certificate: Option<Buffer>,
    /// Everything that could not be checked, or checked out wrong.
    pub problems: Vec<String>,
}

/// What a caller is willing to believe.
#[napi(object)]
pub struct TrustOptions {
    /// Certificates to trust as roots, DER or PEM. Without them nothing can be
    /// trusted, which is what the report will say.
    pub anchors: Option<Vec<Buffer>>,
}

pub struct VerifySignaturesTask {
    pdf: Vec<u8>,
    trust: vellum_engine::TrustOptions,
}

impl Task for VerifySignaturesTask {
    type Output = Vec<vellum_engine::SignatureReport>;
    type JsValue = Vec<SignatureReport>;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("checking signatures", || {
            vellum_engine::verify_signatures(&self.pdf, &self.trust)
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output
            .into_iter()
            .map(|report| SignatureReport {
                field: report.field,
                covers_whole_document: report.covers_whole_document,
                digest_matches: report.digest_matches,
                signature_verifies: report.signature_verifies,
                signer: report.signer,
                signed_at: report.signed_at,
                timestamped: report.timestamped,
                trusted: report.trusted,
                chain: report.chain,
                moment: report.moment.to_string(),
                moment_at: report.moment_at.and_then(|at| i64::try_from(at).ok()),
                signer_certificate: report.signer_certificate.map(Buffer::from),
                issuer_certificate: report.issuer_certificate.map(Buffer::from),
                problems: report.problems,
            })
            .collect())
    }
}

/// Report on every signature the document carries.
///
/// Integrity and authorship from the document; trust against the anchors
/// given; revocation not asked, which needs a responder.
#[napi(ts_return_type = "Promise<SignatureReport[]>")]
pub fn verify_signatures(
    pdf: Buffer,
    trust: Option<TrustOptions>,
) -> AsyncTask<VerifySignaturesTask> {
    AsyncTask::new(VerifySignaturesTask {
        pdf: pdf.to_vec(),
        trust: vellum_engine::TrustOptions {
            anchors: trust
                .and_then(|given| given.anchors)
                .map(|anchors| anchors.iter().map(|anchor| anchor.to_vec()).collect())
                .unwrap_or_default(),
        },
    })
}

/// The responder a certificate names, if it names one.
#[napi]
pub fn responder_url(certificate: Buffer) -> Option<String> {
    vellum_engine::responder_url(&certificate)
}

/// Build the question to post to a revocation responder.
#[napi]
pub fn revocation_query(certificate: Buffer, issuer: Buffer) -> Result<Buffer> {
    let query = guarded("building a revocation query", || {
        vellum_engine::revocation_query(&certificate, &issuer)
    })?;
    Ok(Buffer::from(query))
}

/// What a responder's answer says.
#[napi(object)]
pub struct RevocationAnswer {
    /// `"good"`, `"revoked"` or `"unknown"`.
    pub status: String,
    /// When it was withdrawn, or why nobody could be believed.
    pub detail: Option<String>,
}

/// Read a responder's answer about a certificate.
///
/// `at` is the instant the signature is judged at, a report's `momentAt`: a
/// certificate withdrawn after that does not taint what it signed before.
/// Left out, it is now.
#[napi]
pub fn read_revocation(
    response: Buffer,
    certificate: Buffer,
    issuer: Buffer,
    at: Option<i64>,
) -> RevocationAnswer {
    let answer = vellum_engine::read_revocation(
        &response,
        &certificate,
        &issuer,
        at.and_then(|at| u64::try_from(at).ok()),
    );
    RevocationAnswer {
        status: answer.as_str().to_string(),
        detail: match &answer {
            vellum_engine::Revocation::Good => None,
            vellum_engine::Revocation::Revoked { at } => Some(at.clone()),
            vellum_engine::Revocation::Unknown { reason } => Some(reason.clone()),
        },
    }
}
