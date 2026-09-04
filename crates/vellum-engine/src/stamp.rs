//! Placing an image onto the pages of an existing document.
//!
//! This is the signature a technician draws on a tablet, the photo attached to
//! an intervention report, the watermark on a draft.
//!
//! The picture is written into the document that already exists, as an image
//! XObject named in the page's resources and drawn from its content stream.
//! The obvious alternative — re-authoring the file and redrawing each page
//! onto a fresh one — loses everything the page structure carries: the
//! interactive form, the annotations, the links. A signature is usually
//! stamped onto exactly the kind of document that has all three.

use lopdf::{dictionary, Document, Object, ObjectId, Stream};

use crate::edit::flatten_inheritance;
use crate::page::{isolate_existing_contents, page_height, register_resources};

const IMAGE_KEY: &str = "VellumStamp";
const STATE_KEY: &str = "VellumStampState";

/// Where and how an image is laid onto a page.
///
/// Coordinates are in points from the TOP-LEFT corner, the way a screen layout
/// is written — the convention a caller placing a signature box on a form is
/// already thinking in.
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

/// A picture ready to be written into a document.
struct Picture {
    width: u32,
    height: u32,
    image: Stream,
    /// The alpha channel, as a soft mask. PNG only, and only when the picture
    /// is not fully opaque.
    mask: Option<Stream>,
}

/// Read a JPEG's frame header: its size, and how many colour components it
/// carries.
///
/// Only the header is parsed. The compressed bytes go into the document
/// untouched, as `DCTDecode`, so a photograph stays the size it arrived at
/// instead of being inflated into raw samples — which is the difference
/// between a 2MB intervention report and a 30MB one.
fn jpeg_frame(bytes: &[u8]) -> Result<(u32, u32, u8), String> {
    let malformed = || "cannot read JPEG: no frame header".to_string();
    let mut at = 2; // past the start-of-image marker

    loop {
        let (&0xFF, Some(&marker)) = (bytes.get(at).ok_or_else(malformed)?, bytes.get(at + 1))
        else {
            return Err(malformed());
        };

        // Restart markers and TEM carry no payload.
        if (0xD0..=0xD9).contains(&marker) || marker == 0x01 {
            at += 2;
            continue;
        }

        let length = match (bytes.get(at + 2), bytes.get(at + 3)) {
            (Some(high), Some(low)) => usize::from(u16::from_be_bytes([*high, *low])),
            _ => return Err(malformed()),
        };

        // Every start-of-frame marker but the three that share the range and
        // mean something else.
        if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let field = |offset: usize| -> Result<u16, String> {
                match (bytes.get(at + offset), bytes.get(at + offset + 1)) {
                    (Some(high), Some(low)) => Ok(u16::from_be_bytes([*high, *low])),
                    _ => Err(malformed()),
                }
            };
            let height = u32::from(field(5)?);
            let width = u32::from(field(7)?);
            let components = *bytes.get(at + 9).ok_or_else(malformed)?;
            return Ok((width, height, components));
        }

        at += 2 + length;
    }
}

fn jpeg_picture(bytes: &[u8]) -> Result<Picture, String> {
    let (width, height, components) = jpeg_frame(bytes)?;
    let colour_space = match components {
        1 => "DeviceGray",
        3 => "DeviceRGB",
        // A CMYK JPEG needs the Adobe colour transform and an inverted
        // /Decode. Getting that wrong turns a photograph into its negative
        // without saying so, which is worse than refusing it.
        4 => return Err("cannot stamp a CMYK JPEG — convert it to RGB first".to_string()),
        other => {
            return Err(format!(
                "cannot stamp a JPEG with {other} colour components"
            ))
        }
    };

    let image = Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => width,
            "Height" => height,
            "ColorSpace" => colour_space,
            "BitsPerComponent" => 8,
            "Filter" => "DCTDecode",
        },
        bytes.to_vec(),
    )
    // The bytes are already compressed; re-encoding them would only make the
    // file bigger and the filter chain wrong.
    .with_compression(false);

    Ok(Picture {
        width,
        height,
        image,
        mask: None,
    })
}

fn png_picture(bytes: &[u8]) -> Result<Picture, String> {
    let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .map_err(|error| format!("cannot read PNG: {error}"))?;
    let (width, height) = (decoded.width(), decoded.height());
    let pixels = decoded.to_rgba8();

    let mut colour = Vec::with_capacity(pixels.len() / 4 * 3);
    let mut alpha = Vec::with_capacity(pixels.len() / 4);
    for pixel in pixels.pixels() {
        colour.extend_from_slice(&pixel.0[..3]);
        alpha.push(pixel.0[3]);
    }

    let mut image = Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => width,
            "Height" => height,
            "ColorSpace" => "DeviceRGB",
            "BitsPerComponent" => 8,
        },
        colour,
    );
    image
        .compress()
        .map_err(|error| format!("cannot compress the image: {error}"))?;

    // A signature drawn on a tablet is transparent everywhere but the stroke,
    // so the alpha channel is the whole point of accepting PNG.
    let mask = alpha.iter().any(|value| *value != 255).then(|| {
        let mut mask = Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => width,
                "Height" => height,
                "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => 8,
            },
            alpha,
        );
        let _ = mask.compress();
        mask
    });

    Ok(Picture {
        width,
        height,
        image,
        mask,
    })
}

/// Read an image, choosing the codec by signature.
///
/// Sniffed rather than taken on trust: a caller passing a JPEG named `.png`
/// would otherwise get an opaque decode failure.
fn read_picture(bytes: &[u8]) -> Result<Picture, String> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        png_picture(bytes)
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        jpeg_picture(bytes)
    } else {
        Err("unsupported image format — expected PNG or JPEG".to_string())
    }
}

/// Work out the drawn size from the options and the image's own proportions.
fn drawn_size(picture: &Picture, options: &StampOptions) -> Result<(f32, f32), String> {
    let natural_width = picture.width as f32;
    let natural_height = picture.height as f32;
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

    if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
        return Err(format!(
            "stamp size must be positive and finite, got {width}x{height}"
        ));
    }
    Ok((width, height))
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

    let picture = read_picture(image)?;
    let (width, height) = drawn_size(&picture, options)?;

    let mut document =
        Document::load_mem(pdf).map_err(|error| format!("cannot read PDF: {error}"))?;
    // Inherited attributes are materialised before the pages are touched, so
    // reading a page's MediaBox and Resources does not depend on its ancestry.
    flatten_inheritance(&mut document);

    let pages = document.get_pages();
    if pages.is_empty() {
        return Err("the document has no page".to_string());
    }
    if let Some(page) = options.page {
        if usize::try_from(page).is_err() || page as usize >= pages.len() {
            return Err(format!(
                "page {} does not exist — the document has {}",
                page + 1,
                pages.len()
            ));
        }
    }

    let targets: Vec<ObjectId> = pages
        .into_iter()
        .enumerate()
        .filter(|(index, _)| options.page.is_none_or(|wanted| wanted as usize == *index))
        .map(|(_, (_, page_id))| page_id)
        .collect();

    // The picture is written once and referenced from every page it appears
    // on, rather than embedded again for each.
    let Picture { image, mask, .. } = picture;
    let mut image = image;
    if let Some(mask) = mask {
        let mask_id = document.add_object(Object::Stream(mask));
        image.dict.set("SMask", Object::Reference(mask_id));
    }
    let image_id = document.add_object(Object::Stream(image));

    let state_id = (options.opacity < 1.0).then(|| {
        document.add_object(dictionary! {
            "Type" => "ExtGState",
            "ca" => Object::Real(options.opacity),
        })
    });

    for page_id in targets {
        // The y a caller gives is the TOP of the picture, measured down from
        // the top of the page; PDF wants the bottom edge, measured up.
        let bottom = page_height(&document, page_id) - options.y - height;

        let mut content: Vec<u8> = Vec::new();
        content.extend_from_slice(b"q\n");
        if state_id.is_some() {
            content.extend_from_slice(format!("/{STATE_KEY} gs\n").as_bytes());
        }
        // An image is drawn into the unit square, so the transform carries the
        // whole of its size and position.
        content.extend_from_slice(
            format!(
                "{width} 0 0 {height} {} {bottom} cm\n/{IMAGE_KEY} Do\nQ\n",
                options.x
            )
            .as_bytes(),
        );

        register_resources(
            &mut document,
            page_id,
            "XObject",
            &[(IMAGE_KEY.to_string(), image_id)],
        )?;
        if let Some(state_id) = state_id {
            register_resources(
                &mut document,
                page_id,
                "ExtGState",
                &[(STATE_KEY.to_string(), state_id)],
            )?;
        }
        isolate_existing_contents(&mut document, page_id)?;
        document
            .add_page_contents(page_id, content)
            .map_err(|error| format!("cannot write onto the page: {error}"))?;
    }

    let mut out = Vec::new();
    document
        .save_to(&mut out)
        .map_err(|error| format!("cannot write PDF: {error}"))?;
    Ok(out)
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

    /// The reason this module stopped going through the authoring path.
    /// Re-authoring the document dropped every field it had, silently, and a
    /// signature is stamped onto exactly the kind of document that has them.
    #[test]
    fn stamping_keeps_the_interactive_form() {
        let source = crate::fill::tests::form_document();
        let before = crate::form_fields(&source).expect("the fixture has a form");
        assert!(!before.is_empty(), "the fixture has fields to lose");

        let stamped = stamp_image(
            &source,
            &solid_png(4, 4, RED),
            &StampOptions {
                x: 10.0,
                y: 10.0,
                width: Some(20.0),
                ..Default::default()
            },
        )
        .expect("stamping should succeed");

        let after = crate::form_fields(&stamped).expect("the result parses");
        assert_eq!(
            after.len(),
            before.len(),
            "the form must survive being stamped on"
        );
    }

    /// And so must everything else the page structure carries.
    #[test]
    fn stamping_keeps_the_other_annotations() {
        let source = crate::fill::tests::form_document();
        let stamped = stamp_image(
            &source,
            &solid_png(4, 4, RED),
            &StampOptions {
                x: 10.0,
                y: 10.0,
                width: Some(20.0),
                ..Default::default()
            },
        )
        .expect("stamping should succeed");

        let document = lopdf::Document::load_mem(&stamped).expect("the result parses");
        let page_id = *document
            .get_pages()
            .values()
            .next()
            .expect("the document has a page");
        let annotations = document
            .get_dictionary(page_id)
            .expect("the page reads")
            .get(b"Annots")
            .and_then(|annots| annots.as_array())
            .expect("the page kept its annotations");
        assert_eq!(annotations.len(), 5, "every widget is still on the page");
    }

    /// A signature drawn on a tablet is transparent everywhere but the stroke.
    #[test]
    fn a_transparent_png_lets_the_page_through() {
        let mut pixmap = Pixmap::new(2, 1);
        let transparent = PremulRgba8 {
            r: 0,
            g: 0,
            b: 0,
            a: 0,
        };
        pixmap.data_mut()[0] = RED;
        pixmap.data_mut()[1] = transparent;
        let png = pixmap.into_png().expect("pixmap encodes to PNG");

        let stamped = stamp_image(
            &create_blank(&[A4]).unwrap(),
            &png,
            &StampOptions {
                x: 100.0,
                y: 100.0,
                width: Some(40.0),
                height: Some(20.0),
                ..Default::default()
            },
        )
        .expect("stamping should succeed");

        assert!(is_red(pixel_at(&stamped, 0, 110, 110)), "the opaque half");
        assert!(
            is_white(pixel_at(&stamped, 0, 130, 110)),
            "and the page showing through the transparent one"
        );
    }

    /// A JPEG goes into the document untouched. Decoding and re-storing it as
    /// raw samples would turn a photo report into something nobody can email.
    #[test]
    fn a_jpeg_is_not_inflated_on_the_way_in() {
        let png = solid_png(400, 400, RED);
        let decoded = image::load_from_memory(&png).expect("the fixture decodes");
        let mut jpeg = Vec::new();
        decoded
            .write_to(&mut Cursor::new(&mut jpeg), image::ImageFormat::Jpeg)
            .expect("the fixture encodes as JPEG");

        let blank = create_blank(&[A4]).unwrap();
        let stamped =
            stamp_image(&blank, &jpeg, &StampOptions::default()).expect("stamping should succeed");

        let raw_samples = 400 * 400 * 3;
        assert!(
            stamped.len() < blank.len() + jpeg.len() + 4096,
            "the JPEG should go in as it came, got {} bytes for a {} byte picture",
            stamped.len(),
            jpeg.len()
        );
        assert!(
            stamped.len() < raw_samples,
            "and certainly not as {raw_samples} bytes of samples"
        );
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
