//! A size-capped log sink for supervised daemons.
//!
//! Service output lands in `/run`, a tmpfs, so a daemon looping on an error is
//! a slow way to fill RAM. This keeps a bounded window and discards the rest.
//!
//! The window is the *recent* output: a full file becomes `<name>.log.1` and a
//! fresh one starts, so a daemon's last words survive. Two generations at half
//! the budget each keeps the total under [`BUDGET`] per service.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// Total bytes a single service's logs may occupy.
pub const BUDGET: u64 = 5 * 1024 * 1024;

/// Copy buffer size. Small enough that a chatty daemon does not build up a
/// large unwritten backlog in RAM before the cap is applied.
const CHUNK: usize = 8 * 1024;

pub struct BoundedLog {
    path: PathBuf,
    rotated: PathBuf,
    file: File,
    written: u64,
    /// Bytes allowed in the active file before it is rotated.
    limit: u64,
    /// Total bytes seen, including everything discarded, for the closing note.
    seen: u64,
}

impl BoundedLog {
    /// Opens `path` for writing, truncating whatever the previous run left.
    pub fn create(path: &Path, budget: u64) -> io::Result<Self> {
        // Half the budget per generation, so the active file plus the rotated
        // one together stay inside it. A budget too small to split still gets
        // a usable single-byte-plus limit rather than zero.
        let limit = (budget / 2).max(1);
        Ok(BoundedLog {
            path: path.to_path_buf(),
            rotated: rotated_path(path),
            file: File::create(path)?,
            written: 0,
            limit,
            seen: 0,
        })
    }

    /// Appends a chunk, rotating first if it would overflow the active file.
    ///
    /// A single chunk larger than the whole limit is still written: truncating
    /// mid-line would corrupt the log more than briefly exceeding the cap does,
    /// and the next write rotates it away.
    pub fn write_chunk(&mut self, data: &[u8]) -> io::Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        if self.written > 0 && self.written + data.len() as u64 > self.limit {
            self.rotate()?;
        }

        self.file.write_all(data)?;
        self.written += data.len() as u64;
        self.seen += data.len() as u64;
        Ok(())
    }

    /// Moves the active file aside and starts a new one.
    fn rotate(&mut self) -> io::Result<()> {
        self.file.flush()?;
        let _ = std::fs::remove_file(&self.rotated);
        std::fs::rename(&self.path, &self.rotated)?;

        self.file = File::create(&self.path)?;
        self.written = 0;
        writeln!(
            self.file,
            "--- rotated at {} bytes; previous output is in {} ---",
            self.limit,
            self.rotated.display()
        )?;
        self.written = self.file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(())
    }

    /// Total bytes accepted over this log's lifetime. Only the tests need
    /// this; the supervisor never reads it.
    #[cfg(test)]
    pub fn total_seen(&self) -> u64 {
        self.seen
    }
}

/// `/run/vakt-net.log` becomes `/run/vakt-net.log.1`.
fn rotated_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".1");
    PathBuf::from(name)
}

/// Drains `source` into a capped log until the far end closes it.
///
/// This is what the supervisor runs on a thread per service: the read end of
/// the pipe the daemon's stdout and stderr both point at. It returns when the
/// daemon exits and the pipe reaches EOF.
pub fn drain<R: Read>(mut source: R, mut log: BoundedLog) {
    let mut buffer = [0u8; CHUNK];
    loop {
        match source.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                if log.write_chunk(&buffer[..n]).is_err() {
                    break;
                }
            }
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vakt-log-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn short_output_is_written_verbatim() {
        let dir = scratch("short");
        let path = dir.join("svc.log");
        let mut log = BoundedLog::create(&path, BUDGET).unwrap();
        log.write_chunk(b"hello\n").unwrap();
        log.write_chunk(b"world\n").unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello\nworld\n");
        assert!(!rotated_path(&path).exists(), "no rotation was needed");
    }

    /// The point of the whole module: a daemon that will not stop talking must
    /// not be able to grow its log without bound.
    #[test]
    fn runaway_output_stays_inside_the_budget() {
        let dir = scratch("runaway");
        let path = dir.join("noisy.log");
        let budget = 8 * 1024;

        let mut log = BoundedLog::create(&path, budget).unwrap();
        let line = vec![b'x'; 512];
        for _ in 0..2000 {
            log.write_chunk(&line).unwrap();
        }

        let active = std::fs::metadata(&path).unwrap().len();
        let rotated = std::fs::metadata(rotated_path(&path))
            .map(|m| m.len())
            .unwrap_or(0);

        assert!(
            active + rotated <= budget + line.len() as u64,
            "logs grew to {} bytes on a {} byte budget",
            active + rotated,
            budget
        );
        assert!(
            log.total_seen() > budget * 100,
            "test did not actually produce a runaway"
        );
    }

    /// Rotation has to preserve the newest output, not the oldest - the last
    /// thing a daemon printed before dying is the part worth keeping.
    #[test]
    fn the_most_recent_output_survives_rotation() {
        let dir = scratch("recent");
        let path = dir.join("svc.log");

        let mut log = BoundedLog::create(&path, 2048).unwrap();
        log.write_chunk(&vec![b'a'; 900]).unwrap();
        log.write_chunk(b"the-important-last-words\n").unwrap();
        log.write_chunk(&vec![b'b'; 900]).unwrap();

        let active = std::fs::read_to_string(&path).unwrap();
        let rotated = std::fs::read_to_string(rotated_path(&path)).unwrap();
        assert!(rotated.contains("the-important-last-words"));
        assert!(active.contains("rotated at"), "got: {}", active);
    }

    #[test]
    fn only_one_generation_is_kept() {
        let dir = scratch("generations");
        let path = dir.join("svc.log");

        let mut log = BoundedLog::create(&path, 512).unwrap();
        for _ in 0..50 {
            log.write_chunk(&[b'z'; 200]).unwrap();
        }

        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec!["svc.log".to_string(), "svc.log.1".to_string()]);
    }

    #[test]
    fn drain_copies_a_stream_into_the_log() {
        let dir = scratch("drain");
        let path = dir.join("svc.log");
        let log = BoundedLog::create(&path, BUDGET).unwrap();

        drain(&b"line one\nline two\n"[..], log);

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "line one\nline two\n"
        );
    }
}
