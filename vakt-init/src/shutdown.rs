//! Bringing the system down without losing what was written to it.
//!
//! PID 1 has no default signal dispositions - the kernel does not apply them to
//! init - so a SIGTERM that would end any other process is silently discarded
//! here unless it is handled. That is why the appliance had no way to shut down
//! cleanly before: `poweroff` sends a signal to PID 1 and waits for it to do
//! something about it.
//!
//! Signals are collected through a `signalfd` on a dedicated thread rather than
//! through a handler. A handler would have to hand the work to another thread
//! anyway, since almost nothing that shutting down involves is safe to call
//! from signal context; reading them off a file descriptor makes that explicit
//! and removes the async-signal-safety question entirely.

use crate::mount;
use crate::services::Control;
use nix::sys::reboot::{RebootMode, reboot, set_cad_enabled};
use nix::sys::signal::{SigSet, Signal, kill};
use nix::sys::signalfd::SignalFd;
use nix::unistd::{Pid, sync};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;

/// How long the supervisor gets to bring every daemon down before the system
/// goes ahead without them.
const STOP_TIMEOUT: Duration = Duration::from_secs(15);

/// The signals that mean "go down". SIGPWR is the roadmap's requirement;
/// SIGUSR1 and SIGUSR2 are here because busybox's `halt` and `poweroff` send
/// those to PID 1, and an appliance where `poweroff` does nothing is worse than
/// one with no shutdown at all.
const SHUTDOWN_SIGNALS: &[Signal] = &[
    Signal::SIGINT,
    Signal::SIGTERM,
    Signal::SIGPWR,
    Signal::SIGUSR1,
    Signal::SIGUSR2,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    PowerOff,
    Reboot,
    Halt,
}

impl Action {
    /// What a signal delivered to PID 1 means.
    ///
    /// SIGINT is what the kernel sends init for ctrl-alt-del once
    /// [`disable_ctrl_alt_del`] has taken the reboot out of the kernel's hands,
    /// so it means the same thing as pressing reset used to.
    pub fn from_signal(signal: Signal) -> Option<Action> {
        match signal {
            Signal::SIGINT | Signal::SIGTERM => Some(Action::Reboot),
            Signal::SIGPWR | Signal::SIGUSR2 => Some(Action::PowerOff),
            Signal::SIGUSR1 => Some(Action::Halt),
            _ => None,
        }
    }

    /// Parses the verb from a `SHUTDOWN=` request on the init socket.
    pub fn from_verb(verb: &str) -> Option<Action> {
        match verb {
            "poweroff" | "power-off" | "off" => Some(Action::PowerOff),
            "reboot" | "restart" => Some(Action::Reboot),
            "halt" => Some(Action::Halt),
            _ => None,
        }
    }

    fn mode(self) -> RebootMode {
        match self {
            Action::PowerOff => RebootMode::RB_POWER_OFF,
            Action::Reboot => RebootMode::RB_AUTOBOOT,
            Action::Halt => RebootMode::RB_HALT_SYSTEM,
        }
    }

    fn verb(self) -> &'static str {
        match self {
            Action::PowerOff => "Powering off",
            Action::Reboot => "Rebooting",
            Action::Halt => "Halting",
        }
    }
}

/// The panel (or the recovery shell) currently occupying the console, so
/// shutdown can end it rather than leaving it holding the terminal.
static FOREGROUND: AtomicI32 = AtomicI32::new(0);
static UNDER_WAY: AtomicBool = AtomicBool::new(false);

pub fn set_foreground(pid: u32) {
    FOREGROUND.store(pid as i32, Ordering::SeqCst);
}

pub fn clear_foreground() {
    FOREGROUND.store(0, Ordering::SeqCst);
}

/// Whether a shutdown has already begun, so the main loop stops respawning the
/// panel underneath it.
pub fn under_way() -> bool {
    UNDER_WAY.load(Ordering::SeqCst)
}

/// Blocks the shutdown signals process-wide.
///
/// Must be called before any thread is spawned: the mask is inherited, and a
/// thread that misses it could take a shutdown signal with no handler installed
/// and terminate the whole system by default action.
pub fn block_signals() -> nix::Result<SigSet> {
    let mut mask = SigSet::empty();
    for signal in SHUTDOWN_SIGNALS {
        mask.add(*signal);
    }
    mask.thread_block()?;
    Ok(mask)
}

/// Stops the kernel from rebooting on ctrl-alt-del by itself, so the key
/// combination arrives here as SIGINT and goes through the ordered sequence
/// instead of cutting power to a mounted disk.
pub fn disable_ctrl_alt_del() {
    if let Err(e) = set_cad_enabled(false) {
        println!(
            "[Vakt-Init] \x1b[1;33mCould not take over ctrl-alt-del: {}\x1b[0m",
            e
        );
    }
}

/// Waits for a shutdown signal forever, running the sequence when one arrives.
///
/// Runs on its own thread for the lifetime of the system.
pub fn watch_signals(mask: SigSet, control: std::sync::Arc<Control>) {
    let signals = match SignalFd::new(&mask) {
        Ok(fd) => fd,
        Err(e) => {
            println!(
                "[Vakt-Init] \x1b[1;31mNo signalfd ({}); shutdown signals will be ignored.\x1b[0m",
                e
            );
            return;
        }
    };

    loop {
        match signals.read_signal() {
            Ok(Some(info)) => {
                let Ok(signal) = Signal::try_from(info.ssi_signo as i32) else {
                    continue;
                };
                let Some(action) = Action::from_signal(signal) else {
                    continue;
                };
                println!("[Vakt-Init] Received {}.", signal.as_str());
                execute(action, &control);
            }
            Ok(None) => continue,
            Err(nix::errno::Errno::EINTR) => continue,
            Err(e) => {
                println!("[Vakt-Init] \x1b[1;33mSignal read failed: {}\x1b[0m", e);
                return;
            }
        }
    }
}

/// Takes the system down. Never returns.
///
/// The order is the whole point: stop writing before flushing, flush before
/// unmounting, unmount before cutting power. Skipping any step is how a
/// journalled filesystem ends up needing a recovery pass on the next boot.
pub fn execute(action: Action, control: &Control) -> ! {
    if UNDER_WAY.swap(true, Ordering::SeqCst) {
        // A second request while the first is in progress: let the first finish
        // rather than racing it through the sequence.
        park();
    }

    println!("\n[Vakt-Init] {} - stopping the system.", action.verb());

    // 1. End the console session, so nothing is still writing to the disk or
    //    the terminal while everything else goes down.
    let foreground = FOREGROUND.swap(0, Ordering::SeqCst);
    if foreground > 0 {
        let _ = kill(Pid::from_raw(foreground), Signal::SIGTERM);
    }

    // 2. Ask the supervisor for an orderly stop of every daemon and wait for it
    //    to confirm they are gone.
    control.request_stop();
    if !control.wait_until_stopped(STOP_TIMEOUT) {
        println!("[Vakt-Init] \x1b[1;33mServices did not all stop in time; continuing.\x1b[0m");
    }

    // 3. Flush the page cache to the disk.
    println!("[Vakt-Init] Flushing disks...");
    sync();

    // 4. Unmount the persistent volume cleanly so the next boot does not have
    //    to replay the journal.
    mount::unmount_persistent();

    // 5. And go.
    println!("[Vakt-Init] {}.\n", action.verb());
    sync();
    let _ = reboot(action.mode());

    // reboot(2) only returns on failure, and PID 1 exiting would panic the
    // kernel, so there is nowhere to go from here but to stop.
    println!("[Vakt-Init] \x1b[1;31mThe kernel refused to shut down. System halted.\x1b[0m");
    park();
}

fn park() -> ! {
    loop {
        std::thread::park();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signals_map_to_the_expected_actions() {
        assert_eq!(Action::from_signal(Signal::SIGPWR), Some(Action::PowerOff));
        assert_eq!(Action::from_signal(Signal::SIGUSR2), Some(Action::PowerOff));
        assert_eq!(Action::from_signal(Signal::SIGTERM), Some(Action::Reboot));
        assert_eq!(Action::from_signal(Signal::SIGINT), Some(Action::Reboot));
        assert_eq!(Action::from_signal(Signal::SIGUSR1), Some(Action::Halt));
        assert_eq!(Action::from_signal(Signal::SIGHUP), None);
    }

    #[test]
    fn socket_verbs_map_to_the_expected_actions() {
        assert_eq!(Action::from_verb("poweroff"), Some(Action::PowerOff));
        assert_eq!(Action::from_verb("reboot"), Some(Action::Reboot));
        assert_eq!(Action::from_verb("halt"), Some(Action::Halt));
        assert_eq!(Action::from_verb("format-the-disk"), None);
    }

    /// Every signal the shutdown thread is asked to watch has to mean
    /// something, or it would be blocked process-wide and then dropped.
    #[test]
    fn every_watched_signal_is_actionable() {
        for signal in SHUTDOWN_SIGNALS {
            assert!(
                Action::from_signal(*signal).is_some(),
                "{} is blocked but has no action",
                signal.as_str()
            );
        }
    }
}
