//! Rasterising PDF pages to images.
//!
//! `hayro` renders a page to a premultiplied RGBA pixmap and can encode PNG
//! itself. JPEG goes through `image`, because a scanned A4 at scale 2 weighs
//! several megabytes as PNG — the wrong artefact for a document preview.

use hayro::hayro_interpret::hayro_syntax::Pdf;
use hayro::hayro_interpret::InterpreterSettings;
use hayro::vello_cpu::color::{AlphaColor, PremulRgba8, Srgb};
use hayro::{render, RenderCache, RenderSettings};
use image::codecs::jpeg::JpegEncoder;
use image::{ExtendedColorType, ImageEncoder};

/// How the rasterised page should be encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    /// Quality 1-100. JPEG has no alpha channel, so a transparent background
    /// is composited onto white rather than silently turning black.
    Jpeg(u8),
}

/// Quality used when JPEG is asked for without one. High enough that a scanned
/// document stays readable, low enough that the file is worth choosing JPEG for.
pub const DEFAULT_JPEG_QUALITY: u8 = 82;

impl ImageFormat {
    /// Resolve a format name coming from a caller.
    ///
    /// Parsed here rather than at the binding boundary so the accepted
    /// spellings are pinned by tests, in one place, for every host.
    pub fn parse(name: &str, quality: Option<u8>) -> Result<Self, String> {
        match name.to_ascii_lowercase().as_str() {
            "png" => Ok(Self::Png),
            "jpeg" | "jpg" => Ok(Self::Jpeg(quality.unwrap_or(DEFAULT_JPEG_QUALITY))),
            other => Err(format!(
                "unknown image format {other:?} - expected \"png\" or \"jpeg\""
            )),
        }
    }
}

/// Parse a background colour: `#rgb`, `#rrggbb`, `#rrggbbaa`, or `transparent`.
///
/// The leading `#` is optional. Anything else is refused rather than silently
/// falling back to white, which would hide a typo in a config file.
pub fn parse_color(value: &str) -> Result<[u8; 4], String> {
    let trimmed = value.trim();
    if trimmed.eq_ignore_ascii_case("transparent") {
        return Ok([0, 0, 0, 0]);
    }

    let hex = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("invalid colour {value:?}"));
    }

    let byte = |at: usize| -> Result<u8, String> {
        u8::from_str_radix(&hex[at..at + 2], 16).map_err(|_| format!("invalid colour {value:?}"))
    };

    match hex.len() {
        // #rgb is expanded by repeating each digit, as CSS does: #f00 is #ff0000.
        3 => {
            let mut out = [255u8; 4];
            for (index, digit) in hex.chars().enumerate() {
                let nibble = u8::from_str_radix(&digit.to_string(), 16)
                    .map_err(|_| format!("invalid colour {value:?}"))?;
                out[index] = nibble * 17;
            }
            Ok(out)
        }
        6 => Ok([byte(0)?, byte(2)?, byte(4)?, 255]),
        8 => Ok([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        _ => Err(format!(
            "invalid colour {value:?} - expected #rgb, #rrggbb, #rrggbbaa or \"transparent\""
        )),
    }
}

/// Rasterising options.
#[derive(Debug, Clone, Copy)]
pub struct RenderOptions {
    /// Multiplier over the page's natural size, where 1.0 is 72 DPI.
    /// Ignored when `width` is set.
    pub scale: f32,
    /// Target width in pixels. Takes precedence over `scale`, because a caller
    /// asking for a 1200px preview should not have to know the page's size
    /// first.
    pub width: Option<u32>,
    pub format: ImageFormat,
    /// Background as RGBA. Defaults to opaque white: a PDF paints no
    /// background of its own, so rendering it transparent makes black text
    /// invisible over a dark viewer.
    pub background: [u8; 4],
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            scale: 1.0,
            width: None,
            format: ImageFormat::Png,
            background: [255, 255, 255, 255],
        }
    }
}

/// The natural size of a page in points, before any scaling.
#[derive(Debug, Clone, Copy)]
pub struct PageDimensions {
    pub width: f32,
    pub height: f32,
}

/// A dimension usable for rendering: finite and strictly positive.
///
/// Written as `is_finite() && > 0.0` rather than a negated comparison because
/// NaN fails every comparison silently — `NaN <= 0.0` is false, so a naive
/// bounds check would let it through and allocate a nonsense pixmap.
fn is_usable(value: f32) -> bool {
    value.is_finite() && value > 0.0
}

fn load(bytes: &[u8]) -> Result<Pdf, String> {
    Pdf::new(bytes.to_vec()).map_err(|error| format!("cannot read PDF: {error:?}"))
}

/// The natural size of every page, in points.
pub fn page_dimensions(bytes: &[u8]) -> Result<Vec<PageDimensions>, String> {
    let pdf = load(bytes)?;
    Ok(pdf
        .pages()
        .iter()
        .map(|page| {
            let (width, height) = page.render_dimensions();
            PageDimensions { width, height }
        })
        .collect())
}

/// Rasterise one page, addressed from zero.
pub fn render_page(
    bytes: &[u8],
    page_index: u32,
    options: &RenderOptions,
) -> Result<Vec<u8>, String> {
    let pdf = load(bytes)?;
    let pages = pdf.pages();
    let index = usize::try_from(page_index).map_err(|_| "page index out of range".to_string())?;
    let page = pages.get(index).ok_or_else(|| {
        // Reported in human numbering, not as the index: a caller who asked
        // for page 8 should not be told that "page 7" is missing.
        format!(
            "page {} does not exist — the document has {}",
            page_index + 1,
            pages.len()
        )
    })?;

    encode(&rasterise(page, options)?, options)
}

/// Rasterise every page, in document order.
pub fn render_all(bytes: &[u8], options: &RenderOptions) -> Result<Vec<Vec<u8>>, String> {
    let pdf = load(bytes)?;
    pdf.pages()
        .iter()
        .map(|page| encode(&rasterise(page, options)?, options))
        .collect()
}

/// A rasterised page: premultiplied RGBA pixels and their dimensions.
struct Raster {
    pixels: Vec<PremulRgba8>,
    width: u16,
    height: u16,
}

fn rasterise(
    page: &hayro::hayro_interpret::hayro_syntax::page::Page<'_>,
    options: &RenderOptions,
) -> Result<Raster, String> {
    let (natural_width, natural_height) = page.render_dimensions();
    if !is_usable(natural_width) || !is_usable(natural_height) {
        return Err("page has no renderable area".to_string());
    }

    // A target width is resolved to a scale rather than to hayro's viewport
    // width: the viewport crops, it does not resize, so setting it alone would
    // hand back the top-left corner of the page.
    let scale = match options.width {
        Some(width) if width > 0 => width as f32 / natural_width,
        Some(_) => return Err("target width must be greater than zero".to_string()),
        None => options.scale,
    };
    if !is_usable(scale) {
        return Err(format!("scale must be positive and finite, got {scale}"));
    }

    // u16 is hayro's pixmap limit, and the product is what actually allocates:
    // an unbounded scale is a memory-exhaustion vector on hostile input.
    let scaled_width = natural_width * scale;
    let scaled_height = natural_height * scale;
    if scaled_width > u16::MAX as f32 || scaled_height > u16::MAX as f32 {
        return Err(format!(
            "rendered size {}x{} exceeds the {}px limit — lower the scale or target width",
            scaled_width.floor(),
            scaled_height.floor(),
            u16::MAX
        ));
    }

    let [red, green, blue, alpha] = background_for(options);
    let settings = RenderSettings {
        x_scale: scale,
        y_scale: scale,
        width: None,
        height: None,
        bg_color: AlphaColor::<Srgb>::from_rgba8(red, green, blue, alpha),
    };

    let cache = RenderCache::new();
    let pixmap = render(page, &cache, &InterpreterSettings::default(), &settings);

    Ok(Raster {
        width: pixmap.width(),
        height: pixmap.height(),
        pixels: pixmap.data().to_vec(),
    })
}

/// JPEG cannot carry alpha, so a translucent background becomes opaque white
/// instead of compositing onto black at encode time.
fn background_for(options: &RenderOptions) -> [u8; 4] {
    match options.format {
        ImageFormat::Jpeg(_) if options.background[3] != 255 => [255, 255, 255, 255],
        _ => options.background,
    }
}

fn encode(raster: &Raster, options: &RenderOptions) -> Result<Vec<u8>, String> {
    match options.format {
        ImageFormat::Png => encode_png(raster),
        ImageFormat::Jpeg(quality) => encode_jpeg(raster, quality),
    }
}

fn encode_png(raster: &Raster) -> Result<Vec<u8>, String> {
    let mut pixmap = hayro::vello_cpu::Pixmap::new(raster.width, raster.height);
    pixmap.data_mut().copy_from_slice(&raster.pixels);
    pixmap
        .into_png()
        .map_err(|error| format!("cannot encode PNG: {error}"))
}

fn encode_jpeg(raster: &Raster, quality: u8) -> Result<Vec<u8>, String> {
    if quality == 0 || quality > 100 {
        return Err(format!("JPEG quality must be 1-100, got {quality}"));
    }

    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, quality)
        .write_image(
            &to_rgb8_over_white(&raster.pixels),
            u32::from(raster.width),
            u32::from(raster.height),
            ExtendedColorType::Rgb8,
        )
        .map_err(|error| format!("cannot encode JPEG: {error}"))?;
    Ok(out)
}

/// Flatten premultiplied RGBA onto white.
///
/// The channels are premultiplied, so compositing over an opaque white
/// background is `channel + (255 - alpha)` — no division, and no rounding
/// error from un-premultiplying first.
fn to_rgb8_over_white(pixels: &[PremulRgba8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels.len() * 3);
    for pixel in pixels {
        let uncovered = 255 - pixel.a;
        out.push(pixel.r.saturating_add(uncovered));
        out.push(pixel.g.saturating_add(uncovered));
        out.push(pixel.b.saturating_add(uncovered));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::create_blank;

    const A4: (f32, f32) = (595.28, 841.89);

    /// Width and height out of a PNG's IHDR chunk, which starts at byte 16.
    /// Read straight from the bytes so the assertions describe the file a
    /// consumer receives, not an intermediate the encoder happened to build.
    fn png_size(bytes: &[u8]) -> (u32, u32) {
        assert_eq!(&bytes[0..8], b"\x89PNG\r\n\x1a\n", "not a PNG");
        let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
        (width, height)
    }

    fn a4_document(pages: usize) -> Vec<u8> {
        create_blank(&vec![A4; pages]).expect("authoring should succeed")
    }

    #[test]
    fn renders_a_page_to_png_at_natural_size() {
        let png = render_page(&a4_document(1), 0, &RenderOptions::default())
            .expect("rendering should succeed");

        let (width, height) = png_size(&png);
        // A4 is 595.28 x 841.89pt; at scale 1 (72 DPI) that floors to 595x841.
        assert_eq!((width, height), (595, 841));
    }

    #[test]
    fn scale_multiplies_the_output() {
        let options = RenderOptions {
            scale: 2.0,
            ..Default::default()
        };
        let png = render_page(&a4_document(1), 0, &options).expect("rendering should succeed");

        let (width, height) = png_size(&png);
        assert_eq!((width, height), (1190, 1683));
    }

    #[test]
    fn a_target_width_is_honoured_exactly() {
        let options = RenderOptions {
            width: Some(1200),
            ..Default::default()
        };
        let png = render_page(&a4_document(1), 0, &options).expect("rendering should succeed");

        let (width, height) = png_size(&png);
        assert_eq!(width, 1200, "the requested width should be exact");
        // Aspect ratio preserved: 1200 * (841.89 / 595.28) ≈ 1697.
        assert!((1696..=1698).contains(&height), "got height {height}");
    }

    #[test]
    fn a_target_width_overrides_scale() {
        let options = RenderOptions {
            scale: 8.0,
            width: Some(600),
            ..Default::default()
        };
        let png = render_page(&a4_document(1), 0, &options).expect("rendering should succeed");

        assert_eq!(png_size(&png).0, 600);
    }

    #[test]
    fn renders_jpeg_when_asked() {
        let options = RenderOptions {
            format: ImageFormat::Jpeg(80),
            ..Default::default()
        };
        let jpeg = render_page(&a4_document(1), 0, &options).expect("rendering should succeed");

        assert_eq!(&jpeg[0..3], b"\xff\xd8\xff", "not a JPEG");
    }

    #[test]
    fn jpeg_decodes_back_to_the_requested_size() {
        let options = RenderOptions {
            width: Some(800),
            format: ImageFormat::Jpeg(80),
            ..Default::default()
        };
        let jpeg = render_page(&a4_document(1), 0, &options).expect("rendering should succeed");

        // Decoded rather than sniffed: magic bytes only prove the header was
        // written, not that the scan data describes the image we rendered.
        let decoded = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg)
            .expect("the JPEG we wrote should decode");
        assert_eq!(decoded.width(), 800);
        assert!(
            (1129..=1133).contains(&decoded.height()),
            "got {}",
            decoded.height()
        );
    }

    #[test]
    fn renders_every_page() {
        let images = render_all(&a4_document(3), &RenderOptions::default())
            .expect("rendering should succeed");

        assert_eq!(images.len(), 3);
        for image in &images {
            assert_eq!(png_size(image), (595, 841));
        }
    }

    #[test]
    fn reports_page_dimensions() {
        let dimensions = page_dimensions(&a4_document(2)).expect("should read dimensions");

        assert_eq!(dimensions.len(), 2);
        assert!((dimensions[0].width - A4.0).abs() < 0.5);
        assert!((dimensions[0].height - A4.1).abs() < 0.5);
    }

    #[test]
    fn refuses_a_page_that_does_not_exist() {
        let error = render_page(&a4_document(2), 7, &RenderOptions::default())
            .expect_err("page 7 of a 2-page document should fail");

        assert!(error.contains("does not exist"), "got: {error}");
        assert!(
            error.contains('2'),
            "the error should say how many: {error}"
        );
    }

    #[test]
    fn refuses_a_scale_that_would_exhaust_memory() {
        let options = RenderOptions {
            scale: 1_000.0,
            ..Default::default()
        };
        let error = render_page(&a4_document(1), 0, &options)
            .expect_err("a 595000px-wide render should be refused");

        assert!(error.contains("exceeds"), "got: {error}");
    }

    #[test]
    fn refuses_an_invalid_jpeg_quality() {
        let options = RenderOptions {
            format: ImageFormat::Jpeg(0),
            ..Default::default()
        };
        assert!(render_page(&a4_document(1), 0, &options).is_err());
    }

    #[test]
    fn parses_format_names() {
        assert_eq!(ImageFormat::parse("png", None), Ok(ImageFormat::Png));
        assert_eq!(ImageFormat::parse("PNG", None), Ok(ImageFormat::Png));
        assert_eq!(
            ImageFormat::parse("jpg", None),
            Ok(ImageFormat::Jpeg(DEFAULT_JPEG_QUALITY))
        );
        assert_eq!(
            ImageFormat::parse("jpeg", Some(60)),
            Ok(ImageFormat::Jpeg(60))
        );
        // Quality on PNG is not an error, it simply has no meaning there.
        assert_eq!(ImageFormat::parse("png", Some(60)), Ok(ImageFormat::Png));
        assert!(ImageFormat::parse("webp", None).is_err());
    }

    #[test]
    fn parses_colours() {
        assert_eq!(parse_color("#ffffff"), Ok([255, 255, 255, 255]));
        assert_eq!(parse_color("ffffff"), Ok([255, 255, 255, 255]));
        assert_eq!(parse_color("#f00"), Ok([255, 0, 0, 255]));
        assert_eq!(parse_color("#12345678"), Ok([0x12, 0x34, 0x56, 0x78]));
        assert_eq!(parse_color("transparent"), Ok([0, 0, 0, 0]));
        assert_eq!(parse_color("  TRANSPARENT "), Ok([0, 0, 0, 0]));
    }

    #[test]
    fn refuses_a_malformed_colour() {
        // A typo must not quietly become white — that is a wrong render nobody
        // reports until a document comes out with the wrong background.
        for value in ["#gggggg", "#ff", "#fffff", "white", ""] {
            assert!(parse_color(value).is_err(), "{value:?} should be refused");
        }
    }

    /// JPEG cannot carry alpha. Asking for a transparent background alongside
    /// it must composite onto white, not encode black.
    #[test]
    fn jpeg_forces_an_opaque_background() {
        let options = RenderOptions {
            format: ImageFormat::Jpeg(80),
            background: [0, 0, 0, 0],
            ..Default::default()
        };
        assert_eq!(background_for(&options), [255, 255, 255, 255]);

        let png_options = RenderOptions {
            background: [0, 0, 0, 0],
            ..Default::default()
        };
        assert_eq!(background_for(&png_options), [0, 0, 0, 0]);
    }

    /// The reason `is_usable` checks `is_finite` first: NaN fails every
    /// comparison, so a plain bounds check would let it through.
    #[test]
    fn refuses_a_scale_that_is_not_a_number() {
        for scale in [f32::NAN, f32::INFINITY, -1.0, 0.0] {
            let options = RenderOptions {
                scale,
                ..Default::default()
            };
            assert!(
                render_page(&a4_document(1), 0, &options).is_err(),
                "scale {scale} should be refused"
            );
        }
    }

    #[test]
    fn refuses_bytes_that_are_not_a_pdf() {
        assert!(page_dimensions(b"definitely not a PDF").is_err());
    }
}
