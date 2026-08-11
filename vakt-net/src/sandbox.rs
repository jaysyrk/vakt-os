//! Landlock confinement for the network daemon.
//!
//! vakt-net drives `wpa_supplicant` and `udhcpc` against whatever a network
//! says back, so it gets the filesystem taken away from it. The kernel
//! enforces the ruleset for this process and everything it spawns, and it
//! cannot be undone from inside.

use landlock::{
    ABI, Access, AccessFs, CompatLevel, Compatible, RulesetAttr, RulesetCreatedAttr, RulesetStatus,
    path_beneath_rules,
};
use std::path::Path;

/// Degraded by best-effort compatibility, so an old kernel gets a weaker
/// sandbox rather than a daemon that will not start.
const ABI_TARGET: ABI = ABI::V5;

const PROGRAM_DIRS: &[&str] = &["/bin", "/sbin", "/usr", "/lib", "/lib64"];

const READABLE: &[&str] = &["/proc", "/sys"];

/// `/run` holds the supplicant config, the pid files and the status file.
/// Device nodes are named individually rather than granting `/dev`.
const WRITABLE: &[&str] = &[
    "/run",
    "/dev/null",
    "/dev/zero",
    "/dev/random",
    "/dev/urandom",
    // wpa_supplicant reads this to see whether the radio is blocked.
    "/dev/rfkill",
];

/// Applies the ruleset. Returns what the kernel actually enforced, for the log.
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
        // The only path under /persistent this daemon can see.
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
            " - WARNING: present but still denied: {}",
            blocked.join(", ")
        ),
    })
}

/// Paths that exist but still will not open once the ruleset is sealed. A rule
/// that never applied otherwise surfaces only as an error from a helper several
/// layers down, naming neither Landlock nor this daemon.
fn unreachable<'a>(paths: &'a [&'a str]) -> Vec<&'a str> {
    existing(paths)
        .filter(|p| std::fs::File::open(p).is_err())
        .collect()
}

/// Landlock rules name open file descriptors, so an absent path is an error
/// rather than a no-op. `/lib64` is a symlink here and the fallback config
/// usually does not exist at all.
fn existing<'a>(paths: &'a [&'a str]) -> impl Iterator<Item = &'a str> {
    paths.iter().copied().filter(|p| Path::new(p).exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wired-only hardware has no `/dev/rfkill`; the filter is what stops that
    /// being a daemon that will not start.
    #[test]
    fn absent_paths_are_filtered_out() {
        let kept: Vec<&str> = existing(&["/dev/null", "/definitely/not/here"]).collect();
        assert_eq!(kept, vec!["/dev/null"]);
    }

    #[test]
    fn absence_is_not_denial() {
        assert!(unreachable(&["/dev/null", "/definitely/not/here"]).is_empty());
    }
}
