//! Telling vakt-init that the daemon has finished starting.
//!
//! PID 1 listens on the socket named by `NOTIFY_SOCKET` and holds boot until
//! the daemons check in. The protocol is newline-separated `KEY=value` pairs in
//! one datagram. Sending is best-effort: a daemon that cannot reach PID 1 has
//! nothing useful to do about it.

use std::os::unix::net::UnixDatagram;

/// Set by vakt-init for every service it supervises; absent when the daemon is
/// run by hand.
const SOCKET_ENV: &str = "NOTIFY_SOCKET";

/// Announces that the daemon is up. Safe to call more than once.
pub fn ready(status: &str) {
    send(&format!(
        "READY=1\nNAME=vakt-net\nSTATUS={}\n",
        one_line(status)
    ));
}

/// Updates the status line without claiming readiness.
pub fn status(status: &str) {
    send(&format!("NAME=vakt-net\nSTATUS={}\n", one_line(status)));
}

fn send(message: &str) {
    let Ok(path) = std::env::var(SOCKET_ENV) else {
        return;
    };
    let Ok(socket) = UnixDatagram::unbound() else {
        return;
    };
    let _ = socket.send_to(message.as_bytes(), path);
}

/// A newline in a value would be read as the start of the next key, so status
/// text is flattened before it goes on the wire.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_text_cannot_inject_another_field() {
        assert_eq!(
            one_line("connected\nSHUTDOWN=poweroff"),
            "connected SHUTDOWN=poweroff"
        );
    }

    #[test]
    fn whitespace_is_collapsed() {
        assert_eq!(
            one_line("  lease   acquired \t on eth0 "),
            "lease acquired on eth0"
        );
    }

    /// The daemon is runnable outside vakt-init, so no socket means no-op.
    #[test]
    fn sending_without_a_socket_is_harmless() {
        unsafe { std::env::remove_var(SOCKET_ENV) };
        ready("no listener");
        status("still no listener");
    }
}
