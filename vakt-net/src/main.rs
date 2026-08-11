mod config;
mod link;
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

/// How long association and the WPA handshake get before it counts as failed.
const ASSOC_TIMEOUT: Duration = Duration::from_secs(20);
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

        let configured = config::load();

        // With nothing configured, a cable is still a network. The panel
        // cannot describe a wired setup - its only network page demands an
        // SSID - so without this an appliance with ethernet and no Wi-Fi has
        // no supported route online at all.
        let (source, cfg) = match configured {
            Some((path, cfg)) => (path.display().to_string(), cfg),
            None => match link::first_wired_link() {
                Some(interface) => {
                    log!(
                        "No configuration, but {} has a cable; using DHCP.",
                        interface
                    );
                    (
                        "a wired link, with no configuration".to_string(),
                        NetConfig {
                            interface,
                            ..NetConfig::default()
                        },
                    )
                }
                None => {
                    status::write(&Status {
                        state: State::Unconfigured,
                        interface: NetConfig::default().interface,
                        ssid: None,
                        ip: None,
                        detail: format!(
                            "No config and no wired link. Set up Wi-Fi in the panel, \
                             plug in a cable, or write {}.",
                            config::PERSISTENT_CONF
                        ),
                    });
                    log!("No configuration and no cable; waiting for either.");
                    announce(&mut announced, "no network configured");
                    // Bounded, unlike the old wait: plugging a cable into an
                    // unconfigured appliance changes no file, so there would
                    // be nothing to wake this up.
                    wait_for_config_change(stamp, Some(BACKOFF_MAX));
                    continue;
                }
            },
        };

        log!(
            "Using {} (interface {}, {}).",
            source,
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
    if !link::exists(&cfg.interface) {
        return Err(format!(
            "no interface named {}; the kernel has no driver for this card",
            cfg.interface
        ));
    }

    run("ip", &["link", "set", &cfg.interface, "up"]);

    if cfg.is_wireless() {
        if !link::has_radio() {
            return Err(format!(
                "{} exists but no 802.11 radio is registered; the driver loaded \
                 without its firmware, or the radio is hard-blocked",
                cfg.interface
            ));
        }

        let ssid = cfg.ssid.as_deref().unwrap_or_default();
        let psk = cfg.psk.as_deref().unwrap_or_default();

        stop_previous_supplicant();

        log!("Generating supplicant config for SSID '{}'.", ssid);
        let conf = supplicant_config(ssid, psk)?;
        write_private(Path::new(WPA_CONF), conf.as_bytes())
            .map_err(|e| format!("cannot write {}: {}", WPA_CONF, e))?;

        log!("Starting wpa_supplicant on {}.", cfg.interface);
        let started = Command::new("wpa_supplicant")
            .args(["-B", "-i", &cfg.interface, "-c", WPA_CONF, "-P", WPA_PID])
            .status()
            .map_err(|e| format!("wpa_supplicant unavailable: {}", e))?;

        if !started.success() {
            return Err("wpa_supplicant failed to start".to_string());
        }

        // Asking for a lease before the radio has associated wastes the whole
        // DHCP timeout and reports "no address", which is true of every
        // failure and diagnoses none of them.
        if !link::wait_for_carrier(&cfg.interface, ASSOC_TIMEOUT) {
            return Err(format!(
                "did not associate with '{}' within {}s; wrong passphrase, out \
                 of range, or the radio is blocked",
                ssid,
                ASSOC_TIMEOUT.as_secs()
            ));
        }
        log!("Associated with '{}'.", ssid);
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

/// Builds the wpa_supplicant configuration for one network.
///
/// This used to shell out to `wpa_passphrase`, which is the documented way to
/// turn a passphrase into a PSK, and there is no way to call it that is both
/// correct and safe. Given the passphrase in argv it publishes the Wi-Fi
/// password to every uid on the machine, `/proc/<pid>/cmdline` being mode
/// 0444. Given it on stdin instead, it tries to turn off terminal echo first
/// and a supervised daemon has no terminal, so it dies with
/// `tcgetattr: Inappropriate ioctl for device` and the connection fails.
///
/// wpa_supplicant accepts a quoted passphrase and derives the PSK itself, so
/// the subprocess buys nothing. Nor does it cost any secrecy: `wpa_passphrase`
/// wrote the plaintext into this same file anyway, as a `#psk="..."` comment
/// beside the hash, and [`write_private`] keeps the file 0600 and root-owned
/// either way.
fn supplicant_config(ssid: &str, psk: &str) -> Result<String, String> {
    config_safe("network name", ssid)?;
    if ssid.len() > 32 {
        return Err("a Wi-Fi network name cannot be longer than 32 characters".to_string());
    }

    // 64 hex digits is already a derived PSK rather than a passphrase, and
    // wpa_supplicant tells the two apart by the quoting, not by the length.
    let psk_line = if psk.len() == 64 && psk.bytes().all(|b| b.is_ascii_hexdigit()) {
        format!("\tpsk={}\n", psk)
    } else {
        config_safe("password", psk)?;
        if !(8..=63).contains(&psk.len()) {
            return Err("a Wi-Fi password must be 8 to 63 characters".to_string());
        }
        format!("\tpsk=\"{}\"\n", psk)
    };

    Ok(format!("network={{\n\tssid=\"{}\"\n{}}}\n", ssid, psk_line))
}

/// Rejects anything that cannot survive being written between double quotes.
///
/// Refused rather than escaped: a newline here would let whoever sets the
/// network name append arbitrary directives to the supplicant's configuration,
/// and no legal SSID or WPA passphrase contains one. Saying so plainly beats
/// silently connecting to something other than what was asked for.
fn config_safe(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("the Wi-Fi {} is empty", field));
    }
    if let Some(bad) = value
        .chars()
        .find(|c| c.is_control() || *c == '"' || *c == '\\')
    {
        return Err(format!(
            "the Wi-Fi {} contains a character the supplicant's configuration \
             cannot carry: {:?}",
            field, bad
        ));
    }
    Ok(())
}

/// Writes `contents` to `path` readable only by root.
///
/// `std::fs::write` would create it 0644, and what goes in here is the
/// supplicant configuration, which carries the network's password in the
/// clear. `/run` is a tmpfs mounted 0755, so a world-readable copy there hands
/// the Wi-Fi password to every uid on the system - including the unprivileged
/// panel user and anything zrpkg installs.
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

    /// The passphrase must reach wpa_supplicant without ever being an
    /// argument and without needing a terminal - the two ways this was wrong
    /// before.
    #[test]
    fn a_passphrase_is_written_quoted_for_wpa_supplicant_to_derive() {
        let conf = supplicant_config("Stkyezone_EXT", "correcthorsebattery").unwrap();
        assert_eq!(
            conf,
            "network={\n\tssid=\"Stkyezone_EXT\"\n\tpsk=\"correcthorsebattery\"\n}\n"
        );
    }

    /// 64 hex digits is a derived PSK already. Quoting it would make
    /// wpa_supplicant treat the hash itself as the passphrase and hash it
    /// again, so the network would simply never associate.
    #[test]
    fn an_already_derived_psk_is_written_unquoted() {
        let raw = "a".repeat(64);
        let conf = supplicant_config("Net", &raw).unwrap();
        assert!(conf.contains(&format!("psk={}\n", raw)), "{}", conf);
        assert!(
            !conf.contains("psk=\""),
            "a raw PSK must not be quoted: {}",
            conf
        );
    }

    /// Whoever types the network name into the panel must not be able to
    /// append directives to the supplicant's configuration.
    #[test]
    fn a_newline_cannot_smuggle_extra_configuration_in() {
        let injected = supplicant_config("Net\n}\nnetwork={\n\tssid=\"Evil\"", "password123");
        assert!(injected.is_err(), "a newline in the SSID must be refused");

        let via_psk = supplicant_config("Net", "pass\n\tkey_mgmt=NONE");
        assert!(
            via_psk.is_err(),
            "a newline in the password must be refused"
        );

        let quoted = supplicant_config("Net", "pass\"word\"here");
        assert!(quoted.is_err(), "a double quote must be refused");
    }

    #[test]
    fn a_passphrase_wpa_cannot_use_is_refused_before_the_radio_is_touched() {
        assert!(
            supplicant_config("Net", "short").is_err(),
            "under 8 characters"
        );
        assert!(
            supplicant_config("Net", &"x".repeat(64)).is_err(),
            "over 63 characters"
        );
        assert!(supplicant_config("Net", "").is_err(), "empty");
        assert!(supplicant_config("", "password123").is_err(), "empty SSID");
        assert!(
            supplicant_config(&"n".repeat(33), "password123").is_err(),
            "an SSID cannot exceed 32 characters"
        );
    }

    #[test]
    fn an_ordinary_password_with_punctuation_is_accepted() {
        // The parser already allows = # and spaces in a password; the config
        // writer must not quietly disagree with it.
        let conf = supplicant_config("Net", "a b=c#d!$%").unwrap();
        assert!(conf.contains("psk=\"a b=c#d!$%\"\n"), "{}", conf);
    }
}
