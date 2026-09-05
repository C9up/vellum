use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::collections::HashMap;
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
    /// The most pixels one page may rasterise to. 50 million by default —
    /// room for A4 at 600 DPI. A page declares its own size, so without a
    /// ceiling a document alone could ask for gigabytes.
    pub max_pixels: Option<u32>,
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
    if let Some(max) = options.max_pixels {
        if max == 0 {
            return Err("maxPixels must be greater than zero".to_string());
        }
        engine.max_pixels = max;
    }

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
}

pub struct StampTextTask {
    pdf: Vec<u8>,
    text: String,
    options: vellum_engine::TextStampOptions,
}

impl Task for StampTextTask {
    type Output = Vec<u8>;
    type JsValue = Buffer;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("writing text onto a document", || {
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

#[napi]
pub fn form_fields(bytes: Buffer) -> Result<Vec<FormField>> {
    let owned = bytes.to_vec();
    wrap(move || {
        vellum_engine::form_fields(&owned).map(|fields| {
            fields
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
                .collect()
        })
    })
}

pub struct FillFormTask {
    pdf: Vec<u8>,
    values: Vec<vellum_engine::FieldValue>,
}

impl Task for FillFormTask {
    type Output = Vec<u8>;
    type JsValue = Buffer;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("filling a form", || {
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
pub fn fill_form(pdf: Buffer, values: HashMap<String, String>) -> AsyncTask<FillFormTask> {
    AsyncTask::new(FillFormTask {
        pdf: pdf.to_vec(),
        values: values
            .into_iter()
            .map(|(name, value)| vellum_engine::FieldValue { name, value })
            .collect(),
    })
}

pub struct FlattenFormTask {
    pdf: Vec<u8>,
}

impl Task for FlattenFormTask {
    type Output = Vec<u8>;
    type JsValue = Buffer;

    fn compute(&mut self) -> Result<Self::Output> {
        guarded("flattening a form", || {
            vellum_engine::flatten_form(&self.pdf)
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(Buffer::from(output))
    }
}

/// Paint every field into the page and drop the interactive layer.
#[napi(ts_return_type = "Promise<Buffer>")]
pub fn flatten_form(pdf: Buffer) -> AsyncTask<FlattenFormTask> {
    AsyncTask::new(FlattenFormTask { pdf: pdf.to_vec() })
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
    /// The signature verifies against the certificate it carries.
    pub signature_verifies: bool,
    /// Who the certificate says signed.
    pub signer: Option<String>,
    /// When the signature says it was made.
    pub signed_at: Option<String>,
    /// An authority has vouched for when, so it outlives the certificate.
    pub timestamped: bool,
    /// A path was found from the signer's certificate to a trusted anchor.
    pub trusted: bool,
    /// That path, the signer first and the anchor last.
    pub chain: Vec<String>,
    /// Where the instant used to judge the path came from: `"timestamp"`,
    /// `"claimed"` or `"unknown"`.
    pub moment: String,
    /// That instant, in seconds since the epoch.
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
/// This establishes integrity and authorship, not trust: it does not ask
/// whether the certificate comes from an authority you accept, nor whether it
/// has been revoked.
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
/// `at` is the instant the document was signed: a certificate withdrawn after
/// that does not taint what it signed before.
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
