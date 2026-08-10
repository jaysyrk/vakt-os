//! A minimal service supervisor for vakt-init.
//!
//! This is deliberately small: it spawns background daemons, records their
//! PIDs, captures their output into a size-capped log, reaps them when they
//! die, restarts the ones that are supposed to stay up, and stops all of them
//! in order when the system goes down.
//!
//! Reaping is done with `Child::try_wait()` on each tracked PID rather than a
//! blanket `waitpid(-1)`. As PID 1 we could reap everything, but the main
//! thread runs `vakt-panel` via `Child::wait()`, which needs to collect that
//! child's exit code itself - a wildcard reaper in this thread would race it
//! and steal the result.

use crate::logfile::{self, BoundedLog};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::File;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Where PID files, logs, and the status summary live.
const RUN_DIR: &str = "/run";
/// Summary file read by vakt-panel's Services page.
const STATUS_NAME: &str = "services.status";

/// How often the supervisor checks on its children when nothing wakes it.
const TICK: Duration = Duration::from_secs(2);
/// A service that dies this many times inside `CRASH_WINDOW` is given up on,
/// so a daemon that cannot start never becomes a spin loop.
const MAX_RESTARTS: u32 = 5;
const CRASH_WINDOW: Duration = Duration::from_secs(60);
/// How long a daemon gets to exit on SIGTERM before it is killed outright.
const STOP_GRACE: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct Service {
    pub name: &'static str,
    pub program: &'static str,
    pub args: &'static [&'static str],
    /// Whether to restart the service when it exits.
    pub respawn: bool,
    /// Whether this service reports readiness over `/run/init.sock`. Boot waits
    /// for the ones that do, and does not wait for the ones that do not.
    pub notifies: bool,
    pub description: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Running,
    Exited,
    /// Crashed too often to keep restarting.
    Failed,
    /// Stopped deliberately during shutdown.
    Stopped,
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            State::Running => "running",
            State::Exited => "exited",
            State::Failed => "failed",
            State::Stopped => "stopped",
        })
    }
}

/// What the supervisor is doing, and what it has been asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Running,
    /// Shutdown has been requested; the supervisor is bringing daemons down.
    Stopping,
    /// Every supervised daemon has exited.
    Stopped,
}

#[derive(Default)]
struct Shared {
    phase: Option<Phase>,
    /// Every service that is supposed to report readiness.
    expected: BTreeSet<String>,
    /// Those of them boot is still waiting on. A service that fails outright is
    /// dropped from here without ever joining `ready`, which is what stops boot
    /// waiting out the full timeout for a daemon already known to be dead.
    awaiting: BTreeSet<String>,
    /// Live child PIDs, so a notification can be attributed to a service.
    pids: BTreeMap<u32, String>,
    ready: BTreeSet<String>,
    /// The most recent `STATUS=` line each service sent.
    status: BTreeMap<String, String>,
    /// Bumped by every change worth republishing.
    ///
    /// Readiness arrives on the socket thread, and the supervisor is what
    /// writes the status file. Without this the supervisor only rewrites when
    /// a child exits, so on an appliance where nothing ever crashes the file
    /// keeps whatever `start_all` wrote - every service `waiting`, forever,
    /// on a system that is in fact fully up.
    revision: u64,
}

/// The supervisor's shared state.
///
/// Three threads touch it: the supervisor itself, the thread reading the
/// readiness socket, and whichever thread is running the shutdown sequence.
/// One mutex and one condition variable are enough for all of it, and keeping
/// it to one of each is what makes the lock ordering trivially correct.
pub struct Control {
    shared: Mutex<Shared>,
    changed: Condvar,
}

impl Control {
    pub fn new(specs: &[Service]) -> Arc<Control> {
        let expected: BTreeSet<String> = specs
            .iter()
            .filter(|s| s.notifies)
            .map(|s| s.name.to_string())
            .collect();

        Arc::new(Control {
            shared: Mutex::new(Shared {
                phase: Some(Phase::Running),
                awaiting: expected.clone(),
                expected,
                ..Shared::default()
            }),
            changed: Condvar::new(),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        // A panicking supervisor thread must not wedge shutdown, so a poisoned
        // lock is taken anyway: the state behind it is plain collections that
        // cannot be left in a half-updated shape by a panic between statements.
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Records a notification and returns the service it was attributed to.
    ///
    /// `pid` comes from the kernel and is authoritative. `name` is the sender's
    /// own claim and is only believed when there is no pid to check it against.
    pub fn note_ready(
        &self,
        pid: Option<u32>,
        name: Option<&str>,
        status: Option<&str>,
    ) -> Option<String> {
        let mut shared = self.lock();

        let service = match pid.and_then(|p| shared.pids.get(&p).cloned()) {
            Some(known) => known,
            None => name?.to_string(),
        };

        shared.awaiting.remove(&service);
        shared.ready.insert(service.clone());
        if let Some(status) = status {
            shared.status.insert(service.clone(), status.to_string());
        }
        shared.revision += 1;
        self.changed.notify_all();
        Some(service)
    }

    fn register(&self, pid: u32, name: &str) {
        let mut shared = self.lock();
        shared.pids.insert(pid, name.to_string());
    }

    /// Forgets a service's readiness when it exits: a daemon that has been
    /// restarted has not reported anything yet.
    fn deregister(&self, pid: Option<u32>, name: &str) {
        let mut shared = self.lock();
        if let Some(pid) = pid {
            shared.pids.remove(&pid);
        }
        shared.ready.remove(name);
        shared.revision += 1;
        self.changed.notify_all();
    }

    /// Stops boot from waiting on a service that is never going to report.
    fn settle(&self, name: &str) {
        let mut shared = self.lock();
        shared.awaiting.remove(name);
        shared.revision += 1;
        self.changed.notify_all();
    }

    /// A counter the supervisor compares against to know the published status
    /// file is out of date.
    fn revision(&self) -> u64 {
        self.lock().revision
    }

    fn snapshot(&self, name: &str) -> (bool, Option<String>) {
        let shared = self.lock();
        (
            shared.ready.contains(name),
            shared.status.get(name).cloned(),
        )
    }

    /// Blocks until nothing is left to wait for, or `timeout` expires.
    ///
    /// Returns whether every notifying service actually reported ready, which
    /// is not the same as the wait being over: a service that failed to start
    /// stops boot waiting for it, but it is not ready and never will be.
    pub fn wait_until_ready(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut shared = self.lock();

        while !shared.awaiting.is_empty() {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            let (guard, _) = self
                .changed
                .wait_timeout(shared, remaining)
                .unwrap_or_else(|e| e.into_inner());
            shared = guard;
        }
        shared.expected.is_subset(&shared.ready)
    }

    /// Asks the supervisor to bring everything down and stop respawning.
    pub fn request_stop(&self) {
        let mut shared = self.lock();
        if shared.phase == Some(Phase::Running) {
            shared.phase = Some(Phase::Stopping);
        }
        self.changed.notify_all();
    }

    fn mark_stopped(&self) {
        let mut shared = self.lock();
        shared.phase = Some(Phase::Stopped);
        self.changed.notify_all();
    }

    /// Blocks until the supervisor reports everything down, or `timeout`
    /// expires. Returns whether it got there.
    pub fn wait_until_stopped(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut shared = self.lock();

        while shared.phase != Some(Phase::Stopped) {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            let (guard, _) = self
                .changed
                .wait_timeout(shared, remaining)
                .unwrap_or_else(|e| e.into_inner());
            shared = guard;
        }
        true
    }

    /// The supervisor's sleep between passes. Returns early the moment a
    /// shutdown is requested, so going down does not wait out a whole tick.
    fn wait_tick(&self, tick: Duration) -> Phase {
        let shared = self.lock();
        if shared.phase == Some(Phase::Stopping) {
            return Phase::Stopping;
        }
        let (shared, _) = self
            .changed
            .wait_timeout(shared, tick)
            .unwrap_or_else(|e| e.into_inner());
        shared.phase.unwrap_or(Phase::Running)
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

    /// Spawns the daemon with its output captured to a capped /run/<name>.log.
    ///
    /// Both stdout and stderr are pointed at one pipe we create ourselves, and
    /// a thread drains it. Handing the daemon a log file directly would be
    /// simpler, but there would then be nothing between it and the tmpfs to
    /// enforce a size limit.
    fn start(&mut self, control: &Control) {
        let log_path = self.log_path();

        let piped =
            nix::unistd::pipe()
                .map_err(std::io::Error::from)
                .and_then(|(reader, writer)| {
                    let duplicate = writer.try_clone()?;
                    let log = BoundedLog::create(&log_path, logfile::BUDGET)?;
                    Ok((reader, writer, duplicate, log))
                });

        let mut cmd = Command::new(self.spec.program);
        cmd.args(self.spec.args);
        cmd.stdin(Stdio::null());

        // A signal mask survives execve, and PID 1 blocks the shutdown signals
        // for its signalfd. Left inherited, every daemon has SIGTERM blocked
        // and is killed after the grace period instead of stopping on request.
        //
        // SAFETY: between fork and exec, where only async-signal-safe calls are
        // allowed. sigprocmask qualifies and nothing here allocates.
        unsafe {
            cmd.pre_exec(|| {
                nix::sys::signal::SigSet::empty()
                    .thread_set_mask()
                    .map_err(std::io::Error::from)
            });
        }

        let reader_and_log = match piped {
            Ok((reader, writer, duplicate, log)) => {
                cmd.stdout(Stdio::from(writer))
                    .stderr(Stdio::from(duplicate));
                Some((reader, log))
            }
            Err(e) => {
                println!(
                    "[Vakt-Init] \x1b[1;33mNo log capture for '{}': {}\x1b[0m",
                    self.spec.name, e
                );
                cmd.stdout(Stdio::null()).stderr(Stdio::null());
                None
            }
        };

        match cmd.spawn() {
            Ok(child) => {
                let pid = child.id();
                let _ = std::fs::write(self.pid_path(), format!("{}\n", pid));
                println!(
                    "[Vakt-Init] Started service '{}' (pid {}).",
                    self.spec.name, pid
                );
                control.register(pid, self.spec.name);
                self.child = Some(child);
                self.state = State::Running;
                self.detail = format!("logging to {}", log_path.display());

                // Only now that the child owns the write end will the drain
                // thread see EOF when the child exits.
                if let Some((reader, log)) = reader_and_log {
                    std::thread::spawn(move || logfile::drain(File::from(reader), log));
                }
            }
            Err(e) => {
                println!(
                    "[Vakt-Init] \x1b[1;33mFailed to start '{}': {}\x1b[0m",
                    self.spec.name, e
                );
                self.child = None;
                self.state = State::Failed;
                self.detail = e.to_string();
                control.settle(self.spec.name);
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
    control: Arc<Control>,
}

impl Supervisor {
    pub fn new(specs: &[Service], control: Arc<Control>) -> Self {
        Supervisor::with_run_dir(specs, control, PathBuf::from(RUN_DIR))
    }

    fn with_run_dir(specs: &[Service], control: Arc<Control>, run_dir: PathBuf) -> Self {
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
            control,
        }
    }

    /// Starts every service and supervises them until shutdown is requested,
    /// then stops them and returns. Intended to be run on its own thread so
    /// the console stays free for the panel.
    pub fn run(mut self) {
        self.start_all();
        let mut published = self.control.revision();

        while self.control.wait_tick(TICK) != Phase::Stopping {
            // Readiness and STATUS lines land on the socket thread, so a
            // service coming up is not something `reap` can see. Both have to
            // republish or the file describes the moment of launch and never
            // anything after it.
            let revision = self.control.revision();
            if self.reap() | (revision != published) {
                self.write_status();
                published = revision;
            }
        }

        self.stop_all();
        self.write_status();
        self.control.mark_stopped();
    }

    fn start_all(&mut self) {
        let _ = std::fs::create_dir_all(&self.run_dir);
        let control = Arc::clone(&self.control);
        for service in &mut self.services {
            service.start(&control);
        }
        self.write_status();
    }

    /// Collects exited children and restarts the ones that should stay up.
    /// Returns true when anything changed.
    fn reap(&mut self) -> bool {
        let mut changed = false;
        let control = Arc::clone(&self.control);

        for service in &mut self.services {
            let Some(child) = service.child.as_mut() else {
                continue;
            };

            match child.try_wait() {
                Ok(None) => {}
                Ok(Some(exit)) => {
                    let pid = Some(child.id());
                    println!(
                        "[Vakt-Init] Service '{}' exited ({}).",
                        service.spec.name, exit
                    );
                    service.child = None;
                    service.clear_pid_file();
                    service.state = State::Exited;
                    service.detail = format!("exited with {}", exit);
                    control.deregister(pid, service.spec.name);
                    changed = true;

                    if !service.spec.respawn {
                        control.settle(service.spec.name);
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
                        service.detail = format!("crashed {} times in under 60s", service.restarts);
                        control.settle(service.spec.name);
                        continue;
                    }

                    println!(
                        "[Vakt-Init] Restarting '{}' (attempt {}).",
                        service.spec.name, service.restarts
                    );
                    service.start(&control);
                }
                Err(e) => {
                    let pid = Some(child.id());
                    service.child = None;
                    service.clear_pid_file();
                    service.state = State::Failed;
                    service.detail = format!("wait failed: {}", e);
                    control.deregister(pid, service.spec.name);
                    control.settle(service.spec.name);
                    changed = true;
                }
            }
        }

        changed
    }

    /// Asks every daemon to exit, then makes sure it did.
    ///
    /// SIGTERM goes to all of them at once rather than one at a time, so the
    /// grace period is spent once for the whole system instead of once per
    /// service. Anything still alive at the end of it is not going to exit on
    /// its own and gets SIGKILL.
    fn stop_all(&mut self) {
        let live: Vec<&str> = self
            .services
            .iter()
            .filter(|s| s.child.is_some())
            .map(|s| s.spec.name)
            .collect();

        if live.is_empty() {
            return;
        }
        println!("[Vakt-Init] Stopping services: {}.", live.join(", "));

        for service in &mut self.services {
            if let Some(child) = service.child.as_ref() {
                let _ = kill(Pid::from_raw(child.id() as i32), Signal::SIGTERM);
            }
        }

        let deadline = Instant::now() + STOP_GRACE;
        while Instant::now() < deadline {
            if self.collect_stopped() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        for service in &mut self.services {
            let Some(child) = service.child.as_mut() else {
                continue;
            };
            println!(
                "[Vakt-Init] \x1b[1;33mService '{}' ignored SIGTERM; killing it.\x1b[0m",
                service.spec.name
            );
            let _ = child.kill();
            let _ = child.wait();
            service.child = None;
            service.clear_pid_file();
            service.state = State::Stopped;
            service.detail = "killed after refusing to stop".to_string();
        }
    }

    /// Reaps daemons that have exited during shutdown. Returns true once none
    /// are left.
    fn collect_stopped(&mut self) -> bool {
        let mut all_gone = true;
        for service in &mut self.services {
            let Some(child) = service.child.as_mut() else {
                continue;
            };
            match child.try_wait() {
                Ok(Some(_)) | Err(_) => {
                    service.child = None;
                    service.clear_pid_file();
                    service.state = State::Stopped;
                    service.detail = "stopped for shutdown".to_string();
                }
                Ok(None) => all_gone = false,
            }
        }
        all_gone
    }

    /// Writes a one-line-per-service summary for the panel to display.
    fn write_status(&self) {
        let mut body = String::new();
        for service in &self.services {
            let pid = service.pid().map(|p| p.to_string()).unwrap_or_default();
            let (ready, reported) = self.control.snapshot(service.spec.name);

            let readiness = match (service.spec.notifies, ready) {
                (false, _) => "n/a",
                (true, true) => "ready",
                (true, false) => "waiting",
            };
            let detail = reported.unwrap_or_else(|| service.spec.description.to_string());

            body.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\n",
                service.spec.name, service.state, pid, service.restarts, readiness, detail
            ));
        }

        let status_path = self.run_dir.join(STATUS_NAME);
        let _ = std::fs::create_dir_all(&self.run_dir);
        let tmp = status_path.with_extension("status.tmp");
        if let Ok(mut file) = File::create(&tmp)
            && file.write_all(body.as_bytes()).is_ok()
        {
            let _ = std::fs::rename(&tmp, &status_path);
            return;
        }
        let _ = std::fs::remove_file(&tmp);
    }
}

/// The services vakt-init brings up at boot.
pub const DEFAULT_SERVICES: &[Service] = &[
    Service {
        name: "vakt-net",
        program: "vakt-net",
        args: &[],
        respawn: true,
        notifies: true,
        description: "Wi-Fi and DHCP negotiation",
    },
    Service {
        name: "vakt-ids",
        program: "vakt-ids",
        args: &["--watch", "/persistent"],
        respawn: true,
        notifies: true,
        description: "Filesystem integrity monitor",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique scratch directory per test, so the run dirs never collide.
    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("vakt-init-test-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn supervisor(specs: &[Service], dir: PathBuf) -> Supervisor {
        Supervisor::with_run_dir(specs, Control::new(specs), dir)
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
        notifies: false,
        description: "exits immediately, never restarted",
    }];

    const CRASHER: &[Service] = &[Service {
        name: "crasher",
        program: "/bin/sh",
        args: &["-c", "exit 1"],
        respawn: true,
        notifies: false,
        description: "always fails to stay up",
    }];

    const LONG_RUNNING: &[Service] = &[Service {
        name: "sleeper",
        program: "/bin/sh",
        args: &["-c", "sleep 30"],
        respawn: true,
        notifies: false,
        description: "stays up",
    }];

    const NOTIFIER: &[Service] = &[Service {
        name: "notifier",
        program: "/bin/sh",
        args: &["-c", "sleep 30"],
        respawn: true,
        notifies: true,
        description: "reports readiness",
    }];

    /// `reap` only reports a change when a child exits, so without a second
    /// trigger the status file said `waiting` forever on a healthy appliance.
    #[test]
    fn readiness_republishes_the_status_file_without_anything_exiting() {
        let dir = scratch("readiness-republish");
        let mut sup = supervisor(NOTIFIER, dir.clone());
        let control = Arc::clone(&sup.control);
        sup.start_all();

        let published = control.revision();
        let before = std::fs::read_to_string(dir.join(STATUS_NAME)).unwrap();
        assert!(
            before.contains("\twaiting\t"),
            "a service that has not reported yet is waiting: {}",
            before
        );

        control.note_ready(None, Some("notifier"), Some("watching 3 files"));

        assert_ne!(
            control.revision(),
            published,
            "readiness must mark the published file stale"
        );
        // What the supervisor's loop does with that.
        assert!(!sup.reap(), "nothing exited");
        sup.write_status();

        let after = std::fs::read_to_string(dir.join(STATUS_NAME)).unwrap();
        assert!(after.contains("\tready\t"), "got: {}", after);
        assert!(
            after.contains("watching 3 files"),
            "the reported STATUS line is the useful part: {}",
            after
        );

        if let Some(child) = sup.services[0].child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A daemon inheriting PID 1's blocked SIGTERM ignores `stop_all` and is
    /// killed after the grace period instead. Read back out of /proc, since
    /// what is being tested is what the kernel did across fork and exec.
    #[test]
    fn a_spawned_service_does_not_inherit_a_blocked_sigterm() {
        use nix::sys::signal::{SigSet, Signal};

        let dir = scratch("sigmask");
        let mut sup = supervisor(LONG_RUNNING, dir.clone());

        // Stand in for what PID 1 does at boot.
        let mut blocked = SigSet::empty();
        blocked.add(Signal::SIGTERM);
        blocked.thread_block().unwrap();

        sup.start_all();
        let pid = sup.services[0].pid().expect("service should be running");

        let status = std::fs::read_to_string(format!("/proc/{}/status", pid)).unwrap();
        let sigblk = status
            .lines()
            .find_map(|l| l.strip_prefix("SigBlk:"))
            .expect("SigBlk in /proc/<pid>/status")
            .trim()
            .to_string();
        let blocked_bits = u64::from_str_radix(&sigblk, 16).unwrap();

        // SIGTERM is 15, so bit 14 in the mask /proc reports.
        let sigterm_bit = 1u64 << (Signal::SIGTERM as i32 - 1);
        assert_eq!(
            blocked_bits & sigterm_bit,
            0,
            "the child inherited a blocked SIGTERM (SigBlk={}); it will ignore \
             stop_all and be killed after the grace period instead",
            sigblk
        );

        let _ = SigSet::empty().thread_set_mask();
        if let Some(child) = sup.services[0].child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn running_service_writes_a_pid_file_and_is_reported_running() {
        let dir = scratch("running");
        let mut sup = supervisor(LONG_RUNNING, dir.clone());
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
        assert!(
            !sup.reap(),
            "reap should report no change for a live service"
        );
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
        let mut sup = supervisor(ONESHOT, dir.clone());
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
        let mut sup = supervisor(CRASHER, dir.clone());
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
            notifies: false,
            description: "binary is not installed",
        }];

        let mut sup = supervisor(MISSING, dir.clone());
        sup.start_all();

        assert_eq!(sup.services[0].state, State::Failed);
        let status = std::fs::read_to_string(dir.join(STATUS_NAME)).unwrap();
        assert!(status.contains("failed"), "got: {}", status);
    }

    /// The reason this supervisor reaps per-PID instead of using waitpid(-1):
    /// vakt-init's main thread collects vakt-panel's exit code while the
    /// supervisor runs alongside it. A wildcard reaper would race that call and
    /// swallow the result.
    #[test]
    fn supervisor_thread_does_not_steal_foreground_exit_codes() {
        let dir = scratch("coexist");
        let sup = supervisor(LONG_RUNNING, dir);
        let control = Arc::clone(&sup.control);
        let supervising = std::thread::spawn(move || sup.run());

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

        control.request_stop();
        supervising.join().expect("supervisor thread should finish");
    }

    #[test]
    fn service_output_is_captured_to_a_log_file() {
        let dir = scratch("logging");
        const TALKER: &[Service] = &[Service {
            name: "talker",
            program: "/bin/sh",
            args: &["-c", "echo hello-from-service; echo to-stderr >&2"],
            respawn: false,
            notifies: false,
            description: "writes to stdout and stderr",
        }];

        let mut sup = supervisor(TALKER, dir.clone());
        sup.start_all();
        reap_until_settled(&mut sup, 40);

        // The drain thread may still be finishing when the child is reaped.
        let log_path = dir.join("talker.log");
        let mut log = String::new();
        for _ in 0..40 {
            log = std::fs::read_to_string(&log_path).unwrap_or_default();
            if log.contains("hello-from-service") && log.contains("to-stderr") {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(log.contains("hello-from-service"), "got: {}", log);
        assert!(
            log.contains("to-stderr"),
            "stderr must be captured too: {}",
            log
        );
    }

    /// A daemon that will not stop printing must not be able to fill the
    /// tmpfs that /run lives on.
    #[test]
    fn a_runaway_daemons_log_is_capped() {
        let dir = scratch("runaway");
        const NOISY: &[Service] = &[Service {
            name: "noisy",
            program: "/bin/sh",
            args: &[
                "-c",
                "i=0; while [ $i -lt 40000 ]; do \
                 echo aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa; \
                 i=$((i+1)); done",
            ],
            respawn: false,
            notifies: false,
            description: "prints far more than it should",
        }];

        let mut sup = supervisor(NOISY, dir.clone());
        sup.start_all();
        reap_until_settled(&mut sup, 400);

        // Give the drain thread a moment to consume what is left in the pipe.
        std::thread::sleep(Duration::from_millis(500));

        let size = |name: &str| {
            std::fs::metadata(dir.join(name))
                .map(|m| m.len())
                .unwrap_or(0)
        };
        let total = size("noisy.log") + size("noisy.log.1");
        assert!(
            total <= logfile::BUDGET + 64 * 1024,
            "log capture grew to {} bytes, past the {} byte budget",
            total,
            logfile::BUDGET
        );
        assert!(total > 0, "nothing was captured at all");
    }

    #[test]
    fn shutdown_stops_running_services_and_reports_stopped() {
        let dir = scratch("shutdown");
        let sup = supervisor(LONG_RUNNING, dir.clone());
        let control = Arc::clone(&sup.control);
        let supervising = std::thread::spawn(move || sup.run());

        // Let it get the sleeper up before asking for shutdown.
        std::thread::sleep(Duration::from_millis(200));
        control.request_stop();

        assert!(
            control.wait_until_stopped(Duration::from_secs(10)),
            "supervisor never reported everything stopped"
        );
        supervising.join().expect("supervisor thread should finish");

        assert!(
            !dir.join("sleeper.pid").exists(),
            "pid file should be gone once the service is stopped"
        );
        let status = std::fs::read_to_string(dir.join(STATUS_NAME)).unwrap();
        assert!(status.contains("stopped"), "got: {}", status);
    }

    /// Boot must not sit waiting for a service that has already given up.
    #[test]
    fn readiness_wait_gives_up_on_a_service_that_cannot_start() {
        let dir = scratch("readiness-failed");
        const NOTIFIER: &[Service] = &[Service {
            name: "notifier",
            program: "/nonexistent/vakt-does-not-exist",
            args: &[],
            respawn: true,
            notifies: true,
            description: "would report readiness if it could run",
        }];

        let mut sup = supervisor(NOTIFIER, dir);
        let control = Arc::clone(&sup.control);
        sup.start_all();

        let waited = Instant::now();
        assert!(
            !control.wait_until_ready(Duration::from_secs(5)),
            "a service that never started cannot be ready"
        );
        assert!(
            waited.elapsed() < Duration::from_secs(2),
            "waited {:?} for a service already known to have failed",
            waited.elapsed()
        );
    }

    #[test]
    fn readiness_is_attributed_by_pid_not_by_claimed_name() {
        const TWO: &[Service] = &[
            Service {
                name: "first",
                program: "/bin/true",
                args: &[],
                respawn: false,
                notifies: true,
                description: "one",
            },
            Service {
                name: "second",
                program: "/bin/true",
                args: &[],
                respawn: false,
                notifies: true,
                description: "two",
            },
        ];

        let control = Control::new(TWO);
        control.register(4242, "first");

        // The sender claims to be "second"; the kernel says it is pid 4242,
        // which the supervisor knows is "first". The kernel wins.
        let attributed = control.note_ready(Some(4242), Some("second"), Some("up"));
        assert_eq!(attributed.as_deref(), Some("first"));
        assert_eq!(control.snapshot("first"), (true, Some("up".to_string())));
        assert_eq!(control.snapshot("second"), (false, None));

        // With no credentials there is nothing to check the claim against, so
        // the name is the only identity available.
        assert_eq!(
            control.note_ready(None, Some("second"), None).as_deref(),
            Some("second")
        );
        assert!(control.wait_until_ready(Duration::from_millis(10)));
    }
}
