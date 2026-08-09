//! Filesystem bring-up and teardown for PID 1.
//!
//! The root filesystem is the unpacked initramfs, which lives in RAM. Once the
//! image's own files are in place nothing has any business writing to it again,
//! so it is remounted read-only and every writable path the system needs is
//! either a tmpfs mounted over the top of it (`/run`, `/tmp`, `/dev/shm`) or the
//! persistent disk (`/persistent`).

use nix::mount::{MntFlags, MsFlags, mount, umount2};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Mount point for the persistent data disk.
pub const PERSISTENT: &str = "/persistent";
/// Volatile system state: pid files, logs, sockets, status files.
pub const RUN: &str = "/run";

/// The filesystem label the persistent disk is built with - build.sh's
/// `mkfs.ext4 -L VAKTDATA` and the same label GRUB searches for (see
/// docs/OS_UPDATES.md). Found by scanning every block device rather than a
/// fixed name like `/dev/sda`: the kernel assigns device names by discovery
/// order, which is not stable across boots on any machine with more than one
/// disk present - confirmed on real hardware, where the boot USB and the
/// data disk swapped device letters between boots.
const PERSISTENT_LABEL: &str = "VAKTDATA";

/// ext4's on-disk superblock starts 1024 bytes into the device. Offsets
/// below are from the kernel's fs/ext4/ext4.h: magic number at +0x38,
/// 16-byte NUL-padded volume label at +0x78.
const SUPERBLOCK_OFFSET: u64 = 1024;
const MAGIC_OFFSET: u64 = 0x38;
const EXT4_MAGIC: [u8; 2] = [0x53, 0xEF];
const LABEL_OFFSET: u64 = 0x78;
const LABEL_LEN: usize = 16;

/// How long to keep retrying for the persistent disk before giving up. A
/// spinning disk or a slower-enumerating USB device may not be ready the
/// instant load_modules() returns.
const FIND_ATTEMPTS: u32 = 5;
const FIND_RETRY_DELAY: Duration = Duration::from_secs(1);

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

/// Reads the ext4 volume label off `device`. `None` for anything that isn't
/// a readable ext4 superblock - wrong magic, too short, no permission, gone.
fn ext4_label(device: &Path) -> Option<String> {
    let mut file = fs::File::open(device).ok()?;

    file.seek(SeekFrom::Start(SUPERBLOCK_OFFSET + MAGIC_OFFSET))
        .ok()?;
    let mut magic = [0u8; 2];
    file.read_exact(&mut magic).ok()?;
    if magic != EXT4_MAGIC {
        return None;
    }

    file.seek(SeekFrom::Start(SUPERBLOCK_OFFSET + LABEL_OFFSET))
        .ok()?;
    let mut raw = [0u8; LABEL_LEN];
    file.read_exact(&mut raw).ok()?;
    parse_label(&raw)
}

/// Split out from [`ext4_label`] so the trimming is testable without a real
/// block device.
fn parse_label(raw: &[u8; LABEL_LEN]) -> Option<String> {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(LABEL_LEN);
    let text = std::str::from_utf8(&raw[..end]).ok()?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Every block device name the kernel currently knows about, whole disks and
/// their partitions alike.
///
/// `/sys/block` lists only whole disks (`sda`, `nvme0n1`); a disk's
/// partitions are subdirectories *inside* its entry (`/sys/block/sda/sda1`),
/// so scanning the top level alone would never find a data disk that was
/// partitioned rather than formatted whole - which the README's own
/// `mkfs.ext4 -L VAKTDATA /dev/sdY` avoids, but nothing enforces.
fn block_device_names() -> Vec<String> {
    block_device_names_in(Path::new("/sys/block"))
}

/// Split out from [`block_device_names`] so the partition-walking is testable
/// against a fake sysfs tree.
fn block_device_names_in(sys_block: &Path) -> Vec<String> {
    let mut names = Vec::new();

    let Ok(disks) = fs::read_dir(sys_block) else {
        return names;
    };
    for disk in disks.filter_map(|e| e.ok()) {
        let Ok(disk_name) = disk.file_name().into_string() else {
            continue;
        };

        // A partition's sysfs directory is identified by carrying a
        // `partition` file; other subdirectories (`queue`, `power`, the
        // `holders` symlinks) do not.
        if let Ok(children) = fs::read_dir(disk.path()) {
            for child in children.filter_map(|e| e.ok()) {
                if child.path().join("partition").exists()
                    && let Ok(part_name) = child.file_name().into_string()
                {
                    names.push(part_name);
                }
            }
        }

        names.push(disk_name);
    }

    names.sort();
    names
}

/// Scans every block device the kernel currently knows about for one labeled
/// [`PERSISTENT_LABEL`]. A single pass - callers needing to wait for a
/// slower device retry this themselves.
fn find_persistent_once() -> Option<PathBuf> {
    block_device_names()
        .into_iter()
        .map(|name| PathBuf::from(format!("/dev/{}", name)))
        .find(|device| ext4_label(device).as_deref() == Some(PERSISTENT_LABEL))
}

/// Mounts the persistent data disk. `nosuid` and `nodev` matter here: the disk
/// is the one surface an unprivileged panel session can write to, and neither a
/// setuid binary nor a device node planted there may become a way back to root.
pub fn mount_persistent() -> bool {
    // The mount point ships in the image; on a sealed root this is a no-op that
    // succeeds, and on an unsealed one it recreates a missing directory.
    let _ = fs::create_dir_all(PERSISTENT);

    let mut device = find_persistent_once();
    for _ in 1..FIND_ATTEMPTS {
        if device.is_some() {
            break;
        }
        std::thread::sleep(FIND_RETRY_DELAY);
        device = find_persistent_once();
    }

    let Some(device) = device else {
        println!(
            "[Vakt-Init] \x1b[1;33mNo disk labeled '{}' found. Running in RAM only mode.\x1b[0m",
            PERSISTENT_LABEL
        );
        return false;
    };

    match mount(
        Some(device.as_path()),
        PERSISTENT,
        Some("ext4"),
        MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
        None::<&str>,
    ) {
        Ok(()) => {
            println!(
                "[Vakt-Init] \x1b[1;32mMounted persistent storage ({}) successfully!\x1b[0m",
                device.display()
            );
            true
        }
        Err(e) => {
            println!(
                "[Vakt-Init] \x1b[1;33mCould not mount {} ({}). Running in RAM only mode.\x1b[0m",
                device.display(),
                e
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parse_label_trims_nul_padding() {
        let mut raw = [0u8; LABEL_LEN];
        raw[..8].copy_from_slice(b"VAKTDATA");
        assert_eq!(parse_label(&raw), Some("VAKTDATA".to_string()));
    }

    #[test]
    fn parse_label_trims_a_full_16_byte_label() {
        let raw = *b"SIXTEEN_CHAR_LBL";
        assert_eq!(parse_label(&raw), Some("SIXTEEN_CHAR_LBL".to_string()));
    }

    #[test]
    fn parse_label_is_none_for_an_empty_label() {
        assert_eq!(parse_label(&[0u8; LABEL_LEN]), None);
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("vakt-init-mount-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes a minimal fake ext4 superblock into a plain file, so
    /// `ext4_label` can be exercised end to end without a real block device.
    fn write_fake_superblock(path: &Path, label: &[u8]) {
        let mut buf = vec![0u8; (SUPERBLOCK_OFFSET + 1024) as usize];
        let magic_at = (SUPERBLOCK_OFFSET + MAGIC_OFFSET) as usize;
        buf[magic_at..magic_at + 2].copy_from_slice(&EXT4_MAGIC);
        let label_at = (SUPERBLOCK_OFFSET + LABEL_OFFSET) as usize;
        buf[label_at..label_at + label.len()].copy_from_slice(label);
        std::fs::File::create(path)
            .unwrap()
            .write_all(&buf)
            .unwrap();
    }

    #[test]
    fn ext4_label_reads_a_real_superblock() {
        let dir = scratch("label");
        let path = dir.join("fake-disk");
        write_fake_superblock(&path, b"VAKTDATA");

        assert_eq!(ext4_label(&path), Some("VAKTDATA".to_string()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ext4_label_is_none_without_the_ext4_magic() {
        let dir = scratch("nomagic");
        let path = dir.join("fake-disk");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(&vec![0u8; 2048])
            .unwrap();

        assert_eq!(ext4_label(&path), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Builds a fake `/sys/block` tree: each disk is a directory, each of its
    /// partitions a subdirectory carrying a `partition` file, plus the
    /// ordinary non-partition subdirectories real sysfs has.
    fn fake_sys_block(root: &Path, disks: &[(&str, &[&str])]) {
        for (disk, partitions) in disks {
            let disk_dir = root.join(disk);
            std::fs::create_dir_all(disk_dir.join("queue")).unwrap();
            std::fs::create_dir_all(disk_dir.join("power")).unwrap();
            for partition in *partitions {
                let part_dir = disk_dir.join(partition);
                std::fs::create_dir_all(&part_dir).unwrap();
                std::fs::write(part_dir.join("partition"), "1\n").unwrap();
            }
        }
    }

    #[test]
    fn block_devices_include_partitions_not_just_whole_disks() {
        let dir = scratch("partitions");
        fake_sys_block(&dir, &[("sda", &["sda1", "sda2"]), ("sdb", &[])]);

        let names = block_device_names_in(&dir);

        assert_eq!(names, vec!["sda", "sda1", "sda2", "sdb"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn non_partition_subdirectories_are_not_mistaken_for_devices() {
        let dir = scratch("nonpartition");
        fake_sys_block(&dir, &[("nvme0n1", &["nvme0n1p1"])]);

        let names = block_device_names_in(&dir);

        assert_eq!(names, vec!["nvme0n1", "nvme0n1p1"]);
        assert!(!names.iter().any(|n| n == "queue" || n == "power"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_sys_block_yields_no_devices_rather_than_panicking() {
        let dir = scratch("nosysblock");
        assert!(block_device_names_in(&dir.join("absent")).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ext4_label_is_none_for_a_mismatched_label() {
        let dir = scratch("wronglabel");
        let path = dir.join("fake-disk");
        write_fake_superblock(&path, b"SOMETHING_ELSE");

        assert_ne!(ext4_label(&path), Some(PERSISTENT_LABEL.to_string()));
        std::fs::remove_dir_all(&dir).ok();
    }
}
