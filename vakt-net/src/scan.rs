//! What is in range, for the panel's network picker.
//!
//! The panel cannot scan for itself: it runs unprivileged and has no control
//! socket. This writes what the supplicant found to a file it can read.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

pub const SCAN_FILE: &str = "/run/vakt-net.scan";
pub const CTRL_DIR: &str = "/run/wpa_supplicant";

/// How long the radio gets to sweep every channel before results are read.
const SETTLE: Duration = Duration::from_secs(4);

pub struct Network {
    pub ssid: String,
    pub signal: i32,
    pub freq: i32,
    pub security: String,
}

/// One `scan_results` row: bssid, frequency, signal, flags, ssid.
fn parse_row(line: &str) -> Option<Network> {
    let fields: Vec<&str> = line.split('\t').collect();
    if fields.len() < 5 {
        return None;
    }

    let ssid = fields[4].trim();
    // A network that does not broadcast its name cannot be picked from a list;
    // the panel's SSID field is what that case is for.
    if ssid.is_empty() {
        return None;
    }

    Some(Network {
        ssid: ssid.to_string(),
        signal: fields[2].trim().parse().ok()?,
        freq: fields[1].trim().parse().ok()?,
        security: security_from(fields[3]),
    })
}

/// Reduces a flag string like `[WPA2-PSK-CCMP][ESS]` to one word.
fn security_from(flags: &str) -> String {
    if flags.contains("SAE") {
        "WPA3"
    } else if flags.contains("WPA2") {
        "WPA2"
    } else if flags.contains("WPA") {
        "WPA"
    } else if flags.contains("WEP") {
        "WEP"
    } else {
        "open"
    }
    .to_string()
}

pub fn parse_results(text: &str) -> Vec<Network> {
    text.lines().skip(1).filter_map(parse_row).collect()
}

pub fn render(networks: &[Network]) -> String {
    networks
        .iter()
        .map(|n| format!("{}\t{}\t{}\t{}\n", n.ssid, n.signal, n.freq, n.security))
        .collect()
}

/// Asks the supplicant to scan, then publishes what it heard.
///
/// Returns how many networks were written. Failure is not worth failing a
/// connection over - the picker simply has nothing new to show.
pub fn refresh(interface: &str) -> Result<usize, String> {
    let scan = wpa_cli(interface, "scan")?;
    if scan.contains("FAIL") {
        return Err("the supplicant refused to scan".to_string());
    }
    std::thread::sleep(SETTLE);

    let networks = parse_results(&wpa_cli(interface, "scan_results")?);
    publish(&render(&networks))?;
    Ok(networks.len())
}

fn wpa_cli(interface: &str, command: &str) -> Result<String, String> {
    let out = Command::new("wpa_cli")
        .args(["-p", CTRL_DIR, "-i", interface, command])
        .output()
        .map_err(|e| format!("wpa_cli unavailable: {}", e))?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// World-readable on purpose: the panel runs as another account, and a list of
/// network names already being broadcast over the air is not a secret.
fn publish(body: &str) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(SCAN_FILE, body).map_err(|e| format!("cannot write {}: {}", SCAN_FILE, e))?;
    std::fs::set_permissions(Path::new(SCAN_FILE), std::fs::Permissions::from_mode(0o644))
        .map_err(|e| format!("cannot set permissions on {}: {}", SCAN_FILE, e))
}

/// A supplicant with no network block, so an appliance that has never been
/// configured can still show what is in range.
pub fn scanner_config() -> String {
    format!("ctrl_interface={}\n", CTRL_DIR)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "bssid / frequency / signal level / flags / ssid\n\
02:00:00:00:00:00\t2412\t-30\t[WPA2-PSK-CCMP][ESS]\tVaktTest\n\
02:00:00:00:01:00\t5180\t-56\t[WPA2-PSK-CCMP][WPA2-SAE-CCMP][ESS]\tStykezone\n\
02:00:00:00:02:00\t2437\t-71\t[ESS]\tcoffeeshop\n";

    #[test]
    fn the_header_row_is_not_a_network() {
        let found = parse_results(SAMPLE);
        assert_eq!(found.len(), 3);
        assert!(found.iter().all(|n| n.ssid != "bssid / frequency"));
    }

    #[test]
    fn fields_land_in_the_right_columns() {
        let found = parse_results(SAMPLE);
        assert_eq!(found[0].ssid, "VaktTest");
        assert_eq!(found[0].signal, -30);
        assert_eq!(found[0].freq, 2412);
        assert_eq!(found[0].security, "WPA2");
    }

    /// An access point offering both advertises WPA2 and SAE; calling that
    /// WPA2 would tell the operator the network is weaker than it is.
    #[test]
    fn sae_wins_over_wpa2_in_the_same_flags() {
        assert_eq!(security_from("[WPA2-PSK-CCMP][WPA2-SAE-CCMP][ESS]"), "WPA3");
    }

    #[test]
    fn an_open_network_says_so() {
        let found = parse_results(SAMPLE);
        assert_eq!(found[2].security, "open");
    }

    #[test]
    fn hidden_networks_are_left_out() {
        let hidden = "bssid / frequency / signal level / flags / ssid\n\
02:00:00:00:00:00\t2412\t-30\t[WPA2-PSK-CCMP][ESS]\t\n";
        assert!(parse_results(hidden).is_empty());
    }

    #[test]
    fn a_truncated_row_is_skipped_rather_than_guessed() {
        let broken = "header\n02:00:00:00:00:00\t2412\t-30\n";
        assert!(parse_results(broken).is_empty());
    }

    #[test]
    fn the_rendered_line_is_what_the_panel_parses() {
        let found = parse_results(SAMPLE);
        assert_eq!(
            render(&found[..1]),
            "VaktTest\t-30\t2412\tWPA2\n".to_string()
        );
    }
}
