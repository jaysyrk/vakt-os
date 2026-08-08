mod config;
mod notify;
mod sandbox;
mod status;

use config::NetConfig;
use status::{State, Status};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime};

const WPA_CONF: &str = "/run/wpa_supplicant.conf";
const WPA_PID: &str = "/run/wpa_supplicant.pid";

/// How long to wait for the supplicant to associate before asking for a lease.
const ASSOC_WAIT: Duration = Duration::from_secs(4);
/// Poll interval for noticing that the TUI rewrote the config.
const POLL: Duration = Duration::from_secs(1);
/// Retry backoff bounds after a failed connection attempt.
const BACKOFF_START: Duration = Duration::from_secs(5);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// How often to re-check that a healthy link still has its address.
const HEALTH_CHECK: Duration = Duration::from_secs(10);

macro_rules! log {
    ($($arg:tt)*) => {{
        println!("[vakt-net] {}", format!($($arg)*));
        use std::io::Write;
        let _ = std::io::stdout().flush();
    }};
}

fn main() {
    log!("Network daemon starting.");

    // Must run before sandbox::confine() locks in the Landlock ruleset - see
    // ensure_config_placeholder's own doc comment for why.
    config::ensure_config_placeholder();

    // Before anything else runs, and before the first child process is spawned,
    // so the confinement covers the helpers too.
    match sandbox::confine(&[config::PERSISTENT_CONF, config::FALLBACK_CONF]) {
        Ok(report) => log!("{}", report),
        Err(e) => log!("Could not apply the Landlock sandbox: {}", e),
    }

    let mut backoff = BACKOFF_START;
    // Readiness is reported once, at the first point the daemon has settled
    // into a state the rest of the system can act on. That is not the same as
    // "connected": with no configuration there is nothing to connect to, and
    // boot must not stall waiting for a network that was never set up.
    let mut announced = false;

    loop {
        let stamp = config::config_stamp();

        let Some((path, cfg)) = config::load() else {
            status::write(&Status {
                state: State::Unconfigured,
                interface: NetConfig::default().interface,
                ssid: None,
                ip: None,
                detail: format!(
                    "No config. Set up Wi-Fi in the panel, or write {}.",
                    config::PERSISTENT_CONF
                ),
            });
            log!("No configuration found; waiting for one to appear.");
            announce(&mut announced, "no network configured");
            wait_for_config_change(stamp, None);
            continue;
        };

        log!(
            "Using {} (interface {}, {}).",
            path.display(),
            cfg.interface,
            if cfg.is_wireless() {
                "wireless"
            } else {
                "wired"
            }
        );

        status::write(&Status {
            state: State::Connecting,
            interface: cfg.interface.clone(),
            ssid: cfg.ssid.clone(),
            ip: None,
            detail: "Bringing up interface...".to_string(),
        });

        match connect(&cfg) {
            Ok(ip) => {
                log!("Connected. Address {} on {}.", ip, cfg.interface);
                announce(&mut announced, &format!("{} on {}", ip, cfg.interface));
                status::write(&Status {
                    state: State::Connected,
                    interface: cfg.interface.clone(),
                    ssid: cfg.ssid.clone(),
                    ip: Some(ip),
                    detail: "Link is up.".to_string(),
                });
                backoff = BACKOFF_START;
                monitor(&cfg, stamp);
            }
            Err(e) => {
                log!(
                    "Connection failed: {}. Retrying in {}s.",
                    e,
                    backoff.as_secs()
                );
                status::write(&Status {
                    state: State::Failed,
                    interface: cfg.interface.clone(),
                    ssid: cfg.ssid.clone(),
                    ip: None,
                    detail: e.clone(),
                });
                // A network that will not come up is still a settled answer;
                // boot should carry on and let the panel show the failure.
                announce(&mut announced, &format!("connection failed: {}", e));
                // A config edit should cut the backoff short.
                wait_for_config_change(stamp, Some(backoff));
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        }
    }
}

/// Reports readiness to vakt-init the first time, and refreshes the status line
/// on every call after that.
fn announce(announced: &mut bool, detail: &str) {
    if *announced {
        notify::status(detail);
    } else {
        notify::ready(detail);
        *announced = true;
    }
}

/// Brings the interface up, associates if wireless, then requests a lease.
fn connect(cfg: &NetConfig) -> Result<String, String> {
    run("ip", &["link", "set", &cfg.interface, "up"]);

    if cfg.is_wireless() {
        let ssid = cfg.ssid.as_deref().unwrap_or_default();
        let psk = cfg.psk.as_deref().unwrap_or_default();

        stop_previous_supplicant();

        log!("Generating supplicant config for SSID '{}'.", ssid);
        let output = Command::new("wpa_passphrase")
            .arg(ssid)
            .arg(psk)
            .output()
            .map_err(|e| format!("wpa_passphrase unavailable: {}", e))?;

        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if err.is_empty() {
                "wpa_passphrase rejected the SSID/password".to_string()
            } else {
                err
            });
        }

        write_private(Path::new(WPA_CONF), &output.stdout)
            .map_err(|e| format!("cannot write {}: {}", WPA_CONF, e))?;

        log!("Starting wpa_supplicant on {}.", cfg.interface);
        let started = Command::new("wpa_supplicant")
            .args(["-B", "-i", &cfg.interface, "-c", WPA_CONF, "-P", WPA_PID])
            .status()
            .map_err(|e| format!("wpa_supplicant unavailable: {}", e))?;

        if !started.success() {
            return Err("wpa_supplicant failed to start".to_string());
        }

        // Association is asynchronous; give the radio a moment before DHCP.
        std::thread::sleep(ASSOC_WAIT);
    }

    request_lease(&cfg.interface)?;

    interface_address(&cfg.interface)
        .ok_or_else(|| format!("no address assigned to {}", cfg.interface))
}

/// Runs udhcpc, which daemonizes after obtaining a lease and handles renewals.
fn request_lease(interface: &str) -> Result<(), String> {
    let pid_file = format!("/run/udhcpc.{}.pid", interface);
    kill_from_pidfile(&pid_file);

    log!("Requesting DHCP lease on {}.", interface);
    let status = Command::new("udhcpc")
        .args(["-i", interface, "-t", "8", "-b", "-p", &pid_file])
        .status()
        .map_err(|e| format!("udhcpc unavailable: {}", e))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("DHCP failed on {}", interface))
    }
}

/// Stays here while the link is healthy, returning when it drops or the
/// configuration changes so the main loop can reconnect.
fn monitor(cfg: &NetConfig, stamp: Option<SystemTime>) {
    loop {
        std::thread::sleep(HEALTH_CHECK);

        if config::config_stamp() != stamp {
            log!("Configuration changed; reconnecting.");
            return;
        }

        match interface_address(&cfg.interface) {
            Some(ip) => status::write(&Status {
                state: State::Connected,
                interface: cfg.interface.clone(),
                ssid: cfg.ssid.clone(),
                ip: Some(ip),
                detail: "Link is up.".to_string(),
            }),
            None => {
                log!("Lost address on {}; reconnecting.", cfg.interface);
                return;
            }
        }
    }
}

/// Sleeps until the config file changes, or `timeout` elapses if given.
/// With no timeout this waits indefinitely for a config to appear.
fn wait_for_config_change(stamp: Option<SystemTime>, timeout: Option<Duration>) {
    let mut waited = Duration::ZERO;
    loop {
        std::thread::sleep(POLL);
        waited += POLL;

        if config::config_stamp() != stamp {
            return;
        }
        if timeout.is_some_and(|t| waited >= t) {
            return;
        }
    }
}

/// Parses the first IPv4 address out of `ip addr show <interface>`.
fn interface_address(interface: &str) -> Option<String> {
    let output = Command::new("ip")
        .args(["addr", "show", interface])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find_map(|line| {
            line.strip_prefix("inet ")?
                .split('/')
                .next()
                .map(str::to_string)
        })
}

/// Writes `contents` to `path` readable only by root.
///
/// `std::fs::write` would create it 0644, and what goes in here is the
/// supplicant configuration: `wpa_passphrase` emits the network's plaintext
/// password as a `#psk="..."` comment beside the hashed one, so a
/// world-readable copy hands the Wi-Fi password to every uid on the system -
/// including the unprivileged panel user and anything zrpkg installs.
///
/// Any stale file is removed first rather than being reopened and truncated:
/// `OpenOptions::mode` only applies to a file it actually creates, so
/// truncating an existing 0644 file would silently keep the wrong mode.
fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let _ = std::fs::remove_file(path);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents)
}

fn stop_previous_supplicant() {
    kill_from_pidfile(WPA_PID);
}

/// Kills a daemon via its pid file. busybox `kill` is the only killer available
/// in the rootfs, so shell out rather than use a signal crate.
fn kill_from_pidfile(pid_file: &str) {
    if !Path::new(pid_file).exists() {
        return;
    }
    if let Ok(contents) = std::fs::read_to_string(pid_file) {
        let pid = contents.trim();
        if !pid.is_empty() && pid.chars().all(|c| c.is_ascii_digit()) {
            log!("Stopping previous daemon (pid {}).", pid);
            run("kill", &[pid]);
            std::thread::sleep(Duration::from_millis(300));
        }
    }
    let _ = std::fs::remove_file(pid_file);
}

/// Fire-and-forget command; failures here are non-fatal and get caught later
/// by the address check.
fn run(program: &str, args: &[&str]) {
    let _ = Command::new(program).args(args).status();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("vakt-net-wpa-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn the_supplicant_config_is_readable_only_by_root() {
        let dir = scratch("fresh");
        let path = dir.join("wpa_supplicant.conf");

        write_private(&path, b"network={\n\t#psk=\"plaintext\"\n}\n").unwrap();

        assert_eq!(mode_of(&path), 0o600);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// OpenOptions::mode only applies to a file it creates, so a stale
    /// world-readable config left by an earlier run must be replaced rather
    /// than truncated in place - otherwise the wrong mode survives silently.
    #[test]
    fn a_stale_world_readable_config_does_not_keep_its_mode() {
        let dir = scratch("stale");
        let path = dir.join("wpa_supplicant.conf");
        std::fs::write(&path, b"old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(mode_of(&path), 0o644);

        write_private(&path, b"new").unwrap();

        assert_eq!(mode_of(&path), 0o600);
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        std::fs::remove_dir_all(&dir).ok();
    }
}
