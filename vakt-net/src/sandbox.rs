//! Landlock confinement for the network daemon.
//!
//! vakt-net handles the one input on this system that comes from outside it:
//! whatever a Wi-Fi network says back during association and DHCP. It is also
//! the daemon with the most attack surface behind it, since it drives
//! `wpa_supplicant` and `udhcpc` and their config files. Landlock takes away
//! the filesystem as an escalation route: the kernel enforces the ruleset for
//! this process and every process it spawns, and unlike a chroot it cannot be
//! undone from inside.
//!
//! What survives the ruleset is only what the daemon demonstrably needs:
//!
//! * read and execute on the image's own program directories, because the work
//!   is done by `ip`, `wpa_supplicant` and `udhcpc`, and a
//!   Landlock ruleset is inherited by everything they run in turn;
//! * read and write under `/run`, where the supplicant config, the pid files,
//!   the resolver configuration and the status file live, plus a few device
//!   nodes by name - including `/dev/rfkill`, which wpa_supplicant needs to
//!   see whether the radio is blocked;
//! * read on `/proc` and `/sys`, which the network tools consult;
//! * read on **exactly** `/persistent/etc/vakt-net.conf` and its RAM-only
//!   fallback - the credentials file is the only thing under `/persistent` this
//!   daemon can see at all. Installed packages, the intrusion detection
//!   baseline and every other file on the data disk are unreachable.
//!
//! Everything else - `/root`, `/etc`, the rest of the persistent disk - is
//! denied, including to the helpers.

use landlock::{
    ABI, Access, AccessFs, CompatLevel, Compatible, RulesetAttr, RulesetCreatedAttr, RulesetStatus,
    path_beneath_rules,
};
use std::path::Path;

/// The Landlock feature set to target. Best-effort compatibility degrades this
/// to whatever the running kernel actually implements, so an older kernel gets
/// a weaker sandbox rather than a failed daemon.
const ABI_TARGET: ABI = ABI::V5;

/// Directories the daemon may read from and run programs out of.
const PROGRAM_DIRS: &[&str] = &["/bin", "/sbin", "/usr", "/lib", "/lib64"];

/// Paths the daemon may read but not write. `ip` and `udhcpc` both consult
/// these; nothing here is writable, so neither is anything they spawn.
const READABLE: &[&str] = &["/proc", "/sys"];

/// Paths the daemon may read and write.
///
/// `/run` holds the supplicant configuration, the pid files, the status file
/// and the resolver configuration that `/etc/resolv.conf` points at. Device
/// nodes are named individually rather than granting `/dev`: the network
/// daemon has no business reaching the framebuffer or the disk.
const WRITABLE: &[&str] = &[
    "/run",
    "/dev/null",
    "/dev/zero",
    "/dev/random",
    "/dev/urandom",
    // wpa_supplicant opens this to see whether the radio is blocked, and it
    // inherits this ruleset. Without it: "Cannot open RFKILL control device",
    // no association, and a Wi-Fi network that never comes up. Filtered by
    // `existing`, so a machine with no radio simply has no rule for it.
    "/dev/rfkill",
];

/// Applies the ruleset to this process. Returns a line describing what the
/// kernel actually enforced, for the daemon's log.
pub fn confine(config_files: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let all = AccessFs::from_all(ABI_TARGET);
    let read = AccessFs::from_read(ABI_TARGET);

    let status = landlock::Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(all)?
        .create()?
        .add_rules(path_beneath_rules(existing(PROGRAM_DIRS), read))?
        .add_rules(path_beneath_rules(existing(READABLE), read))?
        .add_rules(path_beneath_rules(existing(WRITABLE), all))?
        // The one path under /persistent this daemon is allowed to see.
        .add_rules(path_beneath_rules(existing(config_files), read))?
        .restrict_self()?;

    Ok(match status.ruleset {
        RulesetStatus::FullyEnforced => "sandbox active (full Landlock enforcement)".to_string(),
        RulesetStatus::PartiallyEnforced => {
            "sandbox active (partial: this kernel supports an older Landlock ABI)".to_string()
        }
        RulesetStatus::NotEnforced => {
            "sandbox INACTIVE (this kernel has no Landlock support)".to_string()
        }
    } + &match unreachable(&[WRITABLE, config_files].concat()) {
        blocked if blocked.is_empty() => String::new(),
        blocked => format!(
            " - WARNING: present but still denied: {}. Anything needing these \
             will fail, and the error will name the helper rather than the \
             sandbox.",
            blocked.join(", ")
        ),
    })
}

/// Paths that exist but still cannot be opened once the ruleset is sealed.
///
/// A rule that never made it in surfaces only as an error several layers
/// down - "Cannot open RFKILL control device" rather than anything naming
/// Landlock - so the daemon checks its own grants and says so.
fn unreachable<'a>(paths: &'a [&'a str]) -> Vec<&'a str> {
    existing(paths)
        .filter(|p| std::fs::File::open(p).is_err())
        .collect()
}

/// Landlock rules name open file descriptors, so a path that does not exist is
/// an error rather than a no-op. `/lib64` is a symlink on this image and the
/// fallback config file usually is not there at all, so the list is filtered
/// before it reaches the kernel.
fn existing<'a>(paths: &'a [&'a str]) -> impl Iterator<Item = &'a str> {
    paths.iter().copied().filter(|p| Path::new(p).exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A machine with no radio has no `/dev/rfkill`, and a Landlock rule for a
    /// path that does not exist is an error rather than a no-op - so the
    /// filter is what stops the daemon failing to start on wired-only
    /// hardware.
    #[test]
    fn absent_paths_are_filtered_out() {
        let kept: Vec<&str> = existing(&["/dev/null", "/definitely/not/here"]).collect();
        assert_eq!(kept, vec!["/dev/null"]);
    }

    /// A merely absent path is not a denied one, so it must not be reported
    /// as a sandbox problem on hardware that simply lacks the device.
    #[test]
    fn absence_is_not_denial() {
        assert!(unreachable(&["/dev/null", "/definitely/not/here"]).is_empty());
    }
}
