//! Reads and writes a GRUB environment block (see GRUB's `lib/envblk.c`): a
//! signature line, `key=value` lines, then `#` padding to a fixed size.
//! Reimplemented rather than shipping `grub-editenv`. Unvalidated against a
//! real GRUB build - see docs/OS_UPDATES.md.

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;

const SIGNATURE: &[u8] = b"# GRUB Environment Block\n";
const BLOCK_SIZE: usize = 1024;

/// Writes `entries` as a GRUB environment block, atomically.
pub fn write(path: &Path, entries: &[(&str, &str)]) -> Result<()> {
    let mut buf = Vec::with_capacity(BLOCK_SIZE);
    buf.extend_from_slice(SIGNATURE);
    for (key, value) in entries {
        if key.contains('=') || key.contains('\n') || value.contains('\n') {
            bail!("invalid GRUB env entry: {}={}", key, value);
        }
        buf.extend_from_slice(key.as_bytes());
        buf.push(b'=');
        buf.extend_from_slice(value.as_bytes());
        buf.push(b'\n');
    }
    if buf.len() > BLOCK_SIZE {
        bail!(
            "GRUB env block entries are {} bytes, over the {}-byte block size",
            buf.len(),
            BLOCK_SIZE
        );
    }
    buf.resize(BLOCK_SIZE, b'#');

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    let mut file = fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    file.write_all(&buf)?;
    file.sync_all()?;
    fs::rename(&tmp, path)
        .with_context(|| format!("renaming {} to {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Reads back the entries in a GRUB environment block written by [`write`].
pub fn read(path: &Path) -> Result<BTreeMap<String, String>> {
    let data = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    if !data.starts_with(SIGNATURE) {
        bail!(
            "{} does not start with the GRUB env block signature",
            path.display()
        );
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

    #[test]
    fn a_written_block_reads_back_the_same_entries() {
        let dir = std::env::temp_dir().join(format!("vakt-envblock-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bootenv");

        write(&path, &[("vakt_active", "B"), ("vakt_previous", "A")]).unwrap();
        let entries = read(&path).unwrap();

        assert_eq!(entries.get("vakt_active"), Some(&"B".to_string()));
        assert_eq!(entries.get("vakt_previous"), Some(&"A".to_string()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_written_file_is_exactly_one_block() {
        let dir =
            std::env::temp_dir().join(format!("vakt-envblock-test-size-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bootenv");

        write(&path, &[("vakt_active", "A")]).unwrap();
        let size = std::fs::metadata(&path).unwrap().len();

        assert_eq!(size, BLOCK_SIZE as u64);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_second_write_overwrites_the_first_rather_than_appending() {
        let dir = std::env::temp_dir().join(format!(
            "vakt-envblock-test-overwrite-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
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
        let dir = std::env::temp_dir().join(format!(
            "vakt-envblock-test-oversize-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bootenv");
        let huge_value = "x".repeat(BLOCK_SIZE);

        let result = write(&path, &[("k", &huge_value)]);

        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
