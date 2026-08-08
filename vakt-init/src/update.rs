//! Decides whether the slot vakt-init just booted into should be trusted, and
//! rolls back to slot A if a `vakt-update`-staged slot B has burned through
//! its boot budget without reaching readiness. Unvalidated on real boot
//! hardware - see docs/OS_UPDATES.md.

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    NotApplicable,
    Decrement(u32),
    RollBack,
}

/// Pure decision logic, testable without touching the filesystem.
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
/// before any service starts. A rollback does not return - reboots straight
/// into slot A.
pub fn check_and_handle(persistent_mounted: bool) {
    if !persistent_mounted {
        return;
    }

    let slot = current_slot();

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

            // Same discipline as shutdown::execute: sync, unmount, sync again,
            // then reboot. reboot(2) does not flush dirty pages on its own -
            // skipping this can leave the bootenv write or the state-file
            // deletion above never actually reaching disk, in which case the
            // very next boot reads the stale state and rolls back again.
            nix::unistd::sync();
            crate::mount::unmount_persistent();
            nix::unistd::sync();

            let _ = nix::sys::reboot::reboot(nix::sys::reboot::RebootMode::RB_AUTOBOOT);
            loop {
                std::thread::park();
            }
        }
    }
}

/// Called once boot considers itself successful. Marks a pending slot-B
/// update accepted.
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
