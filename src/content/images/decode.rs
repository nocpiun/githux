//! Format sniffing and bounded decode for one picture.
//!
//! Raster payloads (`png`, `jpeg`, `webp`, `gif`) have their header read first
//! and their dimensions checked against the budget before any pixel buffer is
//! allocated; only then does the full decode run under the same limits, and the
//! validated original bytes are handed back.
//!
//! Static SVG payloads are parsed and rasterized through `resvg`/`usvg` into
//! PNG bytes under the same size/pixel/allocation limits, with bundled UI/code
//! fonts only (no filesystem or network access, no scripting, no external href
//! resolution, and no `<foreignObject>` or SMIL animations).

use std::io::Cursor;
use std::sync::{Arc, OnceLock};

use image::ImageEncoder;
use resvg::tiny_skia;
use resvg::usvg::{self, fontdb};

use super::{ImageFailure, ImageLimits};

/// Maximum XML element nodes accepted in a single SVG document before rejecting
/// as oversized/malformed, well below `usvg`'s internal 1,000,000 node ceiling.
const MAX_SVG_XML_NODES: usize = 50_000;
/// Maximum SVG nesting depth accepted before rejecting to bound recursion.
const MAX_SVG_XML_DEPTH: usize = 256;
/// Maximum size in bytes for any embedded raster/data URI `<image>` inside SVG.
const MAX_SVG_EMBEDDED_DATA_BYTES: usize = 2 * 1024 * 1024;
/// Maximum width/height rendered in device pixels for SVG rasterization before
/// applying `limits.max_dimension`.
const SVG_SUPERSAMPLE: f32 = 2.0;

const SVG_UI_FONT_REGULAR: &[u8] =
    include_bytes!("../../../assets/fonts/githux-ui/GithuxUI-Regular.ttf");
const SVG_UI_FONT_BOLD: &[u8] = include_bytes!("../../../assets/fonts/githux-ui/GithuxUI-Bold.ttf");
const SVG_CODE_FONT: &[u8] =
    include_bytes!("../../../assets/fonts/google-sans-code/GoogleSansCode.ttf");

fn svg_fontdb() -> Arc<fontdb::Database> {
    static FONTDB: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    FONTDB
        .get_or_init(|| {
            let mut db = fontdb::Database::new();
            // Only upright regular and bold faces: SVG badges/cards use those
            // weights, and `fontdb` picks the closest available face for 500/600.
            // Keeping the embedded set small matters because this module is also
            // compiled into the test and example assemblies.
            db.load_font_data(SVG_UI_FONT_REGULAR.to_vec());
            db.load_font_data(SVG_UI_FONT_BOLD.to_vec());
            db.load_font_data(SVG_CODE_FONT.to_vec());
            db.set_sans_serif_family("Githux UI");
            db.set_serif_family("Githux UI");
            db.set_cursive_family("Githux UI");
            db.set_fantasy_family("Githux UI");
            db.set_monospace_family("Google Sans Code");
            Arc::new(db)
        })
        .clone()
}

/// The picture formats this build accepts for downstream GPUI rendering.
/// Raster payloads pass through as-is; static SVG payloads are rasterized to
/// PNG bytes so UI consumers do not need a separate vector branch.
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
    /// Finite entry animation; `bytes` above is always the retained final PNG.
    pub animation: Option<DecodedAnimation>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct DecodedFrame {
    pub at_ms: u32,
    pub bytes: Vec<u8>,
    pub pixels: DecodedPixels,
}

/// Already decoded BGRA8 pixels, ready for a single-frame GPU image. Preparing
/// them in the provider avoids a fresh asynchronous PNG decode on every beat.
#[derive(Debug, PartialEq, Eq)]
pub struct DecodedPixels {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}
struct RenderedSvg {
    image: DecodedImage,
    pixels: DecodedPixels,
}
#[derive(Debug, PartialEq, Eq)]
pub struct DecodedAnimation {
    pub duration_ms: u32,
    pub frames: Vec<DecodedFrame>,
    pub final_pixels: DecodedPixels,
}

impl DecodedImage {
    /// Decoded memory this picture represents, used for the reader's budget.
    ///
    /// Raster payloads report their own pixel grid. A rasterized SVG keeps its
    /// logical/intrinsic `width`/`height` for layout, but its PNG bytes may be
    /// supersampled to a larger pixel grid; GPUI decodes that grid, so the
    /// budget must charge the raster size rather than the logical one.
    pub fn decoded_bytes(&self) -> usize {
        let (width, height) = match self.format {
            ImageFormat::Png => png_pixel_size(&self.bytes).unwrap_or((self.width, self.height)),
            ImageFormat::Jpeg | ImageFormat::Webp | ImageFormat::Gif => (self.width, self.height),
        };
        (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4)
            .saturating_mul(self.animation.as_ref().map_or(1, |a| a.frames.len() + 1))
    }
}

/// Reads the pixel grid out of a PNG IHDR, so a supersampled raster is charged
/// for the pixels a decoder actually allocates. Returns `None` for a payload
/// that is not a complete PNG header.
fn png_pixel_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    (width > 0 && height > 0).then_some((width, height))
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

/// Whether the bytes look like an SVG (or XML-prefixed SVG) document.
///
/// Used both to route SVG payloads into bounded static rasterization and to
/// distinguish malformed SVG from arbitrary non-image payloads.
pub fn is_svg_like(bytes: &[u8]) -> bool {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let trimmed = bytes.trim_ascii_start();
    if trimmed.starts_with(b"<svg") {
        return matches!(
            trimmed.get(4),
            None | Some(b'>' | b'/' | b' ' | b'\t' | b'\r' | b'\n')
        );
    }
    let tag = trimmed.strip_prefix(b"<").unwrap_or_default();
    let end = tag
        .iter()
        .position(|b| b.is_ascii_whitespace() || matches!(b, b'>' | b'/'))
        .unwrap_or(tag.len());
    if tag[..end].ends_with(b":svg") {
        return true;
    }
    if trimmed.starts_with(b"<?xml")
        || trimmed.starts_with(b"<!DOCTYPE")
        || trimmed.starts_with(b"<!--")
    {
        let scan_len = trimmed.len().min(4096);
        let prefix = &trimmed[..scan_len];
        return contains_ascii_ci(prefix, b"<svg")
            || prefix.split(|b| *b == b'<').any(|tag| {
                let end = tag
                    .iter()
                    .position(|b| b.is_ascii_whitespace() || matches!(b, b'>' | b'/'))
                    .unwrap_or(tag.len());
                tag[..end].ends_with(b":svg")
            });
    }
    false
}

fn contains_ascii_ci(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle))
}

/// Validates `bytes` under `limits` and returns the picture, or a typed reason.
///
/// For raster formats, the returned bytes are the same allocation that was
/// passed in after full validation. For static SVGs, the document is parsed
/// with external resource resolution disabled, checked against `limits` before
/// allocating a pixel buffer, rasterized to PNG bytes, and returned as
/// [`ImageFormat::Png`] with intrinsic/viewBox dimensions preserved.
pub fn decode_image(bytes: Vec<u8>, limits: &ImageLimits) -> Result<DecodedImage, ImageFailure> {
    decode_image_checked(bytes, limits, &|| Ok(()))
}

/// Cooperative checkpoints between finite SVG frames; the host supplies its
/// own cancellation/deadline check without coupling this module to networking.
pub fn decode_image_checked(
    bytes: Vec<u8>,
    limits: &ImageLimits,
    check: &dyn Fn() -> Result<(), ImageFailure>,
) -> Result<DecodedImage, ImageFailure> {
    check()?;
    if bytes.is_empty() {
        return Err(ImageFailure::Malformed);
    }
    if bytes.len() > limits.max_response_bytes {
        return Err(ImageFailure::TooLarge {
            limit: limits.max_response_bytes,
        });
    }
    if let Some(format) = detect_format(&bytes) {
        return decode_raster(bytes, format, limits);
    }
    if is_svg_like(&bytes) {
        return decode_svg(&bytes, limits, check);
    }
    Err(ImageFailure::Unsupported)
}

fn decode_raster(
    bytes: Vec<u8>,
    format: ImageFormat,
    limits: &ImageLimits,
) -> Result<DecodedImage, ImageFailure> {
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
        animation: None,
    })
}

fn decode_svg(
    bytes: &[u8],
    limits: &ImageLimits,
    check: &dyn Fn() -> Result<(), ImageFailure>,
) -> Result<DecodedImage, ImageFailure> {
    let text = validated_svg(bytes)?;
    let doc = parse_svg(text)?;
    let timeline = super::timeline::Timeline::new(&doc, check)?;
    let animated = timeline.duration_ms > 0.0;
    let final_xml = timeline.sample(text, timeline.duration_ms);
    let RenderedSvg {
        image: mut result,
        pixels: final_pixels,
    } = decode_svg_static(final_xml.as_bytes(), limits, animated)?;
    check()?;
    if animated && timeline.duration_ms <= super::timeline::MAX_DURATION_MS {
        let duration_ms = timeline.duration_ms.ceil() as u32;
        let count = (duration_ms.div_ceil(50) as usize).min(100);
        let frame_cost = result.decoded_bytes();
        // Budget the complete retained sequence, including the final frame.
        // Oversized timelines use the explicit static final fallback.
        if frame_cost.saturating_mul(count + 1) <= limits.max_decoded_bytes {
            let mut frames = Vec::new();
            let mut encoded = result.bytes.len();
            for ix in 0..count {
                check()?;
                let at_ms = (ix as u32 * duration_ms) / count as u32;
                let xml = timeline.sample(text, f64::from(at_ms));
                let mut frame_limits = limits.clone();
                // Previous raw frames coexist with this frame's scratch space.
                let retained = frame_cost.saturating_mul(ix + 1);
                frame_limits.max_decoded_bytes = limits.max_decoded_bytes.saturating_sub(retained);
                frame_limits.max_pixels = limits.max_pixels.saturating_sub((retained / 4) as u64);
                let frame = match decode_svg_static(xml.as_bytes(), &frame_limits, true) {
                    Ok(frame) => frame,
                    Err(ImageFailure::TooLarge { .. } | ImageFailure::TooManyPixels { .. }) => {
                        return Ok(result);
                    }
                    Err(error) => return Err(error),
                };
                if (frame.image.width, frame.image.height) != (result.width, result.height) {
                    return Err(ImageFailure::Malformed);
                }
                encoded = encoded.saturating_add(frame.image.bytes.len());
                if encoded > limits.max_response_bytes {
                    return Ok(result);
                }
                frames.push(DecodedFrame {
                    at_ms,
                    bytes: frame.image.bytes,
                    pixels: frame.pixels,
                });
            }
            result.animation = Some(DecodedAnimation {
                duration_ms,
                frames,
                final_pixels,
            });
        }
    }
    check()?;
    Ok(result)
}
fn validated_svg(bytes: &[u8]) -> Result<&str, ImageFailure> {
    if count_svg_nesting_depth(bytes) > MAX_SVG_XML_DEPTH {
        return Err(ImageFailure::Malformed);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| ImageFailure::Malformed)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let doc = parse_svg(text)?;
    if !svg_declares_intrinsic_size(&doc.root_element()) {
        return Err(ImageFailure::Malformed);
    }
    validate_svg_xml(&doc)?;
    Ok(text)
}
fn parse_svg(text: &str) -> Result<usvg::roxmltree::Document<'_>, ImageFailure> {
    usvg::roxmltree::Document::parse_with_options(
        text,
        usvg::roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: MAX_SVG_XML_NODES as u32,
            ..Default::default()
        },
    )
    .map_err(|err| {
        if matches!(err, usvg::roxmltree::Error::NodesLimitReached) {
            ImageFailure::TooLarge {
                limit: MAX_SVG_XML_NODES,
            }
        } else {
            ImageFailure::Malformed
        }
    })
}

fn decode_svg_static(
    bytes: &[u8],
    limits: &ImageLimits,
    native_scale: bool,
) -> Result<RenderedSvg, ImageFailure> {
    if count_svg_nesting_depth(bytes) > MAX_SVG_XML_DEPTH {
        return Err(ImageFailure::Malformed);
    }

    let text = std::str::from_utf8(bytes).map_err(|_| ImageFailure::Malformed)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);

    // Reject DTD/<!ENTITY> declarations outright instead of parsing them:
    // SVG payloads in Markdown never need XML entity expansion or external DTD
    // subsets, and disallowing them prevents XML entity expansion bombs.
    let xml_opt = usvg::roxmltree::ParsingOptions {
        allow_dtd: false,
        nodes_limit: MAX_SVG_XML_NODES as u32,
        ..Default::default()
    };
    let doc = usvg::roxmltree::Document::parse_with_options(text, xml_opt).map_err(|err| {
        if matches!(err, usvg::roxmltree::Error::NodesLimitReached) {
            ImageFailure::TooLarge {
                limit: MAX_SVG_XML_NODES,
            }
        } else {
            ImageFailure::Malformed
        }
    })?;

    if !svg_declares_intrinsic_size(&doc.root_element()) {
        return Err(ImageFailure::Malformed);
    }

    validate_svg_xml(&doc)?;

    let resources = Arc::new(std::sync::Mutex::new((
        0usize,
        0u64,
        doc.descendants().count(),
    )));
    let mut options = svg_options_with_budget(limits, 0, resources.clone());
    options.fontdb = svg_fontdb();

    let tree = usvg::Tree::from_xmltree(&doc, &options).map_err(map_usvg_error)?;
    if !tree.patterns().is_empty() {
        return Err(ImageFailure::Unsupported);
    }
    let svg_size = tree.size();
    let intrinsic_w = svg_size.width();
    let intrinsic_h = svg_size.height();
    if !intrinsic_w.is_finite()
        || !intrinsic_h.is_finite()
        || intrinsic_w <= 0.0
        || intrinsic_h <= 0.0
    {
        return Err(ImageFailure::Malformed);
    }

    let width_f = intrinsic_w.round().max(1.0);
    let height_f = intrinsic_h.round().max(1.0);
    if width_f > limits.max_dimension as f32 || height_f > limits.max_dimension as f32 {
        return Err(ImageFailure::TooManyPixels {
            width: width_f.min(u32::MAX as f32) as u32,
            height: height_f.min(u32::MAX as f32) as u32,
            limit: limits.max_pixels,
        });
    }
    let width = width_f as u32;
    let height = height_f as u32;
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

    let (mut render_w, mut render_h) = if native_scale {
        (width, height)
    } else {
        compute_svg_render_size(intrinsic_w, intrinsic_h, limits)
    };
    let embedded_pixels = resources.lock().map_err(|_| ImageFailure::Malformed)?.1;
    let scratch_cap = limits.max_pixels.min((limits.max_decoded_bytes / 4) as u64);
    if !native_scale
        && super::workspace::pixels(&tree, render_w, render_h, embedded_pixels) > scratch_cap
    {
        (render_w, render_h) = (width, height);
    }
    if super::workspace::pixels(&tree, render_w, render_h, embedded_pixels) > scratch_cap {
        return Err(ImageFailure::TooLarge {
            limit: limits.max_decoded_bytes,
        });
    }
    let render_pixels = u64::from(render_w).saturating_mul(u64::from(render_h));
    if render_pixels > limits.max_pixels
        || render_pixels.saturating_mul(4) > limits.max_decoded_bytes as u64
    {
        return Err(ImageFailure::TooLarge {
            limit: limits.max_decoded_bytes,
        });
    }

    if render_pixels.saturating_add(embedded_pixels) > limits.max_pixels
        || render_pixels
            .saturating_add(embedded_pixels)
            .saturating_mul(4)
            > limits.max_decoded_bytes as u64
    {
        return Err(ImageFailure::TooLarge {
            limit: limits.max_decoded_bytes,
        });
    }

    let mut pixmap = tiny_skia::Pixmap::new(render_w, render_h).ok_or(ImageFailure::TooLarge {
        limit: limits.max_decoded_bytes,
    })?;
    let transform = tiny_skia::Transform::from_scale(
        render_w as f32 / intrinsic_w,
        render_h as f32 / intrinsic_h,
    );
    let root_opacity = super::timeline::root_opacity(&doc);
    if root_opacity > 0.0 {
        resvg::render(&tree, transform, &mut pixmap.as_mut());
    }
    if root_opacity < 1.0 {
        for channel in pixmap.data_mut() {
            *channel = (f32::from(*channel) * root_opacity).round() as u8;
        }
    }

    // Demultiply tiny-skia's premultiplied RGBA into straight RGBA for PNG.
    let mut rgba = pixmap.take();
    let (pixels, _remainder) = rgba.as_chunks_mut::<4>();
    for pixel in pixels {
        let alpha = u32::from(pixel[3]);
        if alpha > 0 && alpha < 255 {
            for channel in &mut pixel[..3] {
                let value = (u32::from(*channel) * 255 + alpha / 2) / alpha;
                *channel = value.min(255) as u8;
            }
        }
    }

    let mut png_bytes = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut png_bytes);
    encoder
        .write_image(&rgba, render_w, render_h, image::ExtendedColorType::Rgba8)
        .map_err(|_| ImageFailure::Malformed)?;

    if png_bytes.len() > limits.max_response_bytes {
        return Err(ImageFailure::TooLarge {
            limit: limits.max_response_bytes,
        });
    }

    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Ok(RenderedSvg {
        image: DecodedImage {
            format: ImageFormat::Png,
            width,
            height,
            bytes: png_bytes,
            animation: None,
        },
        pixels: DecodedPixels {
            width: render_w,
            height: render_h,
            bgra: rgba,
        },
    })
}

fn count_svg_nesting_depth(bytes: &[u8]) -> usize {
    let len = bytes.len();
    let mut depth: usize = 0;
    let mut max_depth: usize = 0;
    let mut i = 0;

    while i < len {
        while i < len && bytes[i] != b'<' {
            i += 1;
        }
        if i >= len {
            break;
        }

        // Comments, CDATA and processing instructions can contain `<`, `>` and
        // quotes that are not markup delimiters; skip them as a unit.
        if bytes[i..].starts_with(b"<!--") {
            i = find_after(bytes, i + 4, b"-->").unwrap_or(len);
            continue;
        }
        if bytes[i..].starts_with(b"<![CDATA[") {
            i = find_after(bytes, i + 9, b"]]>").unwrap_or(len);
            continue;
        }
        if bytes[i..].starts_with(b"<?") {
            i = find_after(bytes, i + 2, b"?>").unwrap_or(len);
            continue;
        }
        if bytes[i..].starts_with(b"<!") {
            // `<!DOCTYPE ...>` may carry an internal subset with nested `<...>`
            // declarations, quoted strings and brackets. DTDs are rejected by
            // the parser anyway; the scanner only has to skip them safely.
            let mut j = i + 2;
            let mut bracket_depth = 0usize;
            let mut quote: Option<u8> = None;
            while j < len {
                let byte = bytes[j];
                match quote {
                    Some(active) if byte == active => quote = None,
                    Some(_) => {}
                    None => match byte {
                        b'"' | b'\'' => quote = Some(byte),
                        b'[' => bracket_depth += 1,
                        b']' => bracket_depth = bracket_depth.saturating_sub(1),
                        b'>' if bracket_depth == 0 => {
                            j += 1;
                            break;
                        }
                        _ => {}
                    },
                }
                j += 1;
            }
            i = j;
            continue;
        }

        if bytes.get(i + 1) == Some(&b'/') {
            depth = depth.saturating_sub(1);
            i = skip_tag(bytes, i + 2);
            continue;
        }

        let (next, self_closing) = scan_start_tag(bytes, i + 1);
        if !self_closing {
            depth += 1;
            max_depth = max_depth.max(depth);
        }
        i = next;
    }

    max_depth
}

/// Returns the byte index just past `needle` at or after `from`.
fn find_after(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if from >= bytes.len() || needle.is_empty() {
        return None;
    }
    bytes[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| from + offset + needle.len())
}

/// Skips the remainder of an end tag. Quoting cannot appear in a well-formed
/// end tag, so the first `>` ends it.
fn skip_tag(bytes: &[u8], from: usize) -> usize {
    bytes[from..]
        .iter()
        .position(|byte| *byte == b'>')
        .map(|offset| from + offset + 1)
        .unwrap_or(bytes.len())
}

/// Scans a start tag starting at its name, honouring quoted attribute values.
/// Returns the index just past the closing `>` and whether the tag was written
/// as `<.../>`. A `/>` inside a quoted value is not a self-closing marker.
fn scan_start_tag(bytes: &[u8], from: usize) -> (usize, bool) {
    let len = bytes.len();
    let mut i = from;
    let mut quote: Option<u8> = None;
    let mut last_significant = 0u8;
    while i < len {
        let byte = bytes[i];
        match quote {
            Some(active) if byte == active => quote = None,
            Some(_) => {}
            None => match byte {
                b'"' | b'\'' => quote = Some(byte),
                b'>' => {
                    return (i + 1, last_significant == b'/');
                }
                b' ' | b'\t' | b'\r' | b'\n' => {}
                _ => last_significant = byte,
            },
        }
        i += 1;
    }
    (len, false)
}

/// Whether the root `<svg>` declares an intrinsic size we can preserve:
/// either a usable `viewBox` or explicit absolute `width`/`height`. A document
/// with only percentages or no size at all is rejected instead of guessing a
/// default canvas, so the caller never receives arbitrary intrinsic dimensions.
fn svg_declares_intrinsic_size(root: &usvg::roxmltree::Node<'_, '_>) -> bool {
    let usable_length = |value: &str| {
        let trimmed = value.trim();
        if trimmed.is_empty()
            || trimmed.ends_with('%')
            || trimmed.ends_with("em")
            || trimmed.ends_with("ex")
        {
            return false;
        }
        let number = trimmed.strip_suffix("px").unwrap_or(trimmed).trim();
        number
            .parse::<f32>()
            .is_ok_and(|n| n.is_finite() && n > 0.0)
    };

    if let Some(view_box) = root.attribute("viewBox") {
        let dims: Vec<f32> = view_box
            .split(['\u{20}', ',', '\t', '\r', '\n'])
            .filter(|part| !part.is_empty())
            .filter_map(|part| part.parse::<f32>().ok())
            .collect();
        if dims.len() == 4 && dims[2] > 0.0 && dims[3] > 0.0 {
            return true;
        }
    }

    matches!((root.attribute("width"), root.attribute("height")), (Some(w), Some(h)) if usable_length(w) && usable_length(h))
}

fn validate_svg_xml(doc: &usvg::roxmltree::Document<'_>) -> Result<(), ImageFailure> {
    let root = doc.root_element();
    if root.tag_name().name() != "svg" {
        return Err(ImageFailure::Malformed);
    }

    // Iterative DFS to enforce nesting depth and reject unsupported active or
    // foreign content explicitly rather than silently dropping interactive HTML
    // or scripted SVG payloads.
    let mut stack = vec![(root, 1usize)];
    while let Some((node, depth)) = stack.pop() {
        if depth > MAX_SVG_XML_DEPTH {
            return Err(ImageFailure::Malformed);
        }
        if node.is_element() {
            let local = node.tag_name().name();
            if local.eq_ignore_ascii_case("foreignObject")
                || local.eq_ignore_ascii_case("script")
                || local.eq_ignore_ascii_case("animate")
                || local.eq_ignore_ascii_case("animateTransform")
                || local.eq_ignore_ascii_case("animateMotion")
                || local.eq_ignore_ascii_case("set")
            {
                return Err(ImageFailure::Unsupported);
            }
            for attr in node.attributes() {
                let name = attr.name();
                // Boundary-safe prefix check: XML attribute names may contain
                // multi-byte characters, so slicing `name[..2]` could panic on
                // a valid document instead of simply ignoring the attribute.
                let name_bytes = name.as_bytes();
                if name_bytes.len() >= 2
                    && name_bytes[0].eq_ignore_ascii_case(&b'o')
                    && name_bytes[1].eq_ignore_ascii_case(&b'n')
                {
                    return Err(ImageFailure::Unsupported);
                }
                if local.eq_ignore_ascii_case("style")
                    && attr.value().to_ascii_lowercase().contains("@import")
                {
                    return Err(ImageFailure::Unsupported);
                }
            }
            if local.eq_ignore_ascii_case("style")
                && node
                    .text()
                    .is_some_and(|css| css.to_ascii_lowercase().contains("@import"))
            {
                return Err(ImageFailure::Unsupported);
            }
        }
        for child in node.children() {
            if child.is_element() {
                stack.push((child, depth + 1));
            }
        }
    }
    Ok(())
}

fn svg_options_with_budget(
    limits: &ImageLimits,
    depth: usize,
    budget: Arc<std::sync::Mutex<(usize, u64, usize)>>,
) -> usvg::Options<'static> {
    let embedded_limits = limits.clone();
    let mut options = usvg::Options {
        // Never read files from disk or follow non-data hrefs inside `<image>`.
        resources_dir: None,
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(move |mime, data, _opt| {
                if data.len() > MAX_SVG_EMBEDDED_DATA_BYTES || depth >= 4 {
                    return None;
                }
                {
                    let mut used = budget.lock().ok()?;
                    used.0 = used.0.checked_add(data.len())?;
                    if used.0 > MAX_SVG_EMBEDDED_DATA_BYTES {
                        return None;
                    }
                }
                if mime == "image/svg+xml" {
                    let text = validated_svg(&data).ok()?;
                    let doc = parse_svg(text).ok()?;
                    {
                        let mut used = budget.lock().ok()?;
                        used.2 = used.2.checked_add(doc.descendants().count())?;
                        if used.2 > MAX_SVG_XML_NODES {
                            return None;
                        }
                    }
                    let timeline = super::timeline::Timeline::new(&doc, &|| Ok(())).ok()?;
                    let xml = timeline.sample(text, timeline.duration_ms);
                    let mut nested =
                        svg_options_with_budget(&embedded_limits, depth + 1, budget.clone());
                    nested.fontdb = svg_fontdb();
                    let normalized = parse_svg(&xml).ok()?;
                    if super::timeline::root_opacity(&normalized) == 0.0 {
                        return None;
                    }
                    let tree = usvg::Tree::from_xmltree(&normalized, &nested).ok()?;
                    if !tree.patterns().is_empty() {
                        return None;
                    }
                    let size = tree.size();
                    let pixels =
                        (size.width().ceil() as u64).checked_mul(size.height().ceil() as u64)?;
                    if size.width() > embedded_limits.max_dimension as f32
                        || size.height() > embedded_limits.max_dimension as f32
                    {
                        return None;
                    }
                    let mut used = budget.lock().ok()?;
                    let next = used.1.checked_add(pixels)?;
                    if next.saturating_mul(2) > embedded_limits.max_pixels
                        || next.saturating_mul(8) > embedded_limits.max_decoded_bytes as u64
                    {
                        return None;
                    }
                    used.1 = next;
                    return Some(usvg::ImageKind::SVG(tree));
                }
                // `usvg` only wraps these bytes; `resvg` decodes them later with
                // no limits of its own. Validate the header against the same
                // dimension/pixel/allocation budget before agreeing to embed it,
                // so a highly compressed giant raster cannot allocate far past
                // the document budget during rendering.
                let format = match mime {
                    "image/jpg" | "image/jpeg" => image::ImageFormat::Jpeg,
                    "image/png" => image::ImageFormat::Png,
                    "image/gif" => image::ImageFormat::Gif,
                    "image/webp" => image::ImageFormat::WebP,
                    // Nested SVG data URLs are blocked so they cannot bypass the
                    // top-level XML/feature validation.
                    _ => return None,
                };
                let (width, height) = embedded_raster_size(data.as_slice(), format)?;
                if width > embedded_limits.max_dimension || height > embedded_limits.max_dimension {
                    return None;
                }
                let pixels = u64::from(width).saturating_mul(u64::from(height));
                if pixels > embedded_limits.max_pixels
                    || pixels.saturating_mul(4) > embedded_limits.max_decoded_bytes as u64
                {
                    return None;
                }
                let canonical_gif = if format == image::ImageFormat::Gif {
                    // resvg's GIF decoder allocates the first frame's own grid,
                    // which can exceed the logical-screen header. Decode under
                    // image::Limits and flatten that frame to a bounded PNG.
                    let mut reader =
                        image::ImageReader::with_format(Cursor::new(data.as_slice()), format);
                    let mut limits = image::Limits::default();
                    limits.max_image_width = Some(embedded_limits.max_dimension);
                    limits.max_image_height = Some(embedded_limits.max_dimension);
                    limits.max_alloc = Some(embedded_limits.max_decoded_bytes as u64);
                    reader.limits(limits);
                    let rgba = reader.decode().ok()?.into_rgba8();
                    let mut png = Vec::new();
                    image::codecs::png::PngEncoder::new(&mut png)
                        .write_image(
                            rgba.as_raw(),
                            width,
                            height,
                            image::ExtendedColorType::Rgba8,
                        )
                        .ok()?;
                    let mut used = budget.lock().ok()?;
                    used.0 = used.0.checked_add(png.len())?;
                    if used.0 > MAX_SVG_EMBEDDED_DATA_BYTES {
                        return None;
                    }
                    Some(Arc::new(png))
                } else {
                    None
                };
                {
                    let mut used = budget.lock().ok()?;
                    let next = used.1.checked_add(pixels)?;
                    if next.saturating_mul(2) > embedded_limits.max_pixels
                        || next.saturating_mul(8) > embedded_limits.max_decoded_bytes as u64
                    {
                        return None;
                    }
                    used.1 = next;
                }
                Some(match format {
                    image::ImageFormat::Jpeg => usvg::ImageKind::JPEG(data),
                    image::ImageFormat::Png => usvg::ImageKind::PNG(data),
                    image::ImageFormat::Gif => usvg::ImageKind::PNG(canonical_gif?),
                    image::ImageFormat::WebP => usvg::ImageKind::WEBP(data),
                    _ => return None,
                })
            }),
            resolve_string: Box::new(|_, _| None),
        },
        ..usvg::Options::default()
    };
    options.font_resolver = usvg::FontResolver {
        select_font: Box::new(|font, db| {
            if let Some(id) = (usvg::FontResolver::default_font_selector())(font, db) {
                return Some(id);
            }
            // Fallback to bundled fonts when the SVG requests system/web font
            // families such as Verdana, Segoe UI, system-ui, Arial, etc.
            let has_mono = font.families().iter().any(|family| {
                matches!(family, usvg::FontFamily::Monospace)
                    || matches!(family, usvg::FontFamily::Named(name) if name.to_ascii_lowercase().contains("mono") || name.to_ascii_lowercase().contains("code") || name.to_ascii_lowercase().contains("consolas") || name.to_ascii_lowercase().contains("courier"))
            });
            let target_family = if has_mono {
                fontdb::Family::Monospace
            } else {
                fontdb::Family::SansSerif
            };
            let query = fontdb::Query {
                families: &[target_family],
                weight: fontdb::Weight(font.weight()),
                stretch: fontdb::Stretch::Normal,
                style: match font.style() {
                    usvg::FontStyle::Normal => fontdb::Style::Normal,
                    usvg::FontStyle::Italic => fontdb::Style::Italic,
                    usvg::FontStyle::Oblique => fontdb::Style::Oblique,
                },
            };
            db.query(&query)
                .or_else(|| {
                    db.query(&fontdb::Query {
                        families: &[target_family],
                        ..Default::default()
                    })
                })
                .or_else(|| db.faces().next().map(|face| face.id))
        }),
        select_fallback: Box::new(|ch, used_fonts, db| {
            if let Some(id) = (usvg::FontResolver::default_fallback_selector())(ch, used_fonts, db)
            {
                return Some(id);
            }
            // Broaden fallback across bundled faces even when weight/style
            // differ (e.g. mono face vs sans-serif UI face).
            for face in db.faces() {
                if !used_fonts.contains(&face.id) {
                    return Some(face.id);
                }
            }
            None
        }),
    };
    options
}

/// Header-only size of an embedded raster data URI. `into_dimensions` reads
/// just enough of the payload to report the grid, so the budget check happens
/// before `resvg` allocates the decoded buffer.
fn embedded_raster_size(data: &[u8], format: image::ImageFormat) -> Option<(u32, u32)> {
    let reader = image::ImageReader::with_format(Cursor::new(data), format);
    let (width, height) = reader.into_dimensions().ok()?;
    (width > 0 && height > 0).then_some((width, height))
}

fn compute_svg_render_size(intrinsic_w: f32, intrinsic_h: f32, limits: &ImageLimits) -> (u32, u32) {
    let max_dim = limits.max_dimension.max(1) as f32;
    let max_pixels = limits.max_pixels.max(1) as f32;
    let max_bytes_pixels = (limits.max_decoded_bytes / 4).max(1) as f32;
    let effective_pixel_cap = (max_pixels.min(max_bytes_pixels) as f64).max(1.0);

    let max_scale_w = max_dim / intrinsic_w;
    let max_scale_h = max_dim / intrinsic_h;
    let max_scale_pixels =
        (effective_pixel_cap / (intrinsic_w as f64 * intrinsic_h as f64)).sqrt() as f32;
    let scale = SVG_SUPERSAMPLE
        .min(max_scale_w)
        .min(max_scale_h)
        .min(max_scale_pixels)
        .max(1.0);

    let max_side = limits.max_dimension.max(1);
    let mut render_w = ((intrinsic_w * scale).floor() as u32).clamp(1, max_side);
    let mut render_h = ((intrinsic_h * scale).floor() as u32).clamp(1, max_side);

    // Rounding down each side is not enough: `sqrt(cap / (w*h))` can leave the
    // truncated product one row/column over the cap (e.g. 2001x2000 at 2x).
    // Shrink proportionally, then finish with a bounded decrement so optional
    // supersampling never rejects an SVG whose intrinsic size fits the budget.
    let product = u64::from(render_w) * u64::from(render_h);
    if product as f64 > effective_pixel_cap {
        let ratio = (effective_pixel_cap / product as f64).sqrt();
        render_w = ((render_w as f64 * ratio).floor() as u32).clamp(1, max_side);
        render_h = ((render_h as f64 * ratio).floor() as u32).clamp(1, max_side);
    }
    let mut guard = 0u32;
    while u64::from(render_w) * u64::from(render_h) > effective_pixel_cap as u64 && guard < 8192 {
        if render_w >= render_h && render_w > 1 {
            render_w -= 1;
        } else if render_h > 1 {
            render_h -= 1;
        } else {
            break;
        }
        guard += 1;
    }
    (render_w, render_h)
}

fn map_usvg_error(error: usvg::Error) -> ImageFailure {
    match error {
        usvg::Error::ElementsLimitReached => ImageFailure::TooLarge {
            limit: MAX_SVG_XML_NODES,
        },
        usvg::Error::NotAnUtf8Str
        | usvg::Error::MalformedGZip
        | usvg::Error::InvalidSize
        | usvg::Error::ParsingFailed(_) => ImageFailure::Malformed,
    }
}
