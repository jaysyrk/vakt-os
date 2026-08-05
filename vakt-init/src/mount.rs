//! Filesystem bring-up and teardown for PID 1.
//!
//! The root filesystem is the unpacked initramfs, which lives in RAM. Once the
//! image's own files are in place nothing has any business writing to it again,
//! so it is remounted read-only and every writable path the system needs is
//! either a tmpfs mounted over the top of it (`/run`, `/tmp`, `/dev/shm`) or the
//! persistent disk (`/persistent`).

use nix::mount::{MntFlags, MsFlags, mount, umount2};
use std::fs;
use std::path::Path;

/// Mount point for the persistent data disk.
pub const PERSISTENT: &str = "/persistent";
/// The block device the data disk is expected to appear as.
const PERSISTENT_DEV: &str = "/dev/sda";
/// Volatile system state: pid files, logs, sockets, status files.
pub const RUN: &str = "/run";

/// Nothing that belongs in a volatile system directory needs to be a setuid
/// binary or a device node, and only `/tmp` ever holds anything executable -
/// which zrpkg stages there but never runs.
fn volatile_flags() -> MsFlags {
    MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_NOEXEC
}

fn report(what: &str, result: nix::Result<()>) -> bool {
    match result {
        Ok(()) => true,
        Err(e) => {
            println!("[Vakt-Init] \x1b[1;33mCould not {}: {}\x1b[0m", what, e);
            false
        }
    }
}

/// Mounts the kernel's pseudo-filesystems. Everything below depends on these,
/// so it runs before anything else.
pub fn virtual_filesystems() {
    let none: Option<&str> = None;
    let flags = MsFlags::empty();

    report(
        "mount /proc",
        mount(Some("proc"), "/proc", Some("proc"), flags, none),
    );
    report(
        "mount /sys",
        mount(Some("sys"), "/sys", Some("sysfs"), flags, none),
    );
    report(
        "mount /dev",
        mount(
            Some("dev"),
            "/dev",
            Some("devtmpfs"),
            MsFlags::MS_NOSUID,
            none,
        ),
    );

    // A pty multiplexer is what lets the panel hand a terminal to a child.
    let _ = fs::create_dir_all("/dev/pts");
    report(
        "mount /dev/pts",
        mount(
            Some("devpts"),
            "/dev/pts",
            Some("devpts"),
            MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC,
            Some("mode=0620,gid=5"),
        ),
    );
}

/// Mounts the writable tmpfs overlays. Sizes are capped so a runaway daemon
/// filling a log or a download cannot consume all of RAM.
pub fn volatile_filesystems() {
    let _ = fs::create_dir_all(RUN);
    report(
        "mount /run",
        mount(
            Some("tmpfs"),
            RUN,
            Some("tmpfs"),
            volatile_flags(),
            Some("mode=0755,size=64M"),
        ),
    );

    let _ = fs::create_dir_all("/tmp");
    report(
        "mount /tmp",
        mount(
            Some("tmpfs"),
            "/tmp",
            Some("tmpfs"),
            volatile_flags(),
            Some("mode=1777,size=64M"),
        ),
    );

    let _ = fs::create_dir_all("/dev/shm");
    report(
        "mount /dev/shm",
        mount(
            Some("tmpfs"),
            "/dev/shm",
            Some("tmpfs"),
            volatile_flags(),
            Some("mode=1777,size=32M"),
        ),
    );
}

/// Remounts `/` read-only. Call this only after [`volatile_filesystems`], or
/// there will be nowhere left to write.
///
/// Returns whether the root is now sealed; a kernel built without tmpfs-backed
/// initramfs support can refuse, and the system is still usable if it does.
pub fn seal_root() -> bool {
    let sealed = report(
        "remount / read-only",
        mount(
            None::<&str>,
            "/",
            None::<&str>,
            MsFlags::MS_REMOUNT | MsFlags::MS_RDONLY,
            None::<&str>,
        ),
    );
    if sealed {
        println!("[Vakt-Init] Root filesystem is read-only.");
    }
    sealed
}

/// Mounts the persistent data disk. `nosuid` and `nodev` matter here: the disk
/// is the one surface an unprivileged panel session can write to, and neither a
/// setuid binary nor a device node planted there may become a way back to root.
pub fn mount_persistent() -> bool {
    // The mount point ships in the image; on a sealed root this is a no-op that
    // succeeds, and on an unsealed one it recreates a missing directory.
    let _ = fs::create_dir_all(PERSISTENT);

    if !Path::new(PERSISTENT_DEV).exists() {
        println!(
            "[Vakt-Init] \x1b[1;33mNo {} present. Running in RAM only mode.\x1b[0m",
            PERSISTENT_DEV
        );
        return false;
    }

    match mount(
        Some(PERSISTENT_DEV),
        PERSISTENT,
        Some("ext4"),
        MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
        None::<&str>,
    ) {
        Ok(()) => {
            println!("[Vakt-Init] \x1b[1;32mMounted persistent storage successfully!\x1b[0m");
            true
        }
        Err(e) => {
            println!(
                "[Vakt-Init] \x1b[1;33mCould not mount {} ({}). Running in RAM only mode.\x1b[0m",
                PERSISTENT_DEV, e
            );
            false
        }
    }
}

/// Unmounts the data disk during shutdown. A lazy unmount is the fallback so a
/// process still holding a file open cannot leave the disk mounted at reboot -
/// by this point the buffers have already been flushed by `sync`.
pub fn unmount_persistent() {
    match umount2(PERSISTENT, MntFlags::empty()) {
        Ok(()) => println!("[Vakt-Init] Unmounted {}.", PERSISTENT),
        Err(nix::errno::Errno::EINVAL) => {} // never was mounted
        Err(e) => {
            println!(
                "[Vakt-Init] \x1b[1;33m{} busy ({}); detaching lazily.\x1b[0m",
                PERSISTENT, e
            );
            let _ = umount2(PERSISTENT, MntFlags::MNT_DETACH);
        }
    }
}
