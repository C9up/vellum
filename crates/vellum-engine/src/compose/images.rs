//! Images placed on a page: PNG, JPEG, GIF and WebP.
//!
//! krilla decodes an image only when the document is written, and puts no
//! bound on it. An upload is untrusted, so the size is read from the header
//! and bounded first, and a PNG, GIF or WebP — which krilla has to decode
//! into samples anyway — is decoded once here, under limits, so a broken or
//! hostile file is refused where it is given rather than when the document
//! ends. A JPEG goes into the PDF as it is, so only its header is read.

use std::io::Cursor;
use std::sync::Arc;

use krilla::geom::Size;
use krilla::image::Image;

use super::graphics::{concat, Matrix};
use super::ops::Op;
use super::{ComposeError, Composer};

/// Larger than any photograph a document has a use for.
pub const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
/// 64 megapixels: 8000 by 8000.
pub const MAX_IMAGE_PIXELS: u64 = 64_000_000;
/// Images, each, in one document.
pub const MAX_IMAGES: usize = 65_536;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Png,
    Jpeg,
    Gif,
    WebP,
}

pub struct LoadedImage {
    data: Arc<Vec<u8>>,
    pub krilla: Image,
    pub width: u32,
    pub height: u32,
    /// The EXIF orientation of a JPEG, 1 to 8; 1 for everything else.
    pub orientation: u8,
}

fn invalid(message: impl Into<String>) -> ComposeError {
    ComposeError::new("INVALID_IMAGE", message)
}

pub fn kind(bytes: &[u8]) -> Option<ImageKind> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageKind::Png)
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(ImageKind::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(ImageKind::Gif)
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(ImageKind::WebP)
    } else {
        None
    }
}

/// Decode under limits, to refuse what krilla would fail on later.
fn validate(bytes: &[u8], kind: ImageKind) -> Result<(), ComposeError> {
    let format = match kind {
        ImageKind::Png => image::ImageFormat::Png,
        ImageKind::Gif => image::ImageFormat::Gif,
        ImageKind::WebP => image::ImageFormat::WebP,
        ImageKind::Jpeg => return Ok(()),
    };
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_IMAGE_PIXELS * 8);
    reader.limits(limits);
    reader
        .decode()
        .map(drop)
        .map_err(|error| invalid(format!("the image cannot be decoded: {error}")))
}

/// The EXIF orientation of a JPEG (tag 0x0112 of IFD0), or 1.
pub fn jpeg_orientation(bytes: &[u8]) -> u8 {
    let mut offset = 2;
    while offset + 4 <= bytes.len() {
        if bytes[offset] != 0xff {
            return 1;
        }
        let marker = bytes[offset + 1];
        // Start of scan: the metadata is over.
        if marker == 0xda || marker == 0xd9 {
            return 1;
        }
        let length = usize::from(u16::from_be_bytes([bytes[offset + 2], bytes[offset + 3]]));
        let segment = bytes.get(offset + 4..offset + 2 + length).unwrap_or(&[]);
        if marker == 0xe1 && segment.starts_with(b"Exif\0\0") {
            return exif_orientation(&segment[6..]).unwrap_or(1);
        }
        offset += 2 + length;
    }
    1
}

fn exif_orientation(tiff: &[u8]) -> Option<u8> {
    let little = match tiff.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let raw = [*tiff.get(at)?, *tiff.get(at + 1)?];
        Some(if little {
            u16::from_le_bytes(raw)
        } else {
            u16::from_be_bytes(raw)
        })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let raw = [
            *tiff.get(at)?,
            *tiff.get(at + 1)?,
            *tiff.get(at + 2)?,
            *tiff.get(at + 3)?,
        ];
        Some(if little {
            u32::from_le_bytes(raw)
        } else {
            u32::from_be_bytes(raw)
        })
    };
    let ifd = usize::try_from(u32_at(4)?).ok()?;
    let entries = usize::from(u16_at(ifd)?);
    for index in 0..entries.min(512) {
        let entry = ifd + 2 + index * 12;
        if u16_at(entry)? == 0x0112 {
            let value = u16_at(entry + 8)?;
            return u8::try_from(value)
                .ok()
                .filter(|value| (1..=8).contains(value));
        }
    }
    None
}

/// Where the stored image goes in a display box of `width` × `height`,
/// for each EXIF orientation: the stored image is drawn into `[0, w] ×
/// [0, h]` — `w`, `h` swapped past 4 — and this maps it onto the box.
pub fn orientation_matrix(orientation: u8, width: f32, height: f32) -> Matrix {
    match orientation {
        2 => [-1.0, 0.0, 0.0, 1.0, width, 0.0],
        3 => [-1.0, 0.0, 0.0, -1.0, width, height],
        4 => [1.0, 0.0, 0.0, -1.0, 0.0, height],
        5 => [0.0, 1.0, 1.0, 0.0, 0.0, 0.0],
        6 => [0.0, 1.0, -1.0, 0.0, width, 0.0],
        7 => [0.0, -1.0, -1.0, 0.0, width, height],
        8 => [0.0, -1.0, 1.0, 0.0, 0.0, height],
        _ => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
    }
}

impl LoadedImage {
    pub fn load(bytes: Vec<u8>) -> Result<Self, ComposeError> {
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(invalid(format!(
                "an image of {} bytes is over the limit of {MAX_IMAGE_BYTES}",
                bytes.len()
            )));
        }
        let kind = kind(&bytes)
            .ok_or_else(|| invalid("unknown image format: PNG, JPEG, GIF and WebP are read"))?;
        let data = Arc::new(bytes);
        let krilla = match kind {
            ImageKind::Png => Image::from_png(data.clone().into(), true),
            ImageKind::Jpeg => Image::from_jpeg(data.clone().into(), true),
            ImageKind::Gif => Image::from_gif(data.clone().into(), true),
            ImageKind::WebP => Image::from_webp(data.clone().into(), true),
        }
        .map_err(|error| invalid(format!("the image cannot be read: {error}")))?;
        let (width, height) = krilla.size();
        if width == 0 || height == 0 {
            return Err(invalid("the image is empty"));
        }
        if u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS {
            return Err(invalid(format!(
                "an image of {width} x {height} pixels is over the limit of {MAX_IMAGE_PIXELS}"
            )));
        }
        validate(&data, kind)?;
        let orientation = match kind {
            ImageKind::Jpeg => jpeg_orientation(&data),
            _ => 1,
        };
        Ok(Self {
            data,
            krilla,
            width,
            height,
            orientation,
        })
    }
}

impl Composer {
    /// Load an image and answer its handle; the same bytes answer the same
    /// handle, so an image drawn twice is embedded once.
    pub fn load_image(&mut self, bytes: Vec<u8>) -> Result<usize, ComposeError> {
        if let Some(index) = self.images.iter().position(|image| *image.data == bytes) {
            return Ok(index);
        }
        if self.images.len() >= MAX_IMAGES {
            return Err(invalid(format!(
                "a document takes at most {MAX_IMAGES} images"
            )));
        }
        let image = LoadedImage::load(bytes)?;
        self.images.push(image);
        Ok(self.images.len() - 1)
    }

    pub fn image(&self, handle: usize) -> Result<&LoadedImage, ComposeError> {
        self.images
            .get(handle)
            .ok_or_else(|| invalid("no image has this handle"))
    }

    /// Draw an image into the box `x`, `y`, `width`, `height` (user space),
    /// turned by its `orientation`, at `opacity` times the fill opacity.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_image(
        &mut self,
        handle: usize,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        orientation: u8,
        opacity: f32,
    ) -> Result<(), ComposeError> {
        self.image(handle)?;
        if ![x, y, width, height, opacity]
            .iter()
            .all(|value| value.is_finite())
        {
            return Err(ComposeError::new(
                "INVALID_OPTIONS",
                "an image's box must be finite numbers",
            ));
        }
        let (drawn_width, drawn_height) = if orientation > 4 {
            (height, width)
        } else {
            (width, height)
        };
        let Some(size) = Size::from_wh(drawn_width.abs(), drawn_height.abs()) else {
            // Zero wide or high: nothing to see.
            return Ok(());
        };
        let placed = concat(self.state.ctm, [1.0, 0.0, 0.0, 1.0, x, y]);
        let ctm = concat(placed, orientation_matrix(orientation, width, height));
        let op = Op::Image {
            image: handle,
            size,
            ctm,
            opacity: super::graphics::opacity(opacity),
            clips: self.state.clips.clone(),
        };
        self.page_mut()?.ops.push(op);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbImage::from_pixel(width, height, image::Rgb([255, 0, 0]))
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn reads_the_formats_it_names() {
        let image = LoadedImage::load(png(3, 2)).unwrap();
        assert_eq!((image.width, image.height, image.orientation), (3, 2, 1));
        assert!(
            LoadedImage::load(b"GIF89a....".to_vec()).is_err(),
            "a broken GIF"
        );
        assert!(LoadedImage::load(b"BM not supported".to_vec()).is_err());
    }

    #[test]
    fn refuses_a_truncated_png_where_it_is_given() {
        let mut bytes = png(64, 64);
        bytes.truncate(bytes.len() - 30);
        assert_eq!(
            LoadedImage::load(bytes).err().map(|e| e.code),
            Some("INVALID_IMAGE")
        );
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn refuses_a_header_claiming_too_many_pixels() {
        // A PNG header for 100 000 x 100 000 pixels and almost no data: a
        // decompression bomb in waiting.
        let mut bytes = png(1, 1);
        bytes[16..20].copy_from_slice(&100_000u32.to_be_bytes());
        bytes[20..24].copy_from_slice(&100_000u32.to_be_bytes());
        // The chunk's CRC covers its type and data, bytes 12 to 29.
        let crc = crc32(&bytes[12..29]);
        bytes[29..33].copy_from_slice(&crc.to_be_bytes());
        let error = LoadedImage::load(bytes).err().map(|error| error.message);
        assert!(error.is_some_and(|message| message.contains("over the limit")));
    }

    #[test]
    fn reads_the_exif_orientation_of_a_jpeg() {
        // SOI, APP1 "Exif" with a big-endian TIFF holding one IFD entry:
        // orientation (0x0112), SHORT, 1 value, 6.
        let mut tiff = b"MM\0\x2a\0\0\0\x08".to_vec();
        tiff.extend_from_slice(&[0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0, 0, 0, 0, 0]);
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff);
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1];
        jpeg.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        jpeg.extend_from_slice(&app1);
        jpeg.extend_from_slice(&[0xff, 0xda, 0, 2]);
        assert_eq!(jpeg_orientation(&jpeg), 6);
        assert_eq!(jpeg_orientation(&[0xff, 0xd8, 0xff, 0xda, 0, 2]), 1);
        assert_eq!(
            jpeg_orientation(&[0xff, 0xd8, 0xff, 0xe1, 0xff, 0xff]),
            1,
            "a segment past the end"
        );
    }

    /// 40 x 20, red on the left, blue on the right, as a JPEG marked with
    /// an EXIF orientation.
    fn halves_jpeg(orientation: u16) -> Vec<u8> {
        let mut encoded = Vec::new();
        image::RgbImage::from_fn(40, 20, |x, _| {
            if x < 20 {
                image::Rgb([255, 0, 0])
            } else {
                image::Rgb([0, 0, 255])
            }
        })
        .write_to(&mut Cursor::new(&mut encoded), image::ImageFormat::Jpeg)
        .unwrap();
        let mut tiff = b"MM\0\x2a\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01".to_vec();
        tiff.extend_from_slice(&orientation.to_be_bytes());
        tiff.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff);
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1];
        jpeg.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        jpeg.extend_from_slice(&app1);
        jpeg.extend_from_slice(&encoded[2..]);
        jpeg
    }

    #[test]
    fn a_jpeg_is_turned_by_its_exif_orientation() {
        use crate::compose::{DocumentSettings, PageSetup, Sides};
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
        let handle = document.load_image(halves_jpeg(6)).unwrap();
        let image = document.image(handle).unwrap();
        assert_eq!((image.width, image.height, image.orientation), (40, 20, 6));
        // Turned a quarter clockwise: 20 wide, 40 high, the left half on top.
        document
            .draw_image(handle, 0.0, 0.0, 20.0, 40.0, 6, 1.0)
            .unwrap();
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
        let pixmap = hayro::vello_cpu::Pixmap::from_png(Cursor::new(png)).unwrap();
        let at = |x: u32, y: u32| pixmap.data()[(y * u32::from(pixmap.width()) + x) as usize];
        let top = at(10, 5);
        let bottom = at(10, 35);
        assert!(top.r > 180 && top.b < 90, "red on top: {top:?}");
        assert!(bottom.b > 180 && bottom.r < 90, "blue below: {bottom:?}");
        let beside = at(30, 20);
        assert!(
            beside.r > 230 && beside.g > 230,
            "nothing past 20 wide: {beside:?}"
        );
    }

    #[test]
    fn each_orientation_lands_inside_its_box() {
        for orientation in 1..=8 {
            let (width, height) = (40.0, 30.0);
            let (stored_width, stored_height) = if orientation > 4 {
                (30.0, 40.0)
            } else {
                (40.0, 30.0)
            };
            let [a, b, c, d, e, f] = orientation_matrix(orientation, width, height);
            for (u, v) in [
                (0.0, 0.0),
                (stored_width, 0.0),
                (0.0, stored_height),
                (stored_width, stored_height),
            ] {
                let (x, y) = (a * u + c * v + e, b * u + d * v + f);
                assert!(
                    (-1e-3..=width + 1e-3).contains(&x) && (-1e-3..=height + 1e-3).contains(&y),
                    "orientation {orientation} puts ({u}, {v}) at ({x}, {y})"
                );
            }
        }
    }
}
