//! In-memory, bounded data-font support for SVG font-face declarations.
//! No URL/file resolver or system-font discovery is used.
use super::ImageFailure;
use base64::Engine;
use resvg::usvg::{fontdb, roxmltree::Document};
use std::{
    collections::BTreeSet,
    io::Read,
    sync::{Arc, Mutex},
};

pub(super) const MAX_FONT_BYTES: usize = 4 * 1024 * 1024;
const MAX_ENCODED_FONT: usize = 1024 * 1024;
fn malformed() -> ImageFailure {
    ImageFailure::Malformed
}
fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?))
}
fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}
fn checksum(bytes: &[u8]) -> u32 {
    bytes.chunks(4).fold(0u32, |sum, chunk| {
        let mut word = [0; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

fn unpack_woff(data: &[u8], limit: usize) -> Result<Vec<u8>, ImageFailure> {
    if data.get(..4) != Some(b"wOFF") {
        // Font collections carry an independent face count which fontdb walks;
        // the byte cap alone does not bound that count. This loader accepts
        // single-face SFNT/WOFF1 data only.
        if data.starts_with(b"ttcf") {
            return Err(ImageFailure::Unsupported);
        }
        return if data.len() <= limit {
            Ok(data.to_vec())
        } else {
            Err(ImageFailure::TooLarge { limit })
        };
    }
    let count = usize::from(u16_at(data, 12).ok_or_else(malformed)?);
    let declared = u32_at(data, 16).ok_or_else(malformed)? as usize;
    if count == 0 || count > 256 || declared > limit {
        return Err(ImageFailure::TooLarge { limit });
    }
    if u32_at(data, 8).map(|n| n as usize) != Some(data.len())
        || data.len() < 44 + count * 20
        || declared < 12 + count * 16
    {
        return Err(malformed());
    }
    let mut output = vec![0u8; 12 + count * 16];
    output[..4].copy_from_slice(&data[4..8]);
    output[4..6].copy_from_slice(&(count as u16).to_be_bytes());
    let power = 1usize << (usize::BITS - 1 - count.leading_zeros());
    output[6..8].copy_from_slice(&((power * 16) as u16).to_be_bytes());
    output[8..10].copy_from_slice(&(power.trailing_zeros() as u16).to_be_bytes());
    output[10..12].copy_from_slice(&((count * 16 - power * 16) as u16).to_be_bytes());
    let mut tags = BTreeSet::new();
    let mut head = None;
    for index in 0..count {
        let entry = 44 + index * 20;
        let tag = data.get(entry..entry + 4).ok_or_else(malformed)?;
        if !tags.insert(tag.to_vec()) {
            return Err(malformed());
        }
        let offset = u32_at(data, entry + 4).ok_or_else(malformed)? as usize;
        let packed = u32_at(data, entry + 8).ok_or_else(malformed)? as usize;
        let length = u32_at(data, entry + 12).ok_or_else(malformed)? as usize;
        let aligned = (length.checked_add(3).ok_or_else(malformed)? / 4) * 4;
        if packed > length
            || offset < 44 + count * 20
            || output
                .len()
                .checked_add(aligned)
                .is_none_or(|n| n > declared || n > limit)
        {
            return Err(ImageFailure::TooLarge { limit });
        }
        let input = data
            .get(offset..offset.checked_add(packed).ok_or_else(malformed)?)
            .ok_or_else(malformed)?;
        let start = output.len();
        if packed == length {
            output.extend_from_slice(input);
        } else {
            let mut table = Vec::new();
            flate2::read::ZlibDecoder::new(input)
                .take(length as u64 + 1)
                .read_to_end(&mut table)
                .map_err(|_| malformed())?;
            if table.len() != length {
                return Err(malformed());
            }
            output.extend_from_slice(&table);
        }
        output.resize(start + aligned, 0);
        let directory = 12 + index * 16;
        output[directory..directory + 4].copy_from_slice(tag);
        output[directory + 4..directory + 8].copy_from_slice(&data[entry + 16..entry + 20]);
        output[directory + 8..directory + 12].copy_from_slice(&(start as u32).to_be_bytes());
        output[directory + 12..directory + 16].copy_from_slice(&(length as u32).to_be_bytes());
        if tag == b"head" {
            if length < 12 {
                return Err(malformed());
            }
            head = Some(start);
            output[start + 8..start + 12].fill(0);
        }
        if checksum(&output[start..start + length])
            != u32_at(data, entry + 16).ok_or_else(malformed)?
        {
            return Err(malformed());
        }
    }
    if output.len() != declared {
        return Err(malformed());
    }
    if let Some(head) = head {
        let adjustment = 0xB1B0_AFBAu32.wrapping_sub(checksum(&output));
        output[head + 8..head + 12].copy_from_slice(&adjustment.to_be_bytes());
    }
    Ok(output)
}

fn validate_name_work(font: &[u8]) -> Result<(), ImageFailure> {
    // Validate the reconstructed flavor too: WOFF's untrusted flavor field
    // must not smuggle a collection past the raw input collection check.
    if !matches!(font.get(..4), Some(b"\0\x01\0\0" | b"OTTO" | b"true")) {
        return Err(ImageFailure::Unsupported);
    }
    let count = usize::from(u16_at(font, 4).ok_or_else(malformed)?);
    if count > 256 {
        return Err(ImageFailure::TooLarge { limit: 256 });
    }
    for index in 0..count {
        let at = 12 + index * 16;
        if font.get(at..at + 4) != Some(b"name") {
            continue;
        }
        let offset = u32_at(font, at + 8).ok_or_else(malformed)? as usize;
        let length = u32_at(font, at + 12).ok_or_else(malformed)? as usize;
        let table = font
            .get(offset..offset.checked_add(length).ok_or_else(malformed)?)
            .ok_or_else(malformed)?;
        let records = usize::from(u16_at(table, 2).ok_or_else(malformed)?);
        if records > 256 {
            return Err(ImageFailure::TooLarge { limit: 256 });
        }
        let mut work = 0usize;
        for entry in 0..records {
            work = work
                .checked_add(usize::from(
                    u16_at(table, 6 + entry * 12 + 8).ok_or_else(malformed)?,
                ))
                .ok_or_else(malformed)?;
            if work > 64 * 1024 {
                return Err(ImageFailure::TooLarge { limit: 64 * 1024 });
            }
        }
    }
    Ok(())
}

pub(super) fn with_embedded(
    doc: &Document<'_>,
    base: Arc<fontdb::Database>,
    budget: &Arc<Mutex<(usize, u64, usize, usize)>>,
) -> Result<Arc<fontdb::Database>, ImageFailure> {
    let faces = super::timeline::font_faces(doc);
    if faces.is_empty() {
        return Ok(base);
    }
    let mut db = (*base).clone();
    for (family, source) in faces {
        let Some((header, encoded)) = source.strip_prefix("data:").and_then(|s| s.split_once(','))
        else {
            continue;
        };
        if !header.ends_with(";base64") || encoded.len() > MAX_ENCODED_FONT * 4 / 3 + 4 {
            continue;
        }
        let data = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| malformed())?;
        if data.len() > MAX_ENCODED_FONT {
            return Err(ImageFailure::TooLarge {
                limit: MAX_ENCODED_FONT,
            });
        }
        // Unknown data-font formats retain the existing fallback behavior.
        // Only supported single-face fonts or explicitly rejected collections
        // enter the bounded SFNT/WOFF parser below.
        if !matches!(
            data.get(..4),
            Some(b"wOFF" | b"OTTO" | b"true" | b"ttcf" | b"\0\x01\0\0")
        ) {
            continue;
        }
        let mut used = budget.lock().map_err(|_| malformed())?;
        let font = unpack_woff(&data, MAX_FONT_BYTES.saturating_sub(used.3))?;
        validate_name_work(&font)?;
        used.3 = used.3.checked_add(font.len()).ok_or_else(malformed)?;
        drop(used);
        let ids = db.load_font_source(fontdb::Source::Binary(Arc::new(font)));
        for id in ids {
            if let Some(mut face) = db.face(id).cloned()
                && let Some((_, language)) = face.families.first().cloned()
            {
                face.families.insert(0, (family.clone(), language));
                db.remove_face(id);
                db.push_face_info(face);
            }
        }
    }
    Ok(Arc::new(db))
}
