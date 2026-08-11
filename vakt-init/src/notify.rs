//! The readiness and control socket at `/run/init.sock`.
//!
//! Daemons tell PID 1 they have finished starting by sending a datagram here,
//! so the panel is drawn when the system is usable rather than after a guessed
//! delay. The format is the same shape as systemd's `sd_notify` - newline
//! separated `KEY=value` pairs - because it needs no library to speak.
//!
//! ```text
//! READY=1
//! STATUS=lease acquired on eth0
//! ```
//!
//! The sender is identified by the credentials the kernel attaches
//! (`SO_PASSCRED`), not by the message body, so a service cannot report
//! readiness for another. `NAME=` is consulted only without credentials.

use nix::sys::socket::{
    AddressFamily, ControlMessageOwned, MsgFlags, SockFlag, SockType, UnixAddr, bind, recvmsg,
    setsockopt, socket, sockopt,
};
use nix::unistd::{Gid, Uid, chown};
use std::io::IoSliceMut;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};

/// Where the socket lives. Daemons are handed this in `NOTIFY_SOCKET`.
pub const SOCKET_PATH: &str = "/run/init.sock";

/// The environment variable a supervised daemon reads to find the socket.
/// Named after the systemd convention so the protocol is recognisable.
pub const SOCKET_ENV: &str = "NOTIFY_SOCKET";

/// Datagrams are tiny; anything larger is a bug or an attack, and truncating
/// is the right answer to both.
const MAX_DATAGRAM: usize = 4096;

/// One parsed notification.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Message {
    /// The sending process, from the socket's credentials. `None` means the
    /// kernel attached none and the sender must be trusted by name instead.
    pub pid: Option<u32>,
    /// `READY=1` was present.
    pub ready: bool,
    /// `NAME=`, used only as a fallback identity.
    pub name: Option<String>,
    /// `STATUS=`, a human-readable line for the panel.
    pub status: Option<String>,
    /// `SHUTDOWN=`, the raw verb; the caller decides what it means.
    pub shutdown: Option<String>,
}

/// Parses the body of a notification datagram.
pub fn parse(body: &str) -> Message {
    let mut message = Message::default();

    for line in body.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();

        match key.trim() {
            "READY" => message.ready = value == "1",
            "NAME" if !value.is_empty() => message.name = Some(value.to_string()),
            "STATUS" if !value.is_empty() => message.status = Some(value.to_string()),
            "SHUTDOWN" if !value.is_empty() => message.shutdown = Some(value.to_ascii_lowercase()),
            _ => {}
        }
    }
    message
}

pub struct Listener {
    socket: OwnedFd,
    path: PathBuf,
}

impl Listener {
    /// Creates the socket, replacing a stale one left by a previous boot.
    ///
    /// The socket is group-writable rather than world-writable: daemons run as
    /// root and the panel runs as `vakt`, and nothing else on the system has
    /// any business sending PID 1 a shutdown request.
    pub fn bind(path: &Path, group: Option<u32>) -> nix::Result<Listener> {
        let _ = std::fs::remove_file(path);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let socket = socket(
            AddressFamily::Unix,
            SockType::Datagram,
            SockFlag::SOCK_CLOEXEC,
            None,
        )?;
        setsockopt(&socket, sockopt::PassCred, &true)?;
        bind(socket.as_raw_fd(), &UnixAddr::new(path)?)?;

        if let Some(gid) = group {
            let _ = chown(path, Some(Uid::from_raw(0)), Some(Gid::from_raw(gid)));
            set_mode(path, 0o660);
        } else {
            set_mode(path, 0o600);
        }

        Ok(Listener {
            socket,
            path: path.to_path_buf(),
        })
    }

    /// Blocks until a datagram arrives.
    pub fn recv(&self) -> nix::Result<Message> {
        let mut buffer = [0u8; MAX_DATAGRAM];
        let mut iov = [IoSliceMut::new(&mut buffer)];
        let mut cmsg = nix::cmsg_space!(nix::libc::ucred);

        let received = recvmsg::<UnixAddr>(
            self.socket.as_raw_fd(),
            &mut iov,
            Some(&mut cmsg),
            MsgFlags::empty(),
        )?;

        let mut pid = None;
        if let Ok(control) = received.cmsgs() {
            for message in control {
                if let ControlMessageOwned::ScmCredentials(credentials) = message {
                    pid = Some(credentials.pid() as u32);
                }
            }
        }

        // Reading `bytes` is the last use of `received`, which is what
        // releases its borrow of `buffer` - an explicit drop would be a no-op
        // here, since RecvMsg is Copy.
        let length = received.bytes.min(MAX_DATAGRAM);

        let mut message = parse(&String::from_utf8_lossy(&buffer[..length]));
        message.pid = pid;
        Ok(message)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixDatagram;

    #[test]
    fn parses_a_readiness_datagram() {
        let message = parse("READY=1\nSTATUS=lease acquired on eth0\n");
        assert!(message.ready);
        assert_eq!(message.status.as_deref(), Some("lease acquired on eth0"));
        assert_eq!(message.shutdown, None);
    }

    #[test]
    fn ready_must_be_exactly_one() {
        assert!(!parse("READY=0\n").ready);
        assert!(!parse("READY=yes\n").ready);
        assert!(!parse("READY\n").ready);
    }

    #[test]
    fn parses_a_shutdown_request_case_insensitively() {
        assert_eq!(
            parse("SHUTDOWN=PowerOff").shutdown.as_deref(),
            Some("poweroff")
        );
    }

    #[test]
    fn unknown_keys_and_junk_are_ignored() {
        let message = parse("EXECSTART=/bin/sh\n\n???\nREADY=1\nNAME=vakt-net");
        assert!(message.ready);
        assert_eq!(message.name.as_deref(), Some("vakt-net"));
    }

    /// End to end over a real socket: the identity must come from the kernel's
    /// credentials, not from anything the sender put in the body.
    #[test]
    fn receives_credentials_from_the_kernel() {
        let dir = std::env::temp_dir().join(format!("vakt-notify-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("init.sock");

        let listener = Listener::bind(&path, None).expect("bind");

        let client = UnixDatagram::unbound().expect("client socket");
        client
            .send_to(b"READY=1\nNAME=someone-else\n", &path)
            .expect("send");

        let message = listener.recv().expect("recv");
        assert!(message.ready);
        assert_eq!(
            message.pid,
            Some(std::process::id()),
            "the sender's real pid must be reported"
        );

        drop(listener);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
