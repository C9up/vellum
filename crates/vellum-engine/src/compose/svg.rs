//! SVG drawn into a page, as vectors.
//!
//! usvg's defaults are for trusted files: an `<image>` may name any file on
//! the machine and usvg reads it, and an image inlined as a `data:` URL goes
//! to krilla, which allocates it from whatever size its header claims. Here
//! no file is ever read, a nested SVG image is not followed, and an inlined
//! image is bounded by its header as a page image is. Text in the SVG uses
//! the fonts the document loaded.

use std::sync::Arc;

use krilla::geom::Size;
use usvg::{ImageHrefResolver, ImageKind, Options, Tree};

use super::images::MAX_IMAGE_PIXELS;
use super::ops::Op;
use super::{ComposeError, Composer};

/// Larger than any drawing a document has a use for.
pub const MAX_SVG_BYTES: usize = 16 * 1024 * 1024;
/// Drawings in one document.
pub const MAX_SVGS: usize = 10_000;

fn invalid(message: impl Into<String>) -> ComposeError {
    ComposeError::new("INVALID_SVG", message)
}

/// An inlined image, if its header's size is within bounds.
fn bounded(data: Arc<Vec<u8>>, kind: fn(Arc<Vec<u8>>) -> ImageKind) -> Option<ImageKind> {
    let size = imagesize::blob_size(&data).ok()?;
    let pixels = (size.width as u64).saturating_mul(size.height as u64);
    (pixels > 0 && pixels <= MAX_IMAGE_PIXELS).then(|| kind(data))
}

fn resolver<'a>() -> ImageHrefResolver<'a> {
    ImageHrefResolver {
        resolve_data: Box::new(|mime: &str, data: Arc<Vec<u8>>, _: &Options| match mime {
            "image/jpg" | "image/jpeg" => bounded(data, ImageKind::JPEG),
            "image/png" => bounded(data, ImageKind::PNG),
            "image/gif" => bounded(data, ImageKind::GIF),
            "image/webp" => bounded(data, ImageKind::WEBP),
            _ => None,
        }),
        // Never a file: an SVG is often someone else's.
        resolve_string: Box::new(|_: &str, _: &Options| None),
    }
}

impl Composer {
    /// Parse an SVG and answer its handle and its size in points.
    pub fn load_svg(&mut self, bytes: &[u8]) -> Result<(usize, f32, f32), ComposeError> {
        if bytes.len() > MAX_SVG_BYTES {
            return Err(invalid(format!(
                "an SVG of {} bytes is over the limit of {MAX_SVG_BYTES}",
                bytes.len()
            )));
        }
        if self.svgs.len() >= MAX_SVGS {
            return Err(invalid(format!(
                "a document takes at most {MAX_SVGS} SVG drawings"
            )));
        }
        let mut fonts = usvg::fontdb::Database::new();
        for font in &self.fonts {
            fonts.load_font_data(font.bytes().to_vec());
        }
        let family = fonts
            .faces()
            .next()
            .and_then(|face| face.families.first())
            .map(|(name, _)| name.clone());
        let mut options = Options {
            resources_dir: None,
            image_href_resolver: resolver(),
            fontdb: Arc::new(fonts),
            ..Options::default()
        };
        if let Some(family) = family {
            options.font_family = family;
        }
        let tree = Tree::from_data(bytes, &options)
            .map_err(|error| invalid(format!("the SVG cannot be read: {error}")))?;
        let (width, height) = (tree.size().width(), tree.size().height());
        if !(width > 0.0 && height > 0.0 && width.is_finite() && height.is_finite()) {
            return Err(invalid("the SVG has no size"));
        }
        self.svgs.push(tree);
        Ok((self.svgs.len() - 1, width, height))
    }

    /// Draw a loaded SVG into the box `x`, `y`, `width`, `height` (user space).
    pub fn draw_svg(
        &mut self,
        handle: usize,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> Result<(), ComposeError> {
        if handle >= self.svgs.len() {
            return Err(invalid("no SVG has this handle"));
        }
        if ![x, y, width, height].iter().all(|value| value.is_finite()) {
            return Err(ComposeError::new(
                "INVALID_OPTIONS",
                "an SVG's box must be finite numbers",
            ));
        }
        let Some(size) = Size::from_wh(width, height) else {
            return Ok(());
        };
        let ctm = super::graphics::concat(self.state.ctm, [1.0, 0.0, 0.0, 1.0, x, y]);
        let op = Op::Svg {
            svg: handle,
            size,
            ctm,
            clips: self.state.clips.clone(),
        };
        self.page_mut()?.ops.push(op);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::{DocumentSettings, PageSetup, Sides};

    fn document() -> Composer {
        let mut document = Composer::new(DocumentSettings::default()).unwrap();
        document
            .add_page(PageSetup {
                width: 100.0,
                height: 100.0,
                margins: Sides {
                    top: 0.0,
                    right: 0.0,
                    bottom: 0.0,
                    left: 0.0,
                },
                user_unit: 1.0,
            })
            .unwrap();
        document
    }

    #[test]
    fn draws_an_svg_as_vectors_where_it_is_told() {
        let mut document = document();
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="red"/></svg>"#;
        let (handle, width, height) = document.load_svg(svg).unwrap();
        assert_eq!((width, height), (10.0, 10.0));
        document.draw_svg(handle, 50.0, 50.0, 40.0, 40.0).unwrap();
        let bytes = document.end().unwrap();
        let png = crate::render_page(
            &bytes,
            0,
            &crate::RenderOptions {
                format: crate::ImageFormat::Png,
                ..Default::default()
            },
        )
        .unwrap();
        let pixmap = hayro::vello_cpu::Pixmap::from_png(std::io::Cursor::new(png)).unwrap();
        let at = |x: u32, y: u32| pixmap.data()[(y * u32::from(pixmap.width()) + x) as usize];
        assert!(
            at(70, 70).r > 200 && at(70, 70).g < 60,
            "red inside the box"
        );
        assert!(at(20, 20).g > 200, "blank outside it");
    }

    #[test]
    fn reads_no_file_and_bounds_inlined_images() {
        let mut document = document();
        // A file named by the SVG is not read, whatever it is.
        let file = br#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="10" height="10"><image xlink:href="/etc/passwd" width="10" height="10"/></svg>"#;
        assert!(document.load_svg(file).is_ok());
        assert!(
            document.svgs[0].root().children().is_empty(),
            "the image is dropped"
        );

        // A PNG header claiming 100 000 x 100 000 pixels is dropped too.
        let mut png = Vec::new();
        image::RgbImage::new(1, 1)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        png[16..20].copy_from_slice(&100_000u32.to_be_bytes());
        png[20..24].copy_from_slice(&100_000u32.to_be_bytes());
        let encoded = base64_encode(&png);
        let bomb = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="10" height="10"><image xlink:href="data:image/png;base64,{encoded}" width="10" height="10"/></svg>"#
        );
        let (handle, _, _) = document.load_svg(bomb.as_bytes()).unwrap();
        assert!(document.svgs[handle].root().children().is_empty());

        assert!(document.load_svg(b"<html>not an svg</html>").is_err());
    }

    fn base64_encode(bytes: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            for index in 0..4 {
                if index <= chunk.len() {
                    out.push(TABLE[((n >> (18 - index * 6)) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }
}
