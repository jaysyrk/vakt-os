//! Finding a wired interface with a cable in it.
//!
//! The panel can only configure Wi-Fi, so without this a wired-only machine
//! has no supported route online. Gated on a carrier rather than assuming
//! `eth0`, so a machine with no ethernet stays `unconfigured` instead of
//! retrying DHCP on a port that is never coming up.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

const SYS_NET: &str = "/sys/class/net";

/// `carrier` reads 0 while the PHY autonegotiates, so give it a moment.
const CARRIER_SETTLE: Duration = Duration::from_secs(2);

/// Interfaces that could carry a wired link, in a stable order. Takes the
/// sysfs root so it can be tested against a fake tree.
pub fn wired_candidates_in(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };

    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|entry| {
            let path = entry.path();
            // Wireless needs credentials; that is the panel's business.
            !path.join("wireless").exists() && !path.join("phy80211").exists()
        })
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name != "lo")
        .collect();

    names.sort();
    names
}

/// Whether `iface` reports a cable. Unreadable counts as none - `carrier`
/// cannot be read at all while the link is down, so callers bring it up first.
pub fn has_carrier_in(root: &Path, iface: &str) -> bool {
    std::fs::read_to_string(root.join(iface).join("carrier"))
        .map(|s| s.trim() == "1")
        .unwrap_or(false)
}

/// The first wired interface with something plugged into it, if any.
pub fn first_wired_link() -> Option<String> {
    let root = Path::new(SYS_NET);
    let candidates = wired_candidates_in(root);
    if candidates.is_empty() {
        return None;
    }

    for iface in &candidates {
        let _ = Command::new("ip")
            .args(["link", "set", iface, "up"])
            .status();
    }
    std::thread::sleep(CARRIER_SETTLE);

    candidates.into_iter().find(|i| has_carrier_in(root, i))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_sysfs(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("vakt-net-link-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn iface(root: &Path, name: &str, carrier: Option<&str>) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(value) = carrier {
            std::fs::write(dir.join("carrier"), value).unwrap();
        }
    }

    fn wireless(root: &Path, name: &str, marker: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(dir.join(marker)).unwrap();
        std::fs::write(dir.join("carrier"), "1").unwrap();
    }

    #[test]
    fn loopback_and_wireless_are_not_wired_candidates() {
        let root = fake_sysfs("candidates");
        iface(&root, "eth0", Some("1"));
        iface(&root, "enp3s0", Some("0"));
        iface(&root, "lo", Some("1"));
        wireless(&root, "wlan0", "wireless");
        wireless(&root, "wlp2s0", "phy80211");

        assert_eq!(
            wired_candidates_in(&root),
            vec!["enp3s0".to_string(), "eth0".to_string()],
            "only wired interfaces, and never lo"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_carrier_is_required_and_an_unreadable_one_counts_as_none() {
        let root = fake_sysfs("carrier");
        iface(&root, "eth0", Some("1\n"));
        iface(&root, "eth1", Some("0\n"));
        // Down interfaces have no readable carrier at all.
        iface(&root, "eth2", None);

        assert!(has_carrier_in(&root, "eth0"));
        assert!(!has_carrier_in(&root, "eth1"));
        assert!(!has_carrier_in(&root, "eth2"));
        assert!(!has_carrier_in(&root, "does-not-exist"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_machine_with_no_interfaces_offers_nothing() {
        let root = fake_sysfs("empty");
        assert!(wired_candidates_in(&root).is_empty());
        assert!(wired_candidates_in(Path::new("/definitely/not/here")).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
