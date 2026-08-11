use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Preferred location: survives reboots on the persistent disk.
pub const PERSISTENT_CONF: &str = "/persistent/etc/vakt-net.conf";
/// Fallback for RAM-only boots, or an image-baked default.
pub const FALLBACK_CONF: &str = "/etc/vakt-net.conf";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetConfig {
    /// Wi-Fi network name. Absent means "wired / DHCP only".
    pub ssid: Option<String>,
    pub psk: Option<String>,
    pub interface: String,
}

impl Default for NetConfig {
    fn default() -> Self {
        // With no configuration at all we assume QEMU-style wired networking,
        // which is how the VM reaches the zrpkg repo on the host at 10.0.2.2.
        NetConfig {
            ssid: None,
            psk: None,
            interface: "eth0".to_string(),
        }
    }
}

impl NetConfig {
    pub fn is_wireless(&self) -> bool {
        self.ssid.as_ref().is_some_and(|s| !s.is_empty())
    }

    /// Parses `key=value` lines. `#` starts a comment; unknown keys are ignored.
    /// Values are taken verbatim after the first `=` so passwords may contain
    /// `=`, spaces, or `#`.
    pub fn parse(text: &str) -> NetConfig {
        let mut cfg = NetConfig::default();
        let mut saw_interface = false;

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim().to_string();

            match key.as_str() {
                "ssid" if !value.is_empty() => cfg.ssid = Some(value),
                "psk" | "password" if !value.is_empty() => cfg.psk = Some(value),
                "interface" | "iface" if !value.is_empty() => {
                    cfg.interface = value;
                    saw_interface = true;
                }
                _ => {}
            }
        }

        // A Wi-Fi config that never named an interface means wlan0, not eth0.
        if cfg.is_wireless() && !saw_interface {
            cfg.interface = "wlan0".to_string();
        }
        cfg
    }
}

/// Returns the config file in use, preferring the persistent copy.
///
/// An empty file counts as absent, not as an empty configuration. That is
/// what [`ensure_config_placeholder`] leaves behind so the Landlock ruleset
/// can name the path, and treating it as a real config would both shadow the
/// image-baked `FALLBACK_CONF` below it and rob the daemon of its
/// "unconfigured" state - the panel would show a failing wired connection
/// instead of "no network configured".
pub fn config_path() -> Option<PathBuf> {
    for candidate in [PERSISTENT_CONF, FALLBACK_CONF] {
        let path = Path::new(candidate);
        let usable = std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() > 0);
        if usable {
            return Some(path.to_path_buf());
        }
    }
    None
}

/// Loads the config, or `None` when no file exists yet.
pub fn load() -> Option<(PathBuf, NetConfig)> {
    let path = config_path()?;
    let text = std::fs::read_to_string(&path).ok()?;
    Some((path, NetConfig::parse(&text)))
}

/// Modification time of the active config, used to notice edits from the TUI.
pub fn config_stamp() -> Option<SystemTime> {
    let path = config_path()?;
    std::fs::metadata(path).ok()?.modified().ok()
}

/// Ensures a file exists at [`PERSISTENT_CONF`], creating an empty one if not.
///
/// Must run before `sandbox::confine()`: Landlock only grants rules for paths
/// that exist when the ruleset is built, and it can never be widened after. On
/// a fresh appliance the file does not exist yet, so without this the daemon
/// could never read it for the rest of that boot. Failing here leaves the file
/// absent, which `confine`'s existence filter already handles.
pub fn ensure_config_placeholder() {
    ensure_placeholder_at(Path::new(PERSISTENT_CONF));
}

fn ensure_placeholder_at(path: &Path) {
    if path.exists() {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::File::create(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_defaults_to_wired_eth0() {
        let cfg = NetConfig::parse("");
        assert!(!cfg.is_wireless());
        assert_eq!(cfg.interface, "eth0");
    }

    #[test]
    fn wifi_without_explicit_interface_uses_wlan0() {
        let cfg = NetConfig::parse("ssid=HomeNet\npsk=hunter2\n");
        assert_eq!(cfg.ssid.as_deref(), Some("HomeNet"));
        assert_eq!(cfg.interface, "wlan0");
    }

    #[test]
    fn explicit_interface_wins() {
        let cfg = NetConfig::parse("ssid=HomeNet\ninterface=wlan1\n");
        assert_eq!(cfg.interface, "wlan1");
    }

    #[test]
    fn passwords_may_contain_equals_hash_and_spaces() {
        let cfg = NetConfig::parse("ssid=Net\npsk=a=b #c d\n");
        assert_eq!(cfg.psk.as_deref(), Some("a=b #c d"));
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("vakt-net-config-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn ensure_placeholder_creates_missing_dirs_and_an_empty_file() {
        let dir = scratch("missing");
        let path = dir.join("etc").join("vakt-net.conf");

        ensure_placeholder_at(&path);

        assert!(path.is_file());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ensure_placeholder_never_touches_an_existing_file() {
        let dir = scratch("existing");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("vakt-net.conf");
        std::fs::write(&path, "ssid=DoNotClobber\n").unwrap();

        ensure_placeholder_at(&path);

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "ssid=DoNotClobber\n"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let cfg = NetConfig::parse("# comment\n\n  ssid = Cafe \n");
        assert_eq!(cfg.ssid.as_deref(), Some("Cafe"));
    }

    /// The placeholder ensure_config_placeholder() leaves behind must not be
    /// mistaken for a real configuration: doing so would shadow the
    /// image-baked fallback and destroy the daemon's "unconfigured" state.
    /// Mirrors config_path's own precedence rule against a temp directory.
    fn first_usable(candidates: &[&Path]) -> Option<std::path::PathBuf> {
        candidates.iter().find_map(|path| {
            let usable = std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() > 0);
            usable.then(|| path.to_path_buf())
        })
    }

    #[test]
    fn an_empty_placeholder_does_not_shadow_the_fallback() {
        let dir = scratch("shadow");
        std::fs::create_dir_all(&dir).unwrap();
        let persistent = dir.join("persistent.conf");
        let fallback = dir.join("fallback.conf");
        ensure_placeholder_at(&persistent);
        std::fs::write(&fallback, "ssid=BakedIn\n").unwrap();

        assert_eq!(
            first_usable(&[&persistent, &fallback]),
            Some(fallback.clone()),
            "an empty persistent placeholder must fall through to the fallback"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_empty_placeholder_alone_still_counts_as_unconfigured() {
        let dir = scratch("unconfigured");
        std::fs::create_dir_all(&dir).unwrap();
        let persistent = dir.join("persistent.conf");
        let fallback = dir.join("absent.conf");
        ensure_placeholder_at(&persistent);

        assert_eq!(first_usable(&[&persistent, &fallback]), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_real_persistent_config_still_wins_over_the_fallback() {
        let dir = scratch("precedence");
        std::fs::create_dir_all(&dir).unwrap();
        let persistent = dir.join("persistent.conf");
        let fallback = dir.join("fallback.conf");
        std::fs::write(&persistent, "ssid=OnDisk\n").unwrap();
        std::fs::write(&fallback, "ssid=BakedIn\n").unwrap();

        assert_eq!(first_usable(&[&persistent, &fallback]), Some(persistent));
        std::fs::remove_dir_all(&dir).ok();
    }
}
