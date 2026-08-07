//! Decides whether the slot vakt-init just booted into should be trusted, and
//! rolls back to slot A if a `vakt-update`-staged slot B has burned through
//! its boot budget without reaching readiness.
//!
//! UNVALIDATED ON REAL BOOT HARDWARE as of this writing - see
//! docs/OS_UPDATES.md. The decision logic in [`next`] below is unit tested
//! and does not touch a filesystem or GRUB at all, which is the part that
//! can be verified here. What can't be verified here: whether GRUB actually
//! reads `/persistent/etc/vakt/bootenv` the way [`envblock`] assumes, and
//! whether a slot-B initramfs actually carries the right `/etc/vakt-slot`
//! value - both need a real reboot into a real slot B to confirm.

use crate::envblock;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

const SLOT_FILE: &str = "/etc/vakt-slot";
const STATE_PATH: &str = "/persistent/etc/vakt-update-state.json";
const BOOTENV_PATH: &str = "/persistent/etc/vakt/bootenv";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct UpdateState {
    tries_left: u32,
}

/// What [`next`] decided should happen to the on-disk state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// Slot A, or no pending update: nothing to do.
    NotApplicable,
    /// Slot B still has attempts left; save the new count and keep booting.
    Decrement(u32),
    /// Slot B ran out of attempts without confirming; fall back to slot A.
    RollBack,
}

/// Pure decision logic, kept separate from real files so it is testable
/// without mounting anything: given the slot currently running and however
/// many boot attempts slot B has left, what should this boot do.
fn next(slot: &str, tries_left: Option<u32>) -> Outcome {
    if slot != "B" {
        return Outcome::NotApplicable;
    }
    match tries_left {
        None => Outcome::NotApplicable,
        Some(0) => Outcome::RollBack,
        Some(n) => Outcome::Decrement(n - 1),
    }
}

fn current_slot() -> String {
    current_slot_from(SLOT_FILE)
}

/// Split out from [`current_slot`] so the missing-file/empty-file fallback is
/// testable without touching `/etc/vakt-slot`.
fn current_slot_from(path: &str) -> String {
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "A".to_string())
}

fn load_tries_left() -> Option<u32> {
    let text = fs::read_to_string(STATE_PATH).ok()?;
    let state: UpdateState = serde_json::from_str(&text).ok()?;
    Some(state.tries_left)
}

fn save_tries_left(tries_left: u32) {
    if let Ok(text) = serde_json::to_string(&UpdateState { tries_left }) {
        let _ = fs::write(STATE_PATH, text);
    }
}

/// Called once, early in boot, right after `/persistent` is mounted and
/// before any service starts. If this rolls back, it does not return - it
/// writes `vakt_active=A` and reboots straight into slot A. Nothing has
/// started yet at this point, so there is no supervisor to stop first; a
/// direct `reboot(2)` is the right tool, not the graceful shutdown sequence
/// that exists for stopping a running system.
pub fn check_and_handle(persistent_mounted: bool) {
    if !persistent_mounted {
        return;
    }

    let slot = current_slot();

    // Diagnostic only: if GRUB's own idea of the active slot disagrees with
    // what this boot's initramfs claims to be, that is a sign something
    // about the update/rollback wiring is wrong, and worth seeing in the
    // boot log even though nothing here acts on it directly.
    if let Ok(entries) = envblock::read(Path::new(BOOTENV_PATH)) {
        if let Some(active) = entries.get("vakt_active") {
            if active != &slot {
                println!(
                    "[Vakt-Init] \x1b[1;33mGRUB's active slot ('{}') does not match \
                     this boot's slot ('{}').\x1b[0m",
                    active, slot
                );
            }
        }
    }

    match next(&slot, load_tries_left()) {
        Outcome::NotApplicable => {}
        Outcome::Decrement(remaining) => save_tries_left(remaining),
        Outcome::RollBack => {
            println!(
                "[Vakt-Init] \x1b[1;31mSlot B did not confirm within its boot budget; \
                 rolling back to slot A.\x1b[0m"
            );
            if let Err(e) = envblock::write(Path::new(BOOTENV_PATH), &[("vakt_active", "A")]) {
                println!(
                    "[Vakt-Init] \x1b[1;31mCould not write the rollback GRUB environment ({}); \
                     rebooting anyway - GRUB may boot slot B again.\x1b[0m",
                    e
                );
            }
            let _ = fs::remove_file(STATE_PATH);
            let _ = nix::sys::reboot::reboot(nix::sys::reboot::RebootMode::RB_AUTOBOOT);
            // reboot(2) only returns on failure; returning from here would let
            // boot continue on a slot this function just decided not to trust.
            loop {
                std::thread::park();
            }
        }
    }
}

/// Called once boot reaches the point it considers itself successful (the
/// same point that decides whether to draw the panel). If this is slot B
/// with a pending update, marks it accepted - future boots into this slot
/// never roll it back again, until the next update replaces it.
pub fn confirm() {
    if current_slot() == "B" && Path::new(STATE_PATH).exists() {
        let _ = fs::remove_file(STATE_PATH);
        println!("[Vakt-Init] Slot B confirmed good.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_a_is_never_touched_by_the_update_machinery() {
        assert_eq!(next("A", None), Outcome::NotApplicable);
        assert_eq!(next("A", Some(0)), Outcome::NotApplicable);
        assert_eq!(next("A", Some(3)), Outcome::NotApplicable);
    }

    #[test]
    fn slot_b_with_no_state_file_is_left_alone() {
        // A confirmed update deletes its state file; a later reboot into that
        // same accepted slot B must not be treated as still-pending.
        assert_eq!(next("B", None), Outcome::NotApplicable);
    }

    #[test]
    fn slot_b_with_attempts_remaining_decrements() {
        assert_eq!(next("B", Some(3)), Outcome::Decrement(2));
        assert_eq!(next("B", Some(1)), Outcome::Decrement(0));
    }

    #[test]
    fn slot_b_at_zero_attempts_rolls_back() {
        assert_eq!(next("B", Some(0)), Outcome::RollBack);
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("vakt-init-slot-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn current_slot_defaults_to_a_when_the_file_is_missing() {
        // Ambiguity must fall back to the safe direction: if this process
        // somehow can't tell which slot it's running, assume the one that is
        // never written to by an update.
        let dir = scratch("missing");
        let path = dir.join("does-not-exist");

        assert_eq!(current_slot_from(path.to_str().unwrap()), "A");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn current_slot_defaults_to_a_when_the_file_is_empty() {
        let dir = scratch("empty");
        let path = dir.join("vakt-slot");
        std::fs::write(&path, "").unwrap();

        assert_eq!(current_slot_from(path.to_str().unwrap()), "A");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn current_slot_reads_and_trims_slot_b() {
        let dir = scratch("b");
        let path = dir.join("vakt-slot");
        std::fs::write(&path, "B\n").unwrap();

        assert_eq!(current_slot_from(path.to_str().unwrap()), "B");
        std::fs::remove_dir_all(&dir).ok();
    }
}
