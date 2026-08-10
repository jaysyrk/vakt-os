//! Vakt OS PID 1.
//!
//! Brings up the filesystems, seals the root, starts and supervises the
//! background daemons, waits for them to report ready, and hands the console to
//! the panel as an unprivileged user. It is also the only process that can shut
//! the machine down cleanly, so it stays for the whole life of the system.

mod envblock;
mod logfile;
mod mount;
mod notify;
mod privilege;
mod services;
mod shutdown;
mod sysctl;
mod update;

use notify::Listener;
use privilege::Identity;
use services::Control;
use std::env;
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitStatus};
use std::sync::Arc;
use std::time::Duration;

/// Where zrpkg unpacks packages. On the persistent disk, so installs survive a
/// reboot.
const ZRPKG_ROOT: &str = "/persistent/zrpkg";

/// vakt-net's config file. Kept in sync with vakt-net's own
/// `config::PERSISTENT_CONF`; vakt-init creates it at boot so the daemon's
/// Landlock ruleset has a path to name. See `privilege::grant_file`.
const VAKT_NET_CONF: &str = "/persistent/etc/vakt-net.conf";

/// The panel's stored PIN. Must belong to the panel's user, which it will not
/// if it was written by a panel that was running as root. See
/// `privilege::adopt_file`.
const VAKT_PANEL_AUTH: &str = "/persistent/etc/vakt-panel.auth";

/// Where vakt-ids records findings, and the panel reads them from.
const IDS_ALERTS: &str = "/run/vakt-ids.alerts";

/// PID 1's own PATH. Deliberately only the image's own directories: the package
/// install root is writable by the unprivileged user, and anything reachable
/// from there must never be a candidate for a program root runs.
const SYSTEM_PATH: &str = "/bin:/sbin:/usr/bin:/usr/sbin";

/// How long boot waits for daemons to report readiness before drawing the panel
/// anyway. Reaching this is not fatal - it only means the panel appears while
/// something is still starting.
const READY_TIMEOUT: Duration = Duration::from_secs(10);

/// Kernel command line flag that gives the console fallback shell root, for
/// recovering an image whose panel will not start.
const ROOT_SHELL_FLAG: &str = "vakt.rootshell";

/// A console round (panel, then fallback shell) finishing faster than this
/// means neither ever got going - nobody read a menu or typed at a prompt in
/// under a second.
const FUTILE_ROUND: Duration = Duration::from_secs(1);
/// How many futile rounds to allow before slowing down and explaining.
const FUTILE_ROUNDS_BEFORE_BACKOFF: u32 = 3;
/// Ceiling on the wait between futile rounds. Long enough that the console
/// stays readable, short enough that a transient problem still recovers
/// without a reboot.
const FUTILE_BACKOFF: Duration = Duration::from_secs(30);

fn main() {
    unsafe {
        env::set_var("PATH", SYSTEM_PATH);
        env::set_var("HOME", "/root");
    }

    // Before any thread exists, so every thread inherits the blocked mask and
    // shutdown signals can only ever be read off the signalfd.
    let signal_mask = shutdown::block_signals();

    println!("[Vakt-Init] Mounting virtual filesystems...");
    mount::virtual_filesystems();

    let (applied, total) = sysctl::harden();
    println!(
        "[Vakt-Init] Applied {}/{} kernel hardening sysctls.",
        applied, total
    );

    println!("[Vakt-Init] Isolating volatile RAM filesystems...");
    mount::volatile_filesystems();

    // Must run before mount_persistent(): the storage controller and disk
    // driver the data disk needs are commonly loadable modules, not built
    // into the kernel, and searching for the disk before its driver exists
    // can never find it.
    println!("[Vakt-Init] Loading hardware drivers...");
    load_modules();

    println!("[Vakt-Init] Searching for persistent storage...");
    let persistent = mount::mount_persistent();

    // May reboot straight back to slot A and never return - see update.rs.
    update::check_and_handle(persistent);

    // Everything that has to write to the image itself has now happened.
    mount::seal_root();
    shutdown::disable_ctrl_alt_del();

    // The panel and everything it launches runs as this user. Without the
    // account the system still boots; it just boots less safely, and says so.
    let identity = privilege::lookup(privilege::VAKT_USER);
    match &identity {
        Some(id) => {
            println!(
                "[Vakt-Init] Panel will run as {} (uid {}).",
                id.name, id.uid
            );
            privilege::grant_console(id);
            privilege::grant(Path::new(&id.home), id);
            // Created before vakt-ids starts, so the daemon appends to a file
            // the panel can already read. vakt-ids runs as root and would
            // otherwise create it 0600 root-owned on its first finding, which
            // the panel cannot open at all - and an unreadable alert file is
            // worse than no alert file, because the page renders it as "no
            // alerts recorded". Not gated on persistent storage: /run is a
            // tmpfs that exists either way, and an appliance in RAM-only mode
            // still reports findings.
            privilege::grant_file(Path::new(IDS_ALERTS), id);
            if persistent {
                privilege::grant(Path::new(ZRPKG_ROOT), id);
                privilege::grant(Path::new("/persistent/etc"), id);
                // Created here, before vakt-net starts, so the daemon's
                // Landlock ruleset can name a path that already exists - and
                // owned by the panel's user, so the panel can rewrite it.
                privilege::grant_file(Path::new(VAKT_NET_CONF), id);
                // Adopted, never created: an auth file written while the panel
                // was running as root stays root-owned and 0600 forever, and
                // the panel then cannot read the PIN it is meant to check
                // against - so it reports no PIN and refuses the correct one.
                privilege::adopt_file(Path::new(VAKT_PANEL_AUTH), id);
            }
        }
        None => println!(
            "[Vakt-Init] \x1b[1;33mNo '{}' account in /etc/passwd; \
             the panel will run as root.\x1b[0m",
            privilege::VAKT_USER
        ),
    }

    banner();

    println!("[Vakt-Init] Starting system services...");
    let control = Control::new(services::DEFAULT_SERVICES);

    // Daemons find the readiness socket through the environment, so it has to
    // exist and be advertised before the supervisor spawns anything.
    let listener = match Listener::bind(
        Path::new(notify::SOCKET_PATH),
        identity.as_ref().map(|i| i.gid),
    ) {
        Ok(listener) => {
            unsafe { env::set_var(notify::SOCKET_ENV, listener.path()) };
            Some(listener)
        }
        Err(e) => {
            println!(
                "[Vakt-Init] \x1b[1;33mNo readiness socket ({}); \
                 boot will not wait for daemons.\x1b[0m",
                e
            );
            None
        }
    };

    if let Some(listener) = listener {
        let control = Arc::clone(&control);
        std::thread::spawn(move || watch_notifications(listener, control));
    }

    match signal_mask {
        Ok(mask) => {
            let control = Arc::clone(&control);
            std::thread::spawn(move || shutdown::watch_signals(mask, control));
        }
        Err(e) => println!(
            "[Vakt-Init] \x1b[1;31mCould not block shutdown signals ({}); \
             clean shutdown is unavailable.\x1b[0m",
            e
        ),
    }

    {
        let supervisor =
            services::Supervisor::new(services::DEFAULT_SERVICES, Arc::clone(&control));
        std::thread::spawn(move || supervisor.run());
    }

    if control.wait_until_ready(READY_TIMEOUT) {
        println!("[Vakt-Init] All services reported ready.");
    } else {
        println!(
            "[Vakt-Init] \x1b[1;33mNot every service reported ready within {}s; \
             starting the panel anyway.\x1b[0m",
            READY_TIMEOUT.as_secs()
        );
    }

    update::confirm();

    console_loop(identity.as_ref(), persistent);
}

/// Runs the panel, falling back to a shell if it exits, until the system goes
/// down. This is the main thread's final job; shutdown happens on another one.
fn console_loop(identity: Option<&Identity>, persistent: bool) {
    let session = session_environment(identity, persistent);
    let recovery = root_shell_requested();
    let shell_identity = if recovery { None } else { identity };

    // `vakt.rootshell` has to skip the panel outright, not merely change who
    // the fallback shell runs as. It is the documented way back in after a
    // forgotten PIN (docs/OPERATIONS.md), and a panel that starts correctly
    // never exits - so leaving it in the way made the recovery entry reach a
    // PIN prompt and stop, which is precisely the situation it exists to
    // rescue. It only appeared to work while the panel was crash-looping.
    if recovery {
        println!(
            "[Vakt-Init] \x1b[1;33m{} on the kernel command line: \
             skipping the panel, going straight to a root shell.\x1b[0m",
            ROOT_SHELL_FLAG
        );
        while !shutdown::under_way() {
            let shell = run_on_console(&["/bin/sh"], None, &session);
            report_exit("Recovery shell", &shell);
            if shutdown::under_way() {
                break;
            }
            // A shell that will not stay is not something to retry at speed.
            std::thread::sleep(Duration::from_secs(1));
        }
        loop {
            std::thread::park();
        }
    }

    // Consecutive rounds where both the panel and the fallback shell gave up
    // immediately. Without this the loop spins as fast as the two execs
    // return: the console fills faster than it can be read, the reason
    // scrolls away, and the machine looks hung when it is actually retrying.
    let mut futile_rounds: u32 = 0;

    while !shutdown::under_way() {
        let round_started = std::time::Instant::now();

        println!("[Vakt-Init] Launching Vakt Panel...");
        // Absolute path, not "vakt-panel": the console session's PATH puts
        // the zrpkg install root first (see session_environment) so an
        // operator can type an installed package's name directly, but that
        // means a bare name here would let any installed package containing
        // usr/bin/vakt-panel silently replace the real panel on every future
        // launch. cttyhack's own exec only consults PATH for a name with no
        // '/' in it, so a literal absolute path bypasses that lookup.
        let panel = run_on_console(&["/usr/bin/vakt-panel"], identity, &session);
        report_exit("Vakt Panel", &panel);

        if shutdown::under_way() {
            break;
        }

        println!("[Vakt-Init] Panel exited. Dropping to a shell...");
        let shell = run_on_console(&["/bin/sh"], shell_identity, &session);
        report_exit("Shell", &shell);

        // A round where the panel and the shell both came straight back is a
        // console the console session cannot use - most often /dev/console
        // not being openable by the user they run as. Retrying instantly
        // just hides the reason, so slow down and say so plainly.
        if round_started.elapsed() < FUTILE_ROUND {
            futile_rounds += 1;
        } else {
            futile_rounds = 0;
        }

        if futile_rounds >= FUTILE_ROUNDS_BEFORE_BACKOFF {
            let pause = FUTILE_BACKOFF.min(Duration::from_secs(1 << (futile_rounds.min(5))));
            println!(
                "\n[Vakt-Init] \x1b[1;31mNeither the panel nor a shell will stay on this \
                 console ({} rounds).\x1b[0m",
                futile_rounds
            );
            match identity {
                Some(id) => println!(
                    "[Vakt-Init] They run as '{}' (uid {}). If /dev/console is not \
                     openable by that user this is what it looks like.",
                    id.name, id.uid
                ),
                None => println!("[Vakt-Init] They run as root."),
            }
            println!(
                "[Vakt-Init] Boot the 'Vakt OS (root recovery shell)' GRUB entry to \
                 investigate; retrying in {}s.\n",
                pause.as_secs()
            );
            std::thread::sleep(pause);
        }
    }

    // Shutdown is in progress on the signal thread and ends in reboot(2).
    // Returning from main would kill PID 1 and panic the kernel, so wait here.
    loop {
        std::thread::park();
    }
}

/// Says how a console program ended, rather than discarding it.
///
/// The exit status is the only evidence there is about why the panel would
/// not stay up: the panel draws over the screen, so anything it printed on
/// the way out is usually gone by the time anyone looks. A status of 1 with
/// no output is a very different problem from a signal.
fn report_exit(what: &str, result: &std::io::Result<ExitStatus>) {
    use std::os::unix::process::ExitStatusExt;

    match result {
        Ok(status) => {
            if status.success() {
                println!("[Vakt-Init] {} exited normally.", what);
            } else if let Some(signal) = status.signal() {
                println!(
                    "[Vakt-Init] \x1b[1;33m{} was killed by signal {}.\x1b[0m",
                    what, signal
                );
            } else {
                println!(
                    "[Vakt-Init] \x1b[1;33m{} exited with status {}.\x1b[0m",
                    what,
                    status.code().unwrap_or(-1)
                );
            }
        }
        Err(e) => println!("[Vakt-Init] \x1b[1;31mCould not run {}: {}\x1b[0m", what, e),
    }
}

/// Starts a program on the console, as `identity` when one is given.
///
/// `setsid` and `cttyhack` are busybox applets: the first puts the program in
/// its own session, the second attaches the system console to it as a
/// controlling terminal. Both run as the target user too, which is why
/// `grant_console` has to hand over `/dev/console` first.
fn run_on_console(
    argv: &[&str],
    identity: Option<&Identity>,
    session: &[(&str, String)],
) -> std::io::Result<ExitStatus> {
    let mut command = Command::new("setsid");
    command.arg("cttyhack");
    command.args(argv);

    for (key, value) in session {
        command.env(key, value);
    }

    let drop_to = identity.map(|id| (id.uid, id.gid));
    if let Some(id) = identity {
        command.env("HOME", &id.home);
        command.env("USER", &id.name);
        command.current_dir(&id.home);
    }

    // Runs in the forked child between fork and exec, so the program on the
    // other side of exec can never have been root.
    //
    // The mask is cleared first and unconditionally. PID 1 blocks the shutdown
    // signals to read them off a signalfd, a signal mask survives execve, and
    // an inherited one leaves the panel and the recovery shell running with
    // SIGINT blocked - which is a shell where ctrl-c does nothing.
    //
    // SAFETY: both calls are async-signal-safe and neither allocates, which is
    // the whole constraint on this side of the fork.
    unsafe {
        command.pre_exec(move || {
            nix::sys::signal::SigSet::empty()
                .thread_set_mask()
                .map_err(std::io::Error::from)?;
            if let Some((uid, gid)) = drop_to {
                privilege::become_user(uid, gid)?;
            }
            Ok(())
        });
    }

    let mut child = command.spawn()?;
    shutdown::set_foreground(child.id());
    let status = child.wait();
    shutdown::clear_foreground();
    status
}

/// The environment the console session gets.
///
/// The package install root goes on the console session's PATH and not on PID
/// 1's, so a package the unprivileged user installed is reachable from the
/// panel without ever becoming something a root process would pick up.
fn session_environment(
    identity: Option<&Identity>,
    persistent: bool,
) -> Vec<(&'static str, String)> {
    let mut session = vec![(
        "PS1",
        "\\[\\e[1;31m\\][Vakt-OS]\\[\\e[0m\\] \\w \\$ ".to_string(),
    )];

    if persistent {
        session.push(("ZRPKG_ROOT", ZRPKG_ROOT.to_string()));
        session.push(("PATH", format!("{}/usr/bin:{}", ZRPKG_ROOT, SYSTEM_PATH)));
    } else {
        session.push(("PATH", SYSTEM_PATH.to_string()));
    }

    if identity.is_none() {
        session.push(("USER", "root".to_string()));
    }
    session
}

/// Receives readiness and shutdown requests from `/run/init.sock`.
fn watch_notifications(listener: Listener, control: Arc<Control>) {
    loop {
        let message = match listener.recv() {
            Ok(message) => message,
            Err(nix::errno::Errno::EINTR) => continue,
            Err(e) => {
                println!(
                    "[Vakt-Init] \x1b[1;33mReadiness socket failed ({}); \
                     no longer listening.\x1b[0m",
                    e
                );
                return;
            }
        };

        if let Some(verb) = message.shutdown.as_deref() {
            match shutdown::Action::from_verb(verb) {
                Some(action) => shutdown::execute(action, &control),
                None => println!("[Vakt-Init] Ignoring unknown shutdown request '{}'.", verb),
            }
        }

        if message.ready {
            match control.note_ready(
                message.pid,
                message.name.as_deref(),
                message.status.as_deref(),
            ) {
                Some(name) => println!("[Vakt-Init] Service '{}' is ready.", name),
                None => println!("[Vakt-Init] Ignoring readiness from an unknown sender."),
            }
        }
    }
}

/// Loads a driver for every device the kernel has enumerated.
///
/// Walking modaliases is what a udev rule would do; there is no udev here, and
/// this runs a few times rather than watching for hotplug - loading a
/// controller driver (e.g. a USB host controller) can itself cause the
/// devices attached to it to finish enumerating and gain their own modalias
/// only after this scan already passed them by, so one static snapshot can
/// miss real hardware. modprobe is idempotent, so repeating this costs
/// nothing for devices already bound.
///
/// The image's own kernel is monolithic and has no modules at all, in which
/// case there is nothing to load and running modprobe a few hundred times to
/// be told so would only slow boot down.
fn load_modules() {
    if !Path::new("/lib/modules").is_dir() {
        println!("[Vakt-Init] Monolithic kernel; no modules to load.");
        return;
    }

    const PASSES: u32 = 3;
    for pass in 0..PASSES {
        if pass > 0 {
            std::thread::sleep(Duration::from_secs(1));
        }
        let _ = Command::new("sh")
            .arg("-c")
            .arg("find /sys -name modalias -exec cat {} + | sort -u | xargs -n 1 modprobe 2>/dev/null")
            .status();
    }
}

fn banner() {
    println!("\x1b[1;31m");
    if let Ok(logo) = fs::read_to_string("/etc/vakt_logo.txt") {
        println!("{}", logo);
    }
    println!("\x1b[0m");
    println!("Welcome to Vakt OS");
    println!();
}

/// Whether the kernel command line asked for a root recovery shell.
fn root_shell_requested() -> bool {
    fs::read_to_string("/proc/cmdline")
        .map(|cmdline| cmdline.split_whitespace().any(|arg| arg == ROOT_SHELL_FLAG))
        .unwrap_or(false)
}
