//! Reads and writes a GRUB environment block (see GRUB's `lib/envblk.c`): a
//! signature line, `key=value` lines, then `#` padding to a fixed size.
//! Duplicated in vakt-update/src/envblock.rs - keep both in sync. Unvalidated
//! against a real GRUB build - see docs/OS_UPDATES.md.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

const SIGNATURE: &[u8] = b"# GRUB Environment Block\n";
const BLOCK_SIZE: usize = 1024;

/// Writes `entries` as a GRUB environment block, atomically.
pub fn write(path: &Path, entries: &[(&str, &str)]) -> io::Result<()> {
    let mut buf = Vec::with_capacity(BLOCK_SIZE);
    buf.extend_from_slice(SIGNATURE);
    for (key, value) in entries {
        if key.contains('=') || key.contains('\n') || value.contains('\n') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid GRUB env entry: {}={}", key, value),
            ));
        }
        buf.extend_from_slice(key.as_bytes());
        buf.push(b'=');
        buf.extend_from_slice(value.as_bytes());
        buf.push(b'\n');
    }
    if buf.len() > BLOCK_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "GRUB env block entries are {} bytes, over the {}-byte block size",
                buf.len(),
                BLOCK_SIZE
            ),
        ));
    }
    buf.resize(BLOCK_SIZE, b'#');

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    let mut file = fs::File::create(&tmp)?;
    file.write_all(&buf)?;
    file.sync_all()?;
    fs::rename(&tmp, path)
}

/// Reads back the entries in a GRUB environment block written by [`write`].
pub fn read(path: &Path) -> io::Result<BTreeMap<String, String>> {
    let data = fs::read(path)?;
    if !data.starts_with(SIGNATURE) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} does not start with the GRUB env block signature",
                path.display()
            ),
        ));
    }
    let rest = &data[SIGNATURE.len()..];
    let text = String::from_utf8_lossy(rest);

    let mut entries = BTreeMap::new();
    for line in text.lines() {
        if line.starts_with('#') {
            break;
        }
        if let Some((key, value)) = line.split_once('=') {
            entries.insert(key.to_string(), value.to_string());
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("vakt-init-envblock-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_written_block_reads_back_the_same_entries() {
        let dir = scratch("roundtrip");
        let path = dir.join("bootenv");

        write(&path, &[("vakt_active", "B"), ("vakt_previous", "A")]).unwrap();
        let entries = read(&path).unwrap();

        assert_eq!(entries.get("vakt_active"), Some(&"B".to_string()));
        assert_eq!(entries.get("vakt_previous"), Some(&"A".to_string()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_written_file_is_exactly_one_block() {
        let dir = scratch("size");
        let path = dir.join("bootenv");

        write(&path, &[("vakt_active", "A")]).unwrap();
        let size = std::fs::metadata(&path).unwrap().len();

        assert_eq!(size, BLOCK_SIZE as u64);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_second_write_overwrites_the_first_rather_than_appending() {
        let dir = scratch("overwrite");
        let path = dir.join("bootenv");

        write(&path, &[("vakt_active", "A")]).unwrap();
        write(&path, &[("vakt_active", "B")]).unwrap();
        let entries = read(&path).unwrap();

        assert_eq!(entries.get("vakt_active"), Some(&"B".to_string()));
        assert_eq!(entries.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn entries_too_large_for_the_block_are_rejected_rather_than_truncated() {
        let dir = scratch("oversize");
        let path = dir.join("bootenv");
        let huge_value = "x".repeat(BLOCK_SIZE);

        let result = write(&path, &[("k", &huge_value)]);

        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
