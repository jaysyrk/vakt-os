//! Landlock confinement for the compositor.
//!
//! The compositor maps `/dev/fb0` and writes pixels into it, so its ruleset is
//! that one device node and nothing else. It can be this strict because
//! Landlock is applied after `exec`: the loader has already mapped everything,
//! and nothing left to do opens a path.

use landlock::{
    ABI, Access, AccessFs, CompatLevel, Compatible, RulesetAttr, RulesetCreatedAttr, RulesetStatus,
    path_beneath_rules,
};
use std::path::Path;

const ABI_TARGET: ABI = ABI::V5;

/// Applies the ruleset. Returns a line describing what the kernel enforced.
pub fn confine(framebuffer: &Path) -> Result<String, Box<dyn std::error::Error>> {
    // Read, write, and the device ioctls used to query the screen geometry.
    let device_access = AccessFs::from_all(ABI_TARGET);

    let mut ruleset = landlock::Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessFs::from_all(ABI_TARGET))?
        .create()?;

    // A missing framebuffer is not a reason to skip the sandbox: confine first
    // and let the open fail with a clear error afterwards.
    if framebuffer.exists() {
        ruleset = ruleset.add_rules(path_beneath_rules([framebuffer], device_access))?;
    }

    let status = ruleset.restrict_self()?;

    Ok(match status.ruleset {
        RulesetStatus::FullyEnforced => format!(
            "sandbox active: {} is the only reachable path",
            framebuffer.display()
        ),
        RulesetStatus::PartiallyEnforced => {
            "sandbox active (partial: this kernel supports an older Landlock ABI)".to_string()
        }
        RulesetStatus::NotEnforced => {
            "sandbox INACTIVE (this kernel has no Landlock support)".to_string()
        }
    })
}
