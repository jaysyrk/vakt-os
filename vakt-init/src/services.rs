//! A minimal service supervisor for vakt-init.
//!
//! This is deliberately small: it spawns background daemons, records their
//! PIDs, reaps them when they die, and restarts the ones that are supposed to
//! stay up.
//!
//! Reaping is done with `Child::try_wait()` on each tracked PID rather than a
//! blanket `waitpid(-1)`. As PID 1 we could reap everything, but the main
//! thread runs `vakt-panel` via `Command::status()`, which needs to collect
//! that child's exit code itself - a wildcard reaper in this thread would race
//! it and steal the result.

use std::fmt;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Where PID files, logs, and the status summary live.
const RUN_DIR: &str = "/run";
/// Summary file read by vakt-panel's Services page.
const STATUS_NAME: &str = "services.status";

/// How often the supervisor checks on its children.
const TICK: Duration = Duration::from_secs(2);
/// A service that dies this many times inside `CRASH_WINDOW` is given up on,
/// so a daemon that cannot start never becomes a spin loop.
const MAX_RESTARTS: u32 = 5;
const CRASH_WINDOW: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct Service {
    pub name: &'static str,
    pub program: &'static str,
    pub args: &'static [&'static str],
    /// Whether to restart the service when it exits.
    pub respawn: bool,
    pub description: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Running,
    Exited,
    /// Crashed too often to keep restarting.
    Failed,
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            State::Running => "running",
            State::Exited => "exited",
            State::Failed => "failed",
        })
    }
}

struct Managed {
    spec: Service,
    child: Option<Child>,
    state: State,
    restarts: u32,
    /// Start of the current crash-counting window.
    window_start: Instant,
    detail: String,
    run_dir: PathBuf,
}

impl Managed {
    fn pid_path(&self) -> PathBuf {
        self.run_dir.join(format!("{}.pid", self.spec.name))
    }

    fn log_path(&self) -> PathBuf {
        self.run_dir.join(format!("{}.log", self.spec.name))
    }

    fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(|c| c.id())
    }

    /// Spawns the daemon with its output captured to /run/<name>.log.
    fn start(&mut self) {
        let log_path = self.log_path();
        let stdout = File::create(&log_path).ok();
        let stderr = stdout.as_ref().and_then(|f| f.try_clone().ok());

        let mut cmd = Command::new(self.spec.program);
        cmd.args(self.spec.args);
        cmd.stdin(Stdio::null());
        match (stdout, stderr) {
            (Some(out), Some(err)) => {
                cmd.stdout(Stdio::from(out)).stderr(Stdio::from(err));
            }
            _ => {
                cmd.stdout(Stdio::null()).stderr(Stdio::null());
            }
        }

        match cmd.spawn() {
            Ok(child) => {
                let pid = child.id();
                let _ = std::fs::write(self.pid_path(), format!("{}\n", pid));
                println!(
                    "[Vakt-Init] Started service '{}' (pid {}).",
                    self.spec.name, pid
                );
                self.child = Some(child);
                self.state = State::Running;
                self.detail = format!("logging to {}", log_path.display());
            }
            Err(e) => {
                println!(
                    "[Vakt-Init] \x1b[1;33mFailed to start '{}': {}\x1b[0m",
                    self.spec.name, e
                );
                self.child = None;
                self.state = State::Failed;
                self.detail = e.to_string();
            }
        }
    }

    fn clear_pid_file(&self) {
        let _ = std::fs::remove_file(self.pid_path());
    }
}

pub struct Supervisor {
    services: Vec<Managed>,
    run_dir: PathBuf,
}

impl Supervisor {
    pub fn new(specs: &[Service]) -> Self {
        Supervisor::with_run_dir(specs, PathBuf::from(RUN_DIR))
    }

    fn with_run_dir(specs: &[Service], run_dir: PathBuf) -> Self {
        Supervisor {
            services: specs
                .iter()
                .map(|spec| Managed {
                    spec: spec.clone(),
                    child: None,
                    state: State::Exited,
                    restarts: 0,
                    window_start: Instant::now(),
                    detail: "not started".to_string(),
                    run_dir: run_dir.clone(),
                })
                .collect(),
            run_dir,
        }
    }

    /// Starts every service, then supervises them forever. Intended to be run
    /// on its own thread so the console stays free for the panel.
    pub fn run(mut self) -> ! {
        self.start_all();

        loop {
            std::thread::sleep(TICK);
            if self.reap() {
                self.write_status();
            }
        }
    }

    fn start_all(&mut self) {
        let _ = std::fs::create_dir_all(&self.run_dir);
        for service in &mut self.services {
            service.start();
        }
        self.write_status();
    }

    /// Collects exited children and restarts the ones that should stay up.
    /// Returns true when anything changed.
    fn reap(&mut self) -> bool {
        let mut changed = false;

        for service in &mut self.services {
            let Some(child) = service.child.as_mut() else {
                continue;
            };

            match child.try_wait() {
                // Still running.
                Ok(None) => {}
                Ok(Some(exit)) => {
                    println!(
                        "[Vakt-Init] Service '{}' exited ({}).",
                        service.spec.name, exit
                    );
                    service.child = None;
                    service.clear_pid_file();
                    service.state = State::Exited;
                    service.detail = format!("exited with {}", exit);
                    changed = true;

                    if !service.spec.respawn {
                        continue;
                    }

                    // Reset the counter once the service has been stable for a
                    // while, so occasional restarts never accumulate.
                    if service.window_start.elapsed() > CRASH_WINDOW {
                        service.restarts = 0;
                        service.window_start = Instant::now();
                    }

                    service.restarts += 1;
                    if service.restarts > MAX_RESTARTS {
                        println!(
                            "[Vakt-Init] \x1b[1;33mService '{}' crashed {} times; giving up.\x1b[0m",
                            service.spec.name, service.restarts
                        );
                        service.state = State::Failed;
                        service.detail =
                            format!("crashed {} times in under 60s", service.restarts);
                        continue;
                    }

                    println!(
                        "[Vakt-Init] Restarting '{}' (attempt {}).",
                        service.spec.name, service.restarts
                    );
                    service.start();
                }
                Err(e) => {
                    service.child = None;
                    service.clear_pid_file();
                    service.state = State::Failed;
                    service.detail = format!("wait failed: {}", e);
                    changed = true;
                }
            }
        }

        changed
    }

    /// Writes a one-line-per-service summary for the panel to display.
    fn write_status(&self) {
        let mut body = String::new();
        for service in &self.services {
            let pid = service.pid().map(|p| p.to_string()).unwrap_or_default();
            body.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\n",
                service.spec.name,
                service.state,
                pid,
                service.restarts,
                service.spec.description
            ));
        }

        let status_path = self.run_dir.join(STATUS_NAME);
        let _ = std::fs::create_dir_all(&self.run_dir);
        let tmp = status_path.with_extension("status.tmp");
        if let Ok(mut file) = File::create(&tmp) {
            if file.write_all(body.as_bytes()).is_ok() {
                let _ = std::fs::rename(&tmp, &status_path);
                return;
            }
        }
        let _ = std::fs::remove_file(&tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique scratch directory per test, so the run dirs never collide.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "vakt-init-test-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Drives reap() until the service leaves the Running state, so the tests
    /// do not depend on how fast the child happens to exit.
    fn reap_until_settled(sup: &mut Supervisor, max_attempts: u32) {
        for _ in 0..max_attempts {
            sup.reap();
            if sup.services[0].state != State::Running {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    const ONESHOT: &[Service] = &[Service {
        name: "oneshot",
        program: "/bin/sh",
        args: &["-c", "exit 3"],
        respawn: false,
        description: "exits immediately, never restarted",
    }];

    const CRASHER: &[Service] = &[Service {
        name: "crasher",
        program: "/bin/sh",
        args: &["-c", "exit 1"],
        respawn: true,
        description: "always fails to stay up",
    }];

    const LONG_RUNNING: &[Service] = &[Service {
        name: "sleeper",
        program: "/bin/sh",
        args: &["-c", "sleep 30"],
        respawn: true,
        description: "stays up",
    }];

    #[test]
    fn running_service_writes_a_pid_file_and_is_reported_running() {
        let dir = scratch("running");
        let mut sup = Supervisor::with_run_dir(LONG_RUNNING, dir.clone());
        sup.start_all();

        let pid_file = dir.join("sleeper.pid");
        assert!(pid_file.exists(), "expected a pid file at {:?}", pid_file);
        let pid: u32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .expect("pid file should contain a number");
        assert_eq!(Some(pid), sup.services[0].pid());

        // A healthy service produces no state change.
        assert!(!sup.reap(), "reap should report no change for a live service");
        assert_eq!(sup.services[0].state, State::Running);

        let status = std::fs::read_to_string(dir.join(STATUS_NAME)).unwrap();
        assert!(status.starts_with("sleeper\trunning\t"), "got: {}", status);

        // Do not leave the child behind.
        if let Some(child) = sup.services[0].child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    #[test]
    fn exited_service_without_respawn_is_reaped_and_left_alone() {
        let dir = scratch("oneshot");
        let mut sup = Supervisor::with_run_dir(ONESHOT, dir.clone());
        sup.start_all();

        reap_until_settled(&mut sup, 40);

        assert_eq!(sup.services[0].state, State::Exited);
        assert_eq!(sup.services[0].restarts, 0, "must not be restarted");
        assert!(
            !dir.join("oneshot.pid").exists(),
            "pid file should be removed once the service exits"
        );
    }

    #[test]
    fn crash_looping_service_is_restarted_then_given_up_on() {
        let dir = scratch("crasher");
        let mut sup = Supervisor::with_run_dir(CRASHER, dir.clone());
        sup.start_all();

        // Each reap collects the corpse and starts a replacement, until the
        // restart budget inside CRASH_WINDOW is exhausted.
        for _ in 0..200 {
            if sup.services[0].state == State::Failed {
                break;
            }
            sup.reap();
            std::thread::sleep(Duration::from_millis(20));
        }

        assert_eq!(
            sup.services[0].state,
            State::Failed,
            "a service that cannot stay up must eventually be abandoned"
        );
        assert!(
            sup.services[0].restarts > MAX_RESTARTS,
            "expected more than {} restart attempts, saw {}",
            MAX_RESTARTS,
            sup.services[0].restarts
        );
        assert!(
            sup.services[0].child.is_none(),
            "no child should be left running after giving up"
        );
    }

    #[test]
    fn unstartable_program_is_marked_failed_not_running() {
        let dir = scratch("missing");
        const MISSING: &[Service] = &[Service {
            name: "missing",
            program: "/nonexistent/vakt-does-not-exist",
            args: &[],
            respawn: true,
            description: "binary is not installed",
        }];

        let mut sup = Supervisor::with_run_dir(MISSING, dir.clone());
        sup.start_all();

        assert_eq!(sup.services[0].state, State::Failed);
        let status = std::fs::read_to_string(dir.join(STATUS_NAME)).unwrap();
        assert!(status.contains("failed"), "got: {}", status);
    }

    /// The reason this supervisor reaps per-PID instead of using waitpid(-1):
    /// vakt-init's main thread collects vakt-panel's exit code with
    /// Command::status() while the supervisor runs alongside it. A wildcard
    /// reaper would race that call and swallow the result.
    #[test]
    fn supervisor_thread_does_not_steal_foreground_exit_codes() {
        let dir = scratch("coexist");
        std::thread::spawn(move || {
            Supervisor::with_run_dir(LONG_RUNNING, dir).run();
        });

        // Span more than one supervisor TICK so its reap pass definitely
        // overlaps the foreground children below.
        let deadline = Instant::now() + TICK * 2;
        let mut rounds = 0;
        while Instant::now() < deadline {
            let status = Command::new("/bin/sh")
                .args(["-c", "exit 42"])
                .status()
                .expect("foreground child should be waitable");
            assert_eq!(
                status.code(),
                Some(42),
                "supervisor stole the foreground child's exit status"
            );
            rounds += 1;
            std::thread::sleep(Duration::from_millis(200));
        }
        assert!(rounds > 1, "test did not run long enough to be meaningful");
    }

    #[test]
    fn service_output_is_captured_to_a_log_file() {
        let dir = scratch("logging");
        const TALKER: &[Service] = &[Service {
            name: "talker",
            program: "/bin/sh",
            args: &["-c", "echo hello-from-service"],
            respawn: false,
            description: "writes to stdout",
        }];

        let mut sup = Supervisor::with_run_dir(TALKER, dir.clone());
        sup.start_all();
        reap_until_settled(&mut sup, 40);

        let log = std::fs::read_to_string(dir.join("talker.log")).unwrap();
        assert!(log.contains("hello-from-service"), "got: {}", log);
    }
}

/// The services vakt-init brings up at boot.
pub const DEFAULT_SERVICES: &[Service] = &[
    Service {
        name: "vakt-net",
        program: "vakt-net",
        args: &[],
        respawn: true,
        description: "Wi-Fi and DHCP negotiation",
    },
    Service {
        name: "vakt-ids",
        program: "vakt-ids",
        args: &["--watch", "/persistent"],
        respawn: true,
        description: "Filesystem integrity monitor",
    },
];
