//! Placing an image onto the pages of an existing document.
//!
//! This is the signature a technician draws on a tablet, the photo attached to
//! an intervention report, the watermark on a draft.
//!
//! It goes through krilla's authoring path rather than hand-written content
//! streams: krilla's `pdf` feature can re-embed an existing page as a Form
//! XObject, so the original page is drawn onto a fresh one and the image goes
//! over it. Its README says embedding existing pages is out of scope; the
//! published `Cargo.toml` says otherwise, and that feature is what makes this
//! module possible.

use std::sync::Arc;

use hayro::hayro_interpret::hayro_syntax::Pdf;
use krilla::geom::{Size, Transform};
use krilla::image::Image;
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::pdf::PdfDocument;
use krilla::Data;
use krilla::Document as Authored;

/// Where and how an image is laid onto a page.
///
/// Coordinates are in points from the TOP-LEFT corner, the way a screen layout
/// is written — krilla's own convention, and the one a caller placing a
/// signature box on a form is already thinking in.
#[derive(Debug, Clone, Copy)]
pub struct StampOptions {
    /// Which page, addressed from zero. `None` stamps every page.
    pub page: Option<u32>,
    pub x: f32,
    pub y: f32,
    /// Drawn size. When both are absent the image keeps its pixel size in
    /// points; when one is given the other follows the aspect ratio.
    pub width: Option<f32>,
    pub height: Option<f32>,
    /// 0 is invisible, 1 is opaque. A watermark usually wants about 0.15.
    pub opacity: f32,
}

impl Default for StampOptions {
    fn default() -> Self {
        Self {
            page: None,
            x: 0.0,
            y: 0.0,
            width: None,
            height: None,
            opacity: 1.0,
        }
    }
}

/// Decode an image from its bytes, choosing the codec by signature.
///
/// Sniffed rather than taken on trust: a caller passing a JPEG named `.png`
/// would otherwise get an opaque decode failure.
fn decode_image(bytes: &[u8]) -> Result<Image, String> {
    let data = Data::from(bytes.to_vec());

    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Image::from_png(data, false).map_err(|error| format!("cannot read PNG: {error}"))
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Image::from_jpeg(data, false).map_err(|error| format!("cannot read JPEG: {error}"))
    } else {
        Err("unsupported image format — expected PNG or JPEG".to_string())
    }
}

/// Work out the drawn size from the options and the image's own proportions.
fn drawn_size(image: &Image, options: &StampOptions) -> Result<Size, String> {
    let (pixel_width, pixel_height) = image.size();
    let natural_width = pixel_width as f32;
    let natural_height = pixel_height as f32;
    if natural_width <= 0.0 || natural_height <= 0.0 {
        return Err("image has no area".to_string());
    }
    let ratio = natural_height / natural_width;

    let (width, height) = match (options.width, options.height) {
        (Some(width), Some(height)) => (width, height),
        (Some(width), None) => (width, width * ratio),
        (None, Some(height)) => (height / ratio, height),
        (None, None) => (natural_width, natural_height),
    };

    Size::from_wh(width, height)
        .ok_or_else(|| format!("stamp size must be positive and finite, got {width}x{height}"))
}

/// Draw `image` onto the pages of `pdf`.
pub fn stamp_image(pdf: &[u8], image: &[u8], options: &StampOptions) -> Result<Vec<u8>, String> {
    if !(0.0..=1.0).contains(&options.opacity) {
        return Err(format!(
            "opacity must be between 0 and 1, got {}",
            options.opacity
        ));
    }
    if !options.x.is_finite() || !options.y.is_finite() {
        return Err("stamp position must be finite".to_string());
    }

    let image = decode_image(image)?;
    let size = drawn_size(&image, options)?;
    let opacity = NormalizedF32::new(options.opacity)
        .ok_or_else(|| format!("opacity must be between 0 and 1, got {}", options.opacity))?;

    let source = Pdf::new(pdf.to_vec()).map_err(|error| format!("cannot read PDF: {error:?}"))?;
    let page_count = source.pages().len();
    if page_count == 0 {
        return Err("the document has no page".to_string());
    }
    if let Some(page) = options.page {
        let index = usize::try_from(page).map_err(|_| "page index out of range".to_string())?;
        if index >= page_count {
            return Err(format!(
                "page {} does not exist — the document has {page_count}",
                page + 1
            ));
        }
    }

    // Sizes are read before the Pdf is handed to krilla, which takes ownership
    // of it behind an Arc.
    let page_sizes: Vec<(f32, f32)> = source
        .pages()
        .iter()
        .map(|page| page.render_dimensions())
        .collect();
    let embedded = PdfDocument::new(Arc::new(source));

    let mut document = Authored::new();
    for (index, (width, height)) in page_sizes.iter().enumerate() {
        let page_size = Size::from_wh(*width, *height)
            .ok_or_else(|| format!("page {} has no area", index + 1))?;
        let mut page = document.start_page_with(PageSettings::new(page_size));
        let mut surface = page.surface();

        surface.draw_pdf_page(&embedded, page_size, index);

        let stamped = options.page.is_none_or(|wanted| wanted as usize == index);
        if stamped {
            surface.push_transform(&Transform::from_translate(options.x, options.y));
            surface.push_opacity(opacity);
            surface.draw_image(image.clone(), size);
            surface.pop();
            surface.pop();
        }

        surface.finish();
        page.finish();
    }

    document
        .finish()
        .map_err(|error| format!("cannot write PDF: {error}"))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use hayro::vello_cpu::color::PremulRgba8;
    use hayro::vello_cpu::Pixmap;

    use super::*;
    use crate::{create_blank, inspect, page_dimensions, render_page, ImageFormat, RenderOptions};

    const A4: (f32, f32) = (595.28, 841.89);
    const RED: PremulRgba8 = PremulRgba8 {
        r: 255,
        g: 0,
        b: 0,
        a: 255,
    };

    /// A solid PNG, built with the rasteriser we already depend on rather than
    /// a checked-in binary fixture.
    fn solid_png(width: u16, height: u16, colour: PremulRgba8) -> Vec<u8> {
        let mut pixmap = Pixmap::new(width, height);
        pixmap.data_mut().fill(colour);
        pixmap.into_png().expect("pixmap encodes to PNG")
    }

    /// Render a page and read one pixel back.
    ///
    /// The strongest check available here: the stamp is not asserted through
    /// the object tree but through what a reader actually paints.
    fn pixel_at(pdf: &[u8], page: u32, x: u32, y: u32) -> PremulRgba8 {
        let png = render_page(
            pdf,
            page,
            &RenderOptions {
                format: ImageFormat::Png,
                ..Default::default()
            },
        )
        .expect("rendering should succeed");

        let pixmap = Pixmap::from_png(Cursor::new(png)).expect("the render decodes");
        let index = (y * u32::from(pixmap.width()) + x) as usize;
        pixmap.data()[index]
    }

    fn is_red(pixel: PremulRgba8) -> bool {
        pixel.r > 200 && pixel.g < 60 && pixel.b < 60
    }

    fn is_white(pixel: PremulRgba8) -> bool {
        pixel.r > 240 && pixel.g > 240 && pixel.b > 240
    }

    #[test]
    fn stamps_an_image_where_it_is_asked_to() {
        let stamped = stamp_image(
            &create_blank(&[A4]).unwrap(),
            &solid_png(10, 10, RED),
            &StampOptions {
                x: 50.0,
                y: 60.0,
                width: Some(100.0),
                height: Some(80.0),
                ..Default::default()
            },
        )
        .expect("stamping should succeed");

        // Inside the stamp: coordinates count from the TOP-LEFT corner.
        assert!(
            is_red(pixel_at(&stamped, 0, 100, 100)),
            "the stamp should cover (100, 100)"
        );
        // Outside it, the page is untouched.
        assert!(
            is_white(pixel_at(&stamped, 0, 400, 700)),
            "the rest of the page should stay blank"
        );
        assert!(
            is_white(pixel_at(&stamped, 0, 20, 20)),
            "above and left of the stamp should stay blank"
        );
    }

    #[test]
    fn stamps_every_page_by_default() {
        let stamped = stamp_image(
            &create_blank(&[A4, A4]).unwrap(),
            &solid_png(10, 10, RED),
            &StampOptions {
                x: 40.0,
                y: 40.0,
                width: Some(60.0),
                height: Some(60.0),
                ..Default::default()
            },
        )
        .unwrap();

        for page in 0..2 {
            assert!(
                is_red(pixel_at(&stamped, page, 60, 60)),
                "page {page} should carry the stamp"
            );
        }
    }

    #[test]
    fn stamps_only_the_page_asked_for() {
        let stamped = stamp_image(
            &create_blank(&[A4, A4]).unwrap(),
            &solid_png(10, 10, RED),
            &StampOptions {
                page: Some(1),
                x: 40.0,
                y: 40.0,
                width: Some(60.0),
                height: Some(60.0),
                ..Default::default()
            },
        )
        .unwrap();

        assert!(
            is_white(pixel_at(&stamped, 0, 60, 60)),
            "page 1 was not selected"
        );
        assert!(
            is_red(pixel_at(&stamped, 1, 60, 60)),
            "page 2 was the one selected"
        );
    }

    #[test]
    fn keeps_the_pages_and_their_sizes() {
        let source = create_blank(&[A4, (419.53, 595.28)]).unwrap();

        let stamped =
            stamp_image(&source, &solid_png(10, 10, RED), &StampOptions::default()).unwrap();

        assert_eq!(inspect(&stamped).unwrap().page_count, 2);
        let sizes = page_dimensions(&stamped).unwrap();
        assert!((sizes[0].width - A4.0).abs() < 1.0);
        assert!(
            (sizes[1].width - 419.53).abs() < 1.0,
            "got {}",
            sizes[1].width
        );
    }

    /// Opacity is what separates a watermark from a redaction.
    #[test]
    fn a_translucent_stamp_lets_the_page_through() {
        let stamped = stamp_image(
            &create_blank(&[A4]).unwrap(),
            &solid_png(10, 10, RED),
            &StampOptions {
                x: 0.0,
                y: 0.0,
                width: Some(200.0),
                height: Some(200.0),
                opacity: 0.2,
                ..Default::default()
            },
        )
        .unwrap();

        let pixel = pixel_at(&stamped, 0, 100, 100);
        assert!(
            !is_red(pixel),
            "a 20% stamp should not read as solid red: {pixel:?}"
        );
        assert!(
            pixel.r > pixel.g && pixel.r > pixel.b,
            "it should still tint the page red: {pixel:?}"
        );
    }

    #[test]
    fn width_alone_keeps_the_aspect_ratio() {
        let image = solid_png(20, 10, RED);
        let stamped = stamp_image(
            &create_blank(&[A4]).unwrap(),
            &image,
            &StampOptions {
                x: 0.0,
                y: 0.0,
                width: Some(200.0),
                ..Default::default()
            },
        )
        .unwrap();

        // 20x10 drawn 200 wide is 100 tall: (150, 50) is inside, (150, 150) is not.
        assert!(is_red(pixel_at(&stamped, 0, 150, 50)));
        assert!(is_white(pixel_at(&stamped, 0, 150, 150)));
    }

    #[test]
    fn accepts_jpeg_as_well_as_png() {
        let jpeg = render_page(
            &create_blank(&[(20.0, 20.0)]).unwrap(),
            0,
            &RenderOptions {
                format: ImageFormat::Jpeg(80),
                ..Default::default()
            },
        )
        .unwrap();

        assert!(stamp_image(
            &create_blank(&[A4]).unwrap(),
            &jpeg,
            &StampOptions::default()
        )
        .is_ok());
    }

    #[test]
    fn refuses_an_image_that_is_neither_png_nor_jpeg() {
        let error = stamp_image(
            &create_blank(&[A4]).unwrap(),
            b"GIF89a and then some",
            &StampOptions::default(),
        )
        .expect_err("a GIF should be refused");

        assert!(error.contains("PNG or JPEG"), "got: {error}");
    }

    #[test]
    fn refuses_an_opacity_outside_zero_to_one() {
        for opacity in [-0.1, 1.5, f32::NAN] {
            let options = StampOptions {
                opacity,
                ..Default::default()
            };
            assert!(
                stamp_image(
                    &create_blank(&[A4]).unwrap(),
                    &solid_png(4, 4, RED),
                    &options
                )
                .is_err(),
                "opacity {opacity} should be refused"
            );
        }
    }

    #[test]
    fn refuses_a_page_that_does_not_exist() {
        let error = stamp_image(
            &create_blank(&[A4, A4]).unwrap(),
            &solid_png(4, 4, RED),
            &StampOptions {
                page: Some(7),
                ..Default::default()
            },
        )
        .expect_err("page index 7 should not exist");

        assert!(error.contains("page 8 does not exist"), "got: {error}");
    }

    #[test]
    fn refuses_bytes_that_are_not_a_pdf() {
        assert!(stamp_image(
            b"not a PDF",
            &solid_png(4, 4, RED),
            &StampOptions::default()
        )
        .is_err());
    }
}
