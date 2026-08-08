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
            if persistent {
                privilege::grant(Path::new(ZRPKG_ROOT), id);
                privilege::grant(Path::new("/persistent/etc"), id);
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
    let shell_identity = if root_shell_requested() {
        None
    } else {
        identity
    };

    while !shutdown::under_way() {
        println!("[Vakt-Init] Launching Vakt Panel...");
        // Absolute path, not "vakt-panel": the console session's PATH puts
        // the zrpkg install root first (see session_environment) so an
        // operator can type an installed package's name directly, but that
        // means a bare name here would let any installed package containing
        // usr/bin/vakt-panel silently replace the real panel on every future
        // launch. cttyhack's own exec only consults PATH for a name with no
        // '/' in it, so a literal absolute path bypasses that lookup.
        let _ = run_on_console(&["/usr/bin/vakt-panel"], identity, &session);

        if shutdown::under_way() {
            break;
        }

        println!("[Vakt-Init] Panel exited. Dropping to a shell...");
        let _ = run_on_console(&["/bin/sh"], shell_identity, &session);
    }

    // Shutdown is in progress on the signal thread and ends in reboot(2).
    // Returning from main would kill PID 1 and panic the kernel, so wait here.
    loop {
        std::thread::park();
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

    if let Some(id) = identity {
        let (uid, gid) = (id.uid, id.gid);
        command.env("HOME", &id.home);
        command.env("USER", &id.name);
        command.current_dir(&id.home);
        // Runs in the forked child between fork and exec, so the program on the
        // other side of exec can never have been root.
        unsafe {
            command.pre_exec(move || privilege::become_user(uid, gid));
        }
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
