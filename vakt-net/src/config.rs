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
pub fn config_path() -> Option<PathBuf> {
    for candidate in [PERSISTENT_CONF, FALLBACK_CONF] {
        let path = Path::new(candidate);
        if path.is_file() {
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

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let cfg = NetConfig::parse("# comment\n\n  ssid = Cafe \n");
        assert_eq!(cfg.ssid.as_deref(), Some("Cafe"));
    }
}
