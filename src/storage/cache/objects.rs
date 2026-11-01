use super::{CacheError, Result, keys::object_name};
use rusqlite::Connection;
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::Path,
};

pub(super) fn ensure_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    if !fs::symlink_metadata(path)?.is_dir() {
        return Err(CacheError::Corrupt);
    }
    Ok(())
}
pub(super) fn publish(root: &Path, id: &str, bytes: &[u8]) -> Result<()> {
    let dir = root.join("objects");
    ensure_directory(&dir)?;
    let temporary = dir.join(format!("{}.tmp", object_name(id)?));
    let mut file = File::options()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(temporary, dir.join(id))?;
    // Unix requires syncing the containing directory for durable rename. Windows
    // does not allow opening directories via File; missing files recover as misses.
    #[cfg(unix)]
    File::open(&dir)?.sync_all()?;
    Ok(())
}
pub(super) fn read(root: &Path, name: &str, size: u64) -> Result<Vec<u8>> {
    let dir = root.join("objects");
    if !fs::symlink_metadata(&dir)?.is_dir() {
        return Err(CacheError::Corrupt);
    }
    let path = dir.join(object_name(name)?);
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_file() || metadata.len() != size {
        return Err(CacheError::Corrupt);
    }
    let mut bytes = Vec::new();
    File::open(path)?.take(size + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != size {
        return Err(CacheError::Corrupt);
    }
    Ok(bytes)
}
pub(super) fn collect(root: &Path, db: &Connection) -> Result<()> {
    let dir = root.join("objects");
    if !fs::symlink_metadata(&dir)?.is_dir() {
        return Err(CacheError::Corrupt);
    }
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let valid = object_name(name).is_ok();
        let temporary = name
            .strip_suffix(".tmp")
            .is_some_and(|id| object_name(id).is_ok());
        if !valid && !temporary {
            continue;
        }
        let referenced: bool = valid
            && db.query_row(
                "SELECT EXISTS(SELECT 1 FROM entries WHERE object=?1)",
                [name],
                |r| r.get(0),
            )?;
        if !referenced {
            // remove_file unlinks symlinks; never recurse or delete unknown paths.
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

// CRC-64/ECMA-182 is an accidental corruption check, not a cryptographic content
// identity or authorization mechanism. UUID filenames never deduplicate by CRC.
const TABLE: [u64; 256] = {
    let mut table = [0; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = (i as u64) << 56;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & (1 << 63) != 0 {
                (crc << 1) ^ 0x42F0E1EBA9EA3693
            } else {
                crc << 1
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};
pub(super) fn checksum(bytes: &[u8]) -> String {
    let crc = bytes.iter().fold(0u64, |crc, b| {
        (crc << 8) ^ TABLE[((crc >> 56) as u8 ^ b) as usize]
    });
    format!("{crc:016x}")
}
