//! Dropping root before handing the console to the panel.
//!
//! There is no glibc and no NSS in this image, so `/etc/passwd` is read
//! directly rather than through `getpwnam`. That is not a shortcut: the file is
//! the whole of the user database here, written by `build.sh`, and parsing it
//! in-process keeps PID 1 free of a libc lookup path that would have to be
//! present in the initramfs.

use nix::unistd::{Gid, Uid, chown, setgid, setgroups, setuid};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// The unprivileged account the panel and everything it launches runs as.
pub const VAKT_USER: &str = "vakt";

const PASSWD: &str = "/etc/passwd";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub home: String,
}

/// Looks `name` up in the system's `/etc/passwd`.
pub fn lookup(name: &str) -> Option<Identity> {
    let passwd = std::fs::read_to_string(PASSWD).ok()?;
    parse_passwd(&passwd, name)
}

/// Finds one account in the contents of a `passwd(5)` file.
///
/// Split out from [`lookup`] so the parser is testable without touching the
/// host's real user database.
fn parse_passwd(passwd: &str, name: &str) -> Option<Identity> {
    for line in passwd.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // name:password:uid:gid:gecos:home:shell
        let mut fields = line.split(':');
        let entry = fields.next()?;
        if entry != name {
            continue;
        }
        let _password = fields.next()?;
        let uid: u32 = fields.next()?.parse().ok()?;
        let gid: u32 = fields.next()?.parse().ok()?;
        let _gecos = fields.next().unwrap_or("");
        let home = fields.next().unwrap_or("/").to_string();

        return Some(Identity {
            name: entry.to_string(),
            uid,
            gid,
            home,
        });
    }
    None
}

/// Irreversibly becomes `identity`.
///
/// The order is the only order that works: supplementary groups and the group
/// id must go while the process is still root, because dropping the user id
/// first would take away the privilege needed to drop the rest. The final
/// check is what makes "irreversible" a claim rather than a hope - if the
/// kernel left any path back to uid 0, the caller must not continue.
///
/// This is called from `pre_exec`, between `fork` and `exec` in a process that
/// has other threads, so it sticks to async-signal-safe syscalls and allocates
/// nothing - including on the failure path, which is why the last error is a
/// raw errno rather than a message.
pub fn become_user(uid: u32, gid: u32) -> io::Result<()> {
    let uid = Uid::from_raw(uid);
    let gid = Gid::from_raw(gid);

    setgroups(&[gid]).map_err(io::Error::from)?;
    setgid(gid).map_err(io::Error::from)?;
    setuid(uid).map_err(io::Error::from)?;

    if setuid(Uid::from_raw(0)).is_ok() {
        return Err(io::Error::from_raw_os_error(nix::libc::EPERM));
    }
    Ok(())
}

/// Hands a path to the unprivileged user, creating it first if needed.
///
/// Used for the few places the panel legitimately has to write: its home, the
/// package install root, and the directory holding the network configuration
/// it saves.
pub fn grant(path: &Path, identity: &Identity) {
    let _ = std::fs::create_dir_all(path);
    if let Err(e) = chown(
        path,
        Some(Uid::from_raw(identity.uid)),
        Some(Gid::from_raw(identity.gid)),
    ) {
        println!(
            "[Vakt-Init] \x1b[1;33mCould not give {} to {}: {}\x1b[0m",
            path.display(),
            identity.name,
            e
        );
    }
}

/// Creates `path` if it is missing and hands it to `identity`.
///
/// Unlike [`grant`], this is for a single file the unprivileged panel has to
/// be able to rewrite in place. It exists so vakt-net's Landlock ruleset can
/// name the Wi-Fi config path at startup even on an appliance that has never
/// had Wi-Fi configured: Landlock cannot grant a rule for a path that does
/// not exist, and a ruleset can never be widened afterward.
///
/// The file must be owned by the panel's user, not root, or the panel's
/// write would fail with EACCES against a root-owned placeholder.
pub fn grant_file(path: &Path, identity: &Identity) {
    if !path.exists() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // 0600: this file holds the Wi-Fi PSK once the panel writes one.
        if let Err(e) = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
        {
            println!(
                "[Vakt-Init] \x1b[1;33mCould not create {}: {}\x1b[0m",
                path.display(),
                e
            );
            return;
        }
    }

    if let Err(e) = chown(
        path,
        Some(Uid::from_raw(identity.uid)),
        Some(Gid::from_raw(identity.gid)),
    ) {
        println!(
            "[Vakt-Init] \x1b[1;33mCould not give {} to {}: {}\x1b[0m",
            path.display(),
            identity.name,
            e
        );
    }
}

/// Hands an *existing* file to `identity`, doing nothing if it is absent.
///
/// Needed because `chown` on a directory does not recurse: a root-owned file
/// inside a granted directory stays root-owned, and the panel then cannot read
/// it. Never creates the file, unlike [`grant_file`] - an empty auth file
/// parses as a damaged PIN rather than as no PIN.
pub fn adopt_file(path: &Path, identity: &Identity) {
    if !path.exists() {
        return;
    }
    if let Err(e) = chown(
        path,
        Some(Uid::from_raw(identity.uid)),
        Some(Gid::from_raw(identity.gid)),
    ) {
        println!(
            "[Vakt-Init] \x1b[1;33mCould not give {} to {}: {}\x1b[0m",
            path.display(),
            identity.name,
            e
        );
    }
}

/// Hands the console devices to the unprivileged user.
///
/// Without this the panel starts as `vakt` and immediately fails to open a
/// terminal: devtmpfs creates `/dev/console` owned by root with mode 0600, and
/// `cttyhack` needs to open it to give the panel a controlling tty. `/dev/fb0`
/// is here for the same reason - the compositor the panel launches writes to it
/// directly.
pub fn grant_console(identity: &Identity) {
    const DEVICES: &[&str] = &[
        "/dev/console",
        "/dev/tty0",
        "/dev/tty1",
        "/dev/ttyS0",
        "/dev/fb0",
    ];

    for device in DEVICES {
        let path = Path::new(device);
        if !path.exists() {
            continue;
        }
        // Reported rather than ignored: if /dev/console in particular cannot
        // be handed over, the panel and the fallback shell both fail to open
        // a terminal and exit immediately, which looks like a crash loop with
        // no stated cause. Better to name it here than to leave it to be
        // inferred from the loop.
        if let Err(e) = chown(
            path,
            Some(Uid::from_raw(identity.uid)),
            Some(Gid::from_raw(identity.gid)),
        ) {
            println!(
                "[Vakt-Init] \x1b[1;33mCould not give {} to {}: {}\x1b[0m",
                device, identity.name, e
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// grant_file exists so the panel can rewrite the Wi-Fi config in place.
    /// A root-owned placeholder would make the panel's write fail with
    /// EACCES, so the file it creates must be 0600 and must be handed to the
    /// panel's user. The chown itself needs privileges this test suite does
    /// not have, so what is pinned here is the part that always holds:
    /// creation, mode, and that an existing file is never clobbered.
    /// adopt_file must never create the file it is handed.
    ///
    /// grant_file creates a missing file on purpose, for a config the panel
    /// rewrites in place. Doing that to the panel's stored PIN would leave an
    /// empty file, which the panel reads as a PIN it cannot parse - so a brand
    /// new appliance would announce that its old PIN is gone.
    #[test]
    fn adopt_file_leaves_a_missing_file_missing() {
        let dir = std::env::temp_dir().join(format!("vakt-init-adopt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("vakt-panel.auth");

        let identity = Identity {
            name: "vakt".to_string(),
            uid: nix::unistd::Uid::current().as_raw(),
            gid: nix::unistd::Gid::current().as_raw(),
            home: dir.to_string_lossy().into_owned(),
        };

        adopt_file(&path, &identity);
        assert!(
            !path.exists(),
            "adopt_file created a file; an empty one reads as a damaged PIN"
        );

        std::fs::write(&path, "salt:digest\n").unwrap();
        adopt_file(&path, &identity);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "salt:digest\n",
            "adopt_file must not rewrite the stored PIN"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn grant_file_creates_a_private_file_without_clobbering_an_existing_one() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("vakt-init-grantfile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("etc").join("vakt-net.conf");

        let identity = Identity {
            name: "vakt".to_string(),
            uid: nix::unistd::Uid::current().as_raw(),
            gid: nix::unistd::Gid::current().as_raw(),
            home: dir.join("home").to_string_lossy().into_owned(),
        };

        grant_file(&path, &identity);
        assert!(path.is_file(), "grant_file should create a missing file");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the Wi-Fi config holds a PSK");

        std::fs::write(&path, "ssid=Keep\n").unwrap();
        grant_file(&path, &identity);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "ssid=Keep\n",
            "an existing config must never be truncated"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    const SAMPLE: &str = "\
# system accounts
root:x:0:0:root:/root:/bin/sh
vakt:x:1000:1000:Vakt appliance user:/home/vakt:/bin/sh
nobody:x:65534:65534:nobody:/:/bin/false
";

    #[test]
    fn finds_an_account_by_name() {
        let vakt = parse_passwd(SAMPLE, "vakt").expect("vakt should be present");
        assert_eq!(vakt.uid, 1000);
        assert_eq!(vakt.gid, 1000);
        assert_eq!(vakt.home, "/home/vakt");
    }

    #[test]
    fn root_is_still_root() {
        let root = parse_passwd(SAMPLE, "root").unwrap();
        assert_eq!((root.uid, root.gid), (0, 0));
    }

    #[test]
    fn missing_account_is_none() {
        assert_eq!(parse_passwd(SAMPLE, "postgres"), None);
    }

    /// A name that only appears inside another field must not match, or a
    /// crafted gecos string could pick the account the panel runs as.
    #[test]
    fn only_the_first_field_is_matched() {
        assert_eq!(parse_passwd(SAMPLE, "Vakt appliance user"), None);
        assert_eq!(parse_passwd(SAMPLE, "/bin/sh"), None);
    }

    #[test]
    fn malformed_lines_are_skipped_not_fatal() {
        let passwd = "garbage\nvakt:x:notanumber:1000::/home/vakt:/bin/sh\n";
        assert_eq!(parse_passwd(passwd, "vakt"), None);
    }
}
