//! Whether an interactive session owns the console.
//!
//! PID 1 and its background threads share `/dev/console` with the panel and
//! the shell. A service reporting ready while someone is typing splices a line
//! into their command, and a TUI never repaints those cells, so fragments of
//! it stay on screen. Neither is cosmetic: both make the console lie about
//! what was typed and what is there.
//!
//! Nothing is lost by holding messages back - they go to the log either way.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

const LOG: &str = "/run/vakt-init.log";

static BUSY: AtomicBool = AtomicBool::new(false);

/// Marks the console as owned for as long as the guard lives.
pub struct Session;

pub fn claim() -> Session {
    BUSY.store(true, Ordering::SeqCst);
    Session
}

impl Drop for Session {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::SeqCst);
    }
}

pub fn owned() -> bool {
    BUSY.load(Ordering::SeqCst)
}

/// `println!` for anything that can run while a session owns the console.
macro_rules! note {
    ($($arg:tt)*) => { $crate::console::emit(&format!($($arg)*)) };
}
pub(crate) use note;

/// Prints to the console when nothing else is using it, and to the log when
/// something is. Boot output, which happens before any session exists, is
/// unaffected.
pub fn emit(line: &str) {
    if !owned() {
        println!("{}", line);
        let _ = std::io::stdout().flush();
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(LOG)
    {
        let _ = writeln!(file, "{}", line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard is what makes this safe to use from a background thread: a
    /// session that ends by any path has to release the console.
    #[test]
    fn the_console_is_released_when_the_session_ends() {
        assert!(!owned());
        {
            let _session = claim();
            assert!(owned());
        }
        assert!(!owned(), "the guard did not release the console");
    }
}
