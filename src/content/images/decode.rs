//! Format sniffing and bounded decode for one picture.
//!
//! The header is read first and its dimensions are checked against the budget
//! before any pixel buffer is allocated. Only then does the full decode run,
//! under the same limits, to prove the payload is really a picture. The decoded
//! pixels are dropped; the validated original bytes are handed back so the
//! renderer can use them without a second copy of the raw pixels.

use std::io::Cursor;

use super::{ImageFailure, ImageLimits};

/// The picture formats this build accepts. Matches the enabled `image`
/// features (`png`, `jpeg`, `webp`, `gif`); SVG is deliberately absent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Webp,
    Gif,
}

impl ImageFormat {
    /// The `image` crate's format for this format.
    fn image_format(self) -> image::ImageFormat {
        match self {
            Self::Png => image::ImageFormat::Png,
            Self::Jpeg => image::ImageFormat::Jpeg,
            Self::Webp => image::ImageFormat::WebP,
            Self::Gif => image::ImageFormat::Gif,
        }
    }
}

/// A validated picture: its format, its intrinsic size and the original bytes.
#[derive(Debug, PartialEq, Eq)]
pub struct DecodedImage {
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

impl DecodedImage {
    /// Decoded memory this picture represents, used for the reader's budget.
    pub fn decoded_bytes(&self) -> usize {
        (self.width as usize)
            .saturating_mul(self.height as usize)
            .saturating_mul(4)
    }
}

/// Sniffs the format from the leading bytes rather than a header or extension.
pub fn detect_format(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.len() >= 8 && bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageFormat::Png)
    } else if bytes.len() >= 3 && bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(ImageFormat::Jpeg)
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(ImageFormat::Webp)
    } else if bytes.len() >= 6 && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        Some(ImageFormat::Gif)
    } else {
        None
    }
}

/// Whether the bytes look like SVG or XML, so the placeholder can say the
/// format is unsupported instead of calling a real vector image "malformed".
pub fn is_svg_like(bytes: &[u8]) -> bool {
    let head = bytes
        .iter()
        .copied()
        .take(256)
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect::<Vec<u8>>();
    head.starts_with(b"<svg") || head.starts_with(b"<?xml") || head.starts_with(b"<!DOCTYPE svg")
}

/// Validates `bytes` under `limits` and returns the picture, or a typed reason.
///
/// The returned bytes are the same allocation that was passed in; the decoded
/// pixel buffer is dropped after it proved the payload is a real picture.
pub fn decode_image(bytes: Vec<u8>, limits: &ImageLimits) -> Result<DecodedImage, ImageFailure> {
    if bytes.is_empty() {
        return Err(ImageFailure::Malformed);
    }
    if bytes.len() > limits.max_response_bytes {
        return Err(ImageFailure::TooLarge {
            limit: limits.max_response_bytes,
        });
    }
    let Some(format) = detect_format(&bytes) else {
        return Err(ImageFailure::Unsupported);
    };
    if bytes.len() as u64 > limits.max_decoded_bytes as u64 {
        return Err(ImageFailure::TooLarge {
            limit: limits.max_decoded_bytes,
        });
    }

    let mut decode_limits = image::Limits::default();
    decode_limits.max_image_width = Some(limits.max_dimension);
    decode_limits.max_image_height = Some(limits.max_dimension);
    decode_limits.max_alloc = Some(limits.max_decoded_bytes as u64);

    // Header first: `into_dimensions` reads only what is needed to know the
    // size. The dimension limits are applied by this module so an oversized
    // picture is reported as oversized rather than as malformed data.
    let mut header_limits = image::Limits::default();
    header_limits.max_image_width = None;
    header_limits.max_image_height = None;
    header_limits.max_alloc = Some(limits.max_decoded_bytes as u64);
    let (width, height) = {
        let mut reader =
            image::ImageReader::with_format(Cursor::new(bytes.as_slice()), format.image_format());
        reader.limits(header_limits);
        reader
            .into_dimensions()
            .map_err(|_| ImageFailure::Malformed)?
    };
    if width > limits.max_dimension || height > limits.max_dimension {
        return Err(ImageFailure::TooManyPixels {
            width,
            height,
            limit: limits.max_pixels,
        });
    }
    let pixels = u64::from(width).saturating_mul(u64::from(height));
    if pixels > limits.max_pixels {
        return Err(ImageFailure::TooManyPixels {
            width,
            height,
            limit: limits.max_pixels,
        });
    }
    if pixels.saturating_mul(4) > limits.max_decoded_bytes as u64 {
        return Err(ImageFailure::TooLarge {
            limit: limits.max_decoded_bytes,
        });
    }

    // Full decode under the same limits. This is the step that rejects a
    // truncated or malformed payload whose header merely looked valid; the
    // decoded image is dropped immediately after validation.
    let mut reader =
        image::ImageReader::with_format(Cursor::new(bytes.as_slice()), format.image_format());
    reader.limits(decode_limits);
    let decoded = reader.decode().map_err(|_| ImageFailure::Malformed)?;
    if decoded.width() != width || decoded.height() != height {
        return Err(ImageFailure::Malformed);
    }
    drop(decoded);

    Ok(DecodedImage {
        format,
        width,
        height,
        bytes,
    })
}
