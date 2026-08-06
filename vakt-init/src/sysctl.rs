//! Boot-time kernel hardening via `/proc/sys`.
//!
//! `lsm=landlock,yama` on the kernel command line loads Yama, but loading the
//! LSM does not turn on `ptrace_scope` - that is a separate sysctl, and the
//! default leaves cross-process ptrace open. The same is true of the network
//! sysctls below: the kernel ships sensible defaults for a general-purpose
//! machine, not for an appliance with a fixed, known network role. Nothing
//! here is a symbol the kernel build has to carry specially; every path is
//! written only if it already exists, so a kernel built without a given knob
//! degrades to leaving it alone rather than failing boot.

use std::path::Path;

struct Sysctl {
    path: &'static str,
    value: &'static str,
}

/// Kept in sync by hand with the equivalent list `vakt-audit` checks in
/// `tools/cmd/vakt-audit/main.go` - that tool has to read these values
/// independently of how they were set to mean anything as an audit.
const HARDENING: &[Sysctl] = &[
    // Ptrace is otherwise usable between any two processes owned by the same
    // user; restricting it to direct parent/child is what Yama being loaded
    // is actually for.
    Sysctl {
        path: "/proc/sys/kernel/yama/ptrace_scope",
        value: "1",
    },
    // Kernel pointers and dmesg both leak addresses useful for defeating
    // KASLR from userspace.
    Sysctl {
        path: "/proc/sys/kernel/kptr_restrict",
        value: "2",
    },
    Sysctl {
        path: "/proc/sys/kernel/dmesg_restrict",
        value: "1",
    },
    // Reverse path filtering drops packets whose source address could not
    // have arrived on the interface they came in on - a basic anti-spoofing
    // measure with no legitimate traffic on this appliance's network role.
    Sysctl {
        path: "/proc/sys/net/ipv4/conf/all/rp_filter",
        value: "1",
    },
    Sysctl {
        path: "/proc/sys/net/ipv4/conf/default/rp_filter",
        value: "1",
    },
    // ICMP redirects and source routing are both ways to steer this
    // appliance's routing table from off-box; nothing here depends on either.
    Sysctl {
        path: "/proc/sys/net/ipv4/conf/all/accept_redirects",
        value: "0",
    },
    Sysctl {
        path: "/proc/sys/net/ipv4/conf/default/accept_redirects",
        value: "0",
    },
    Sysctl {
        path: "/proc/sys/net/ipv4/conf/all/accept_source_route",
        value: "0",
    },
    Sysctl {
        path: "/proc/sys/net/ipv4/conf/default/accept_source_route",
        value: "0",
    },
    Sysctl {
        path: "/proc/sys/net/ipv4/icmp_echo_ignore_broadcasts",
        value: "1",
    },
    Sysctl {
        path: "/proc/sys/net/ipv4/tcp_syncookies",
        value: "1",
    },
];

/// Applies the fixed hardening set under `root`, skipping any path that does
/// not exist there. Returns (applied, total) so the caller can report how
/// much of the intended hardening actually took on this kernel build.
fn apply(root: &Path, entries: &[Sysctl]) -> (usize, usize) {
    let mut applied = 0;
    for entry in entries {
        let path = root.join(entry.path.trim_start_matches('/'));
        if path.exists() && std::fs::write(&path, entry.value).is_ok() {
            applied += 1;
        }
    }
    (applied, entries.len())
}

/// Applies the hardening set to the real system. Call only after `/proc` is
/// mounted.
pub fn harden() -> (usize, usize) {
    apply(Path::new("/"), HARDENING)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("vakt-sysctl-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn writes_only_to_paths_that_exist() {
        let dir = scratch("exists");
        let existing = dir.join("proc/sys/kernel/dmesg_restrict");
        fs::create_dir_all(existing.parent().unwrap()).unwrap();
        fs::write(&existing, "0").unwrap();

        let entries = &[
            Sysctl {
                path: "/proc/sys/kernel/dmesg_restrict",
                value: "1",
            },
            Sysctl {
                path: "/proc/sys/does/not/exist",
                value: "1",
            },
        ];

        let (applied, total) = apply(&dir, entries);
        assert_eq!((applied, total), (1, 2));
        assert_eq!(fs::read_to_string(&existing).unwrap(), "1");
    }

    #[test]
    fn a_missing_path_does_not_abort_the_rest() {
        let dir = scratch("missing-first");
        let second = dir.join("proc/sys/kernel/kptr_restrict");
        fs::create_dir_all(second.parent().unwrap()).unwrap();
        fs::write(&second, "0").unwrap();

        let entries = &[
            Sysctl {
                path: "/proc/sys/does/not/exist",
                value: "1",
            },
            Sysctl {
                path: "/proc/sys/kernel/kptr_restrict",
                value: "2",
            },
        ];

        let (applied, _) = apply(&dir, entries);
        assert_eq!(applied, 1);
        assert_eq!(fs::read_to_string(&second).unwrap(), "2");
    }

    #[test]
    fn the_real_hardening_list_has_no_duplicate_paths() {
        let mut paths: Vec<&str> = HARDENING.iter().map(|s| s.path).collect();
        paths.sort_unstable();
        paths.dedup();
        assert_eq!(
            paths.len(),
            HARDENING.len(),
            "the hardening list has a duplicate path"
        );
    }
}
