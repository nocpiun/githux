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
    if trimmed.starts_with(b"<?xml") || trimmed.starts_with(b"<!DOCTYPE") {
        let scan_len = trimmed.len().min(4096);
        let prefix = &trimmed[..scan_len];
        return contains_ascii_ci(prefix, b"<svg");
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
        return decode_svg(&bytes, limits);
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
    })
}

fn decode_svg(bytes: &[u8], limits: &ImageLimits) -> Result<DecodedImage, ImageFailure> {
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

    let mut options = svg_usvg_options();
    options.fontdb = svg_fontdb();

    let tree = usvg::Tree::from_xmltree(&doc, &options).map_err(map_usvg_error)?;
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

    let (render_w, render_h) = compute_svg_render_size(intrinsic_w, intrinsic_h, limits);
    let render_pixels = u64::from(render_w).saturating_mul(u64::from(render_h));
    if render_pixels > limits.max_pixels
        || render_pixels.saturating_mul(4) > limits.max_decoded_bytes as u64
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
    resvg::render(&tree, transform, &mut pixmap.as_mut());

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

    Ok(DecodedImage {
        format: ImageFormat::Png,
        width,
        height,
        bytes: png_bytes,
    })
}

fn count_svg_nesting_depth(bytes: &[u8]) -> usize {
    let mut depth: usize = 0;
    let mut max_depth: usize = 0;
    let mut in_tag = false;
    let mut is_closing = false;
    let mut is_declaration = false;
    let mut tag_name = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'<' {
            in_tag = true;
            is_closing = false;
            is_declaration = false;
            tag_name.clear();
            if i + 1 < bytes.len() {
                if bytes[i + 1] == b'/' {
                    is_closing = true;
                    i += 1;
                } else if bytes[i + 1] == b'!' || bytes[i + 1] == b'?' {
                    is_declaration = true;
                }
            }
        } else if b == b'>' && in_tag {
            in_tag = false;
            if !is_declaration {
                let self_closing = i > 0 && bytes[i - 1] == b'/';
                let tag_str = String::from_utf8_lossy(&tag_name).to_ascii_lowercase();
                let is_void = matches!(
                    tag_str.as_str(),
                    "circle"
                        | "ellipse"
                        | "line"
                        | "path"
                        | "polygon"
                        | "polyline"
                        | "rect"
                        | "stop"
                        | "image"
                        | "use"
                );
                if is_closing {
                    depth = depth.saturating_sub(1);
                } else if !self_closing && !is_void {
                    depth += 1;
                    max_depth = max_depth.max(depth);
                }
            }
        } else if in_tag && !b.is_ascii_whitespace() && b != b'/' && tag_name.len() < 32 {
            tag_name.push(b);
        }
        i += 1;
    }
    max_depth
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
                if name.len() >= 2 && name[..2].eq_ignore_ascii_case("on") {
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

fn svg_usvg_options() -> usvg::Options<'static> {
    let mut options = usvg::Options {
        // Never read files from disk or follow non-data hrefs inside `<image>`.
        resources_dir: None,
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|mime, data, opt| {
                if data.len() > MAX_SVG_EMBEDDED_DATA_BYTES {
                    return None;
                }
                // Allow embedded raster data URLs only; block nested SVG data URLs
                // so embedded SVG payloads cannot bypass top-level validation.
                match mime {
                    "image/jpg" | "image/jpeg" => Some(usvg::ImageKind::JPEG(data)),
                    "image/png" => Some(usvg::ImageKind::PNG(data)),
                    "image/gif" => Some(usvg::ImageKind::GIF(data)),
                    "image/webp" => Some(usvg::ImageKind::WEBP(data)),
                    _ => {
                        let _ = opt;
                        None
                    }
                }
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

fn compute_svg_render_size(intrinsic_w: f32, intrinsic_h: f32, limits: &ImageLimits) -> (u32, u32) {
    let max_dim = limits.max_dimension.max(1) as f32;
    let max_pixels = limits.max_pixels.max(1) as f32;
    let max_bytes_pixels = (limits.max_decoded_bytes / 4).max(1) as f32;
    let effective_pixel_cap = max_pixels.min(max_bytes_pixels);

    let max_scale_w = max_dim / intrinsic_w;
    let max_scale_h = max_dim / intrinsic_h;
    let max_scale_pixels = (effective_pixel_cap / (intrinsic_w * intrinsic_h)).sqrt();
    let scale = SVG_SUPERSAMPLE
        .min(max_scale_w)
        .min(max_scale_h)
        .min(max_scale_pixels)
        .max(1.0);

    let render_w = ((intrinsic_w * scale).round() as u32).clamp(1, limits.max_dimension.max(1));
    let render_h = ((intrinsic_h * scale).round() as u32).clamp(1, limits.max_dimension.max(1));
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
