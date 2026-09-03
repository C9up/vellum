use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::panic::catch_unwind;

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

#[napi]
pub fn inspect(bytes: Buffer) -> Result<DocumentInfo> {
    // Own the bytes before the unwind-safe closure — a napi Buffer is not
    // UnwindSafe, and it borrows memory the JS heap still owns.
    let owned = bytes.to_vec();
    wrap(move || {
        vellum_engine::inspect(&owned).map(|info| DocumentInfo {
            page_count: info.page_count,
            version: info.version,
            encrypted: info.encrypted,
        })
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

#[napi]
pub fn page_dimensions(bytes: Buffer) -> Result<Vec<PageDimensions>> {
    let owned = bytes.to_vec();
    wrap(move || {
        vellum_engine::page_dimensions(&owned).map(|pages| {
            pages
                .into_iter()
                .map(|page| PageDimensions {
                    width: f64::from(page.width),
                    height: f64::from(page.height),
                })
                .collect()
        })
    })
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

#[napi]
pub fn metadata(bytes: Buffer) -> Result<DocumentMetadata> {
    let owned = bytes.to_vec();
    wrap(move || {
        vellum_engine::metadata(&owned).map(|info| DocumentMetadata {
            title: info.title,
            author: info.author,
            subject: info.subject,
            keywords: info.keywords,
            creator: info.creator,
            producer: info.producer,
            created_at: info.created_at,
            modified_at: info.modified_at,
        })
    })
}

/// Extracting text walks the whole content stream, so it goes to the thread
/// pool for the same reason rasterising does.
pub struct ExtractTextTask {
    bytes: Vec<u8>,
    page_index: u32,
}

impl Task for ExtractTextTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("extracting text", || {
            vellum_engine::extract_text(&self.bytes, self.page_index)
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

pub struct ExtractTextAllTask {
    bytes: Vec<u8>,
}

impl Task for ExtractTextAllTask {
    type Output = Vec<String>;
    type JsValue = Vec<String>;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("extracting text", || {
            vellum_engine::extract_text_all(&self.bytes)
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

#[napi(ts_return_type = "Promise<string>")]
pub fn extract_text(bytes: Buffer, page_index: u32) -> AsyncTask<ExtractTextTask> {
    AsyncTask::new(ExtractTextTask {
        bytes: bytes.to_vec(),
        page_index,
    })
}

#[napi(ts_return_type = "Promise<string[]>")]
pub fn extract_text_all(bytes: Buffer) -> AsyncTask<ExtractTextAllTask> {
    AsyncTask::new(ExtractTextAllTask {
        bytes: bytes.to_vec(),
    })
}

/// Rewriting a document parses and re-serialises the whole object tree, so it
/// belongs on the thread pool alongside rendering.
pub struct MergeTask {
    documents: Vec<Vec<u8>>,
}

impl Task for MergeTask {
    type Output = Vec<u8>;
    type JsValue = Buffer;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("merging documents", || {
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
}

impl Task for SelectPagesTask {
    type Output = Vec<u8>;
    type JsValue = Buffer;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("selecting pages", || {
            vellum_engine::select_pages(&self.bytes, &self.pages)
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(Buffer::from(output))
    }
}

pub struct SplitTask {
    bytes: Vec<u8>,
}

impl Task for SplitTask {
    type Output = Vec<Vec<u8>>;
    type JsValue = Vec<Buffer>;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("splitting a document", || vellum_engine::split(&self.bytes))
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output.into_iter().map(Buffer::from).collect())
    }
}

pub struct RotateTask {
    bytes: Vec<u8>,
    degrees: i64,
    pages: Option<Vec<u32>>,
}

impl Task for RotateTask {
    type Output = Vec<u8>;
    type JsValue = Buffer;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("rotating pages", || {
            vellum_engine::rotate(&self.bytes, self.degrees, self.pages.as_deref())
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(Buffer::from(output))
    }
}

#[napi(ts_return_type = "Promise<Buffer>")]
pub fn merge(documents: Vec<Buffer>) -> AsyncTask<MergeTask> {
    AsyncTask::new(MergeTask {
        documents: documents.iter().map(|document| document.to_vec()).collect(),
    })
}

#[napi(ts_return_type = "Promise<Buffer>")]
pub fn select_pages(bytes: Buffer, pages: Vec<u32>) -> AsyncTask<SelectPagesTask> {
    AsyncTask::new(SelectPagesTask {
        bytes: bytes.to_vec(),
        pages,
    })
}

#[napi(ts_return_type = "Promise<Buffer[]>")]
pub fn split(bytes: Buffer) -> AsyncTask<SplitTask> {
    AsyncTask::new(SplitTask {
        bytes: bytes.to_vec(),
    })
}

#[napi(ts_return_type = "Promise<Buffer>")]
pub fn rotate(bytes: Buffer, degrees: i64, pages: Option<Vec<u32>>) -> AsyncTask<RotateTask> {
    AsyncTask::new(RotateTask {
        bytes: bytes.to_vec(),
        degrees,
        pages,
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
}

pub struct StampTask {
    pdf: Vec<u8>,
    image: Vec<u8>,
    options: vellum_engine::StampOptions,
}

impl Task for StampTask {
    type Output = Vec<u8>;
    type JsValue = Buffer;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("stamping a document", || {
            vellum_engine::stamp_image(&self.pdf, &self.image, &self.options)
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(Buffer::from(output))
    }
}

#[napi(ts_return_type = "Promise<Buffer>")]
pub fn stamp(pdf: Buffer, image: Buffer, options: Option<StampOptions>) -> AsyncTask<StampTask> {
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
    })
}
