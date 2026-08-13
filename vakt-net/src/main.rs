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
/// What /etc/resolv.conf symlinks to; /etc is read-only after boot. udhcpc's
/// script writes the same file on every lease.
const RESOLV_CONF: &str = "/run/resolv.conf";

/// How long association and the WPA handshake get before it counts as failed.
const ASSOC_TIMEOUT: Duration = Duration::from_secs(20);
/// How long to watch for an address after udhcpc starts. It keeps retrying in
/// the background, so a lease can arrive well after it has forked away.
const DHCP_TIMEOUT: Duration = Duration::from_secs(15);
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
        let mut started = start_supplicant(ssid, psk, &cfg.interface, true)?;

        // A wpa_supplicant built without SAE does not skip the option, it
        // rejects the whole file and exits - so the modern block cannot simply
        // be written and hoped for.
        if !started.success() {
            log!("Configuration refused; retrying without WPA3 and hidden-network support.");
            started = start_supplicant(ssid, psk, &cfg.interface, false)?;
        }

        if !started.success() {
            return Err("wpa_supplicant failed to start".to_string());
        }

        // Asking for a lease before association wastes the DHCP timeout and
        // reports "no address", which is true of every failure and diagnoses
        // none of them.
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

    if cfg.is_static() {
        return configure_static(cfg);
    }

    request_lease(&cfg.interface)?;

    // udhcpc's exit status says nothing: -b makes it fork into the background
    // and exit 0 the moment the first attempt fails, so it reports success for
    // a network that never answered. The address is the only evidence, and it
    // can arrive after udhcpc has already forked away.
    poll(DHCP_TIMEOUT, || interface_address(&cfg.interface)).ok_or_else(|| {
        format!(
            "no DHCP lease on {} within {}s; the network answered no address request",
            cfg.interface,
            DHCP_TIMEOUT.as_secs()
        )
    })
}

/// Rejects an address with no prefix length before `ip` can accept it.
///
/// `ip addr replace 192.168.1.50 dev wlan0` succeeds and silently means /32,
/// which puts no subnet on the link - so the gateway is then unreachable and
/// the only error is `Nexthop has invalid gateway` from a step that is not the
/// one at fault.
fn checked_address(address: &str) -> Result<(), String> {
    if address.contains('/') {
        return Ok(());
    }
    Err(format!(
        "'{}' needs a prefix length, like {}/24; without one the kernel assumes \
         a single address and the gateway cannot be reached",
        address, address
    ))
}

/// Applies a hand-configured address, for a network with no DHCP server - or
/// one whose server does not answer.
///
/// `replace` throughout, so reconnecting to the same network is idempotent and
/// a second interface can still take the default route.
fn configure_static(cfg: &NetConfig) -> Result<String, String> {
    let address = cfg.address.as_deref().unwrap_or_default();
    checked_address(address)?;
    log!("Configuring {} statically as {}.", cfg.interface, address);

    let ok = Command::new("ip")
        .args(["addr", "replace", address, "dev", &cfg.interface])
        .status()
        .map_err(|e| format!("ip unavailable: {}", e))?;
    if !ok.success() {
        return Err(format!(
            "'{}' was refused as an address for {}",
            address, cfg.interface
        ));
    }

    if let Some(gateway) = cfg.gateway.as_deref().filter(|g| !g.is_empty()) {
        let routed = Command::new("ip")
            .args([
                "route",
                "replace",
                "default",
                "via",
                gateway,
                "dev",
                &cfg.interface,
            ])
            .status()
            .map_err(|e| format!("ip unavailable: {}", e))?;
        if !routed.success() {
            return Err(format!(
                "the address was set, but '{}' was refused as a gateway; nothing \
                 beyond this network will be reachable",
                gateway
            ));
        }
    }

    if !cfg.dns.is_empty() {
        let resolv: String = cfg
            .dns
            .iter()
            .map(|s| format!("nameserver {}\n", s))
            .collect();
        if let Err(e) = std::fs::write(RESOLV_CONF, resolv) {
            log!(
                "Could not write {}: {}. Names will not resolve.",
                RESOLV_CONF,
                e
            );
        }
    }

    interface_address(&cfg.interface).ok_or_else(|| {
        format!(
            "set {} on {}, but the kernel reports no address there",
            address, cfg.interface
        )
    })
}

/// Calls `f` until it yields a value or `timeout` elapses.
fn poll<T>(timeout: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let step = Duration::from_millis(500);
    let mut waited = Duration::ZERO;
    loop {
        if let Some(value) = f() {
            return Some(value);
        }
        if waited >= timeout {
            return None;
        }
        std::thread::sleep(step);
        waited += step;
    }
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

    // Only a failure to launch is reported here. A zero exit means udhcpc
    // started, not that a lease exists - see the caller.
    if status.success() {
        Ok(())
    } else {
        Err(format!("udhcpc could not start on {}", interface))
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

/// Writes the supplicant configuration and starts the daemon on `interface`.
fn start_supplicant(
    ssid: &str,
    psk: &str,
    interface: &str,
    wpa3: bool,
) -> Result<std::process::ExitStatus, String> {
    let conf = supplicant_config(ssid, psk, wpa3)?;
    write_private(Path::new(WPA_CONF), conf.as_bytes())
        .map_err(|e| format!("cannot write {}: {}", WPA_CONF, e))?;

    log!("Starting wpa_supplicant on {}.", interface);
    Command::new("wpa_supplicant")
        .args(["-B", "-i", interface, "-c", WPA_CONF, "-P", WPA_PID])
        .status()
        .map_err(|e| format!("wpa_supplicant unavailable: {}", e))
}

/// Builds the wpa_supplicant configuration for one network.
///
/// Deliberately not `wpa_passphrase`: passing the passphrase in argv publishes
/// it via `/proc/<pid>/cmdline`, and passing it on stdin makes it die with
/// `tcgetattr: Inappropriate ioctl for device` since a daemon has no terminal.
/// wpa_supplicant derives the PSK from a quoted passphrase itself.
///
/// With `wpa3`, the network also accepts SAE and looks for hidden networks.
/// Without it, the block is the bare minimum every build understands - see
/// [`connect`] for why both exist.
fn supplicant_config(ssid: &str, psk: &str, wpa3: bool) -> Result<String, String> {
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

    // Without key_mgmt, wpa_supplicant defaults to WPA-PSK and WPA-EAP, so a
    // WPA3-only network never associates. Listing all three keeps WPA2 working.
    // ieee80211w=1 negotiates management-frame protection, which SAE requires
    // and WPA2 ignores; =2 would demand it and break WPA2. scan_ssid finds a
    // network that does not broadcast its name.
    let extras = if wpa3 {
        "\tkey_mgmt=WPA-PSK WPA-PSK-SHA256 SAE\n\tieee80211w=1\n\tscan_ssid=1\n"
    } else {
        ""
    };

    Ok(format!(
        "network={{\n\tssid=\"{}\"\n{}{}}}\n",
        ssid, psk_line, extras
    ))
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

/// Writes `contents` to `path` readable only by root - this carries the
/// network's password in the clear, and `std::fs::write` would create it 0644
/// on a 0755 tmpfs.
///
/// Any stale file is removed first: `OpenOptions::mode` applies only to a file
/// it creates, so truncating an existing 0644 file would keep the wrong mode.
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

    /// ip accepts a bare address as /32 and the failure then surfaces two
    /// steps later as "Nexthop has invalid gateway", blaming the gateway for
    /// the address's mistake.
    #[test]
    fn an_address_without_a_prefix_is_refused_here() {
        let err = checked_address("192.168.1.50").unwrap_err();
        assert!(err.contains("prefix length"), "{}", err);
        assert!(err.contains("192.168.1.50/24"), "{}", err);
        assert!(checked_address("192.168.1.50/24").is_ok());
        assert!(checked_address("10.0.0.5/8").is_ok());
    }

    /// udhcpc forks away and keeps trying, so the address can appear well
    /// after it exits - checking once is what made a working network report
    /// "no address assigned".
    #[test]
    fn polling_waits_for_a_late_answer() {
        let mut calls = 0;
        let got = poll(Duration::from_secs(5), || {
            calls += 1;
            if calls >= 3 { Some("10.0.0.2") } else { None }
        });
        assert_eq!(got, Some("10.0.0.2"));
        assert!(calls >= 3, "gave up before the value arrived");
    }

    #[test]
    fn polling_gives_up_rather_than_hanging() {
        let start = std::time::Instant::now();
        let got: Option<()> = poll(Duration::from_millis(600), || None);
        assert!(got.is_none());
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "poll overran its timeout"
        );
    }

    /// Without key_mgmt, wpa_supplicant defaults to WPA-PSK and WPA-EAP, so a
    /// WPA3-only network never associates and a hidden one is never found.
    #[test]
    fn the_modern_block_offers_wpa3_and_finds_hidden_networks() {
        let conf = supplicant_config("Net", "password123", true).unwrap();
        assert!(
            conf.contains("key_mgmt=WPA-PSK WPA-PSK-SHA256 SAE"),
            "{}",
            conf
        );
        assert!(conf.contains("scan_ssid=1"), "{}", conf);
        // =2 would demand management-frame protection and break WPA2.
        assert!(conf.contains("ieee80211w=1"), "{}", conf);
    }

    /// The fallback exists because a wpa_supplicant without SAE rejects the
    /// whole file rather than the one option, so it has to be exactly the
    /// block that worked before any of this was added.
    #[test]
    fn the_fallback_block_is_the_bare_minimum() {
        let conf = supplicant_config("Net", "password123", false).unwrap();
        assert_eq!(
            conf,
            "network={\n\tssid=\"Net\"\n\tpsk=\"password123\"\n}\n"
        );
    }

    /// The passphrase must reach wpa_supplicant without ever being an
    /// argument and without needing a terminal - the two ways this was wrong
    /// before.
    #[test]
    fn a_passphrase_is_written_quoted_for_wpa_supplicant_to_derive() {
        let conf = supplicant_config("Example_Network", "correcthorsebattery", false).unwrap();
        assert_eq!(
            conf,
            "network={\n\tssid=\"Example_Network\"\n\tpsk=\"correcthorsebattery\"\n}\n"
        );
    }

    /// 64 hex digits is a derived PSK already. Quoting it would make
    /// wpa_supplicant treat the hash itself as the passphrase and hash it
    /// again, so the network would simply never associate.
    #[test]
    fn an_already_derived_psk_is_written_unquoted() {
        let raw = "a".repeat(64);
        let conf = supplicant_config("Net", &raw, true).unwrap();
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
        let injected = supplicant_config("Net\n}\nnetwork={\n\tssid=\"Evil\"", "password123", true);
        assert!(injected.is_err(), "a newline in the SSID must be refused");

        let via_psk = supplicant_config("Net", "pass\n\tkey_mgmt=NONE", true);
        assert!(
            via_psk.is_err(),
            "a newline in the password must be refused"
        );

        let quoted = supplicant_config("Net", "pass\"word\"here", true);
        assert!(quoted.is_err(), "a double quote must be refused");
    }

    #[test]
    fn a_passphrase_wpa_cannot_use_is_refused_before_the_radio_is_touched() {
        assert!(
            supplicant_config("Net", "short", true).is_err(),
            "under 8 characters"
        );
        assert!(
            supplicant_config("Net", &"x".repeat(64), true).is_err(),
            "over 63 characters"
        );
        assert!(supplicant_config("Net", "", true).is_err(), "empty");
        assert!(
            supplicant_config("", "password123", true).is_err(),
            "empty SSID"
        );
        assert!(
            supplicant_config(&"n".repeat(33), "password123", true).is_err(),
            "an SSID cannot exceed 32 characters"
        );
    }

    #[test]
    fn an_ordinary_password_with_punctuation_is_accepted() {
        // The parser already allows = # and spaces in a password; the config
        // writer must not quietly disagree with it.
        let conf = supplicant_config("Net", "a b=c#d!$%", true).unwrap();
        assert!(conf.contains("psk=\"a b=c#d!$%\"\n"), "{}", conf);
    }
}
