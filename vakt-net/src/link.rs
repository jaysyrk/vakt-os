//! Finding a wired interface with a cable in it.
//!
//! An appliance with an ethernet cable and no configuration should reach the
//! network. The panel's only network page is Wi-Fi Setup and it refuses an
//! empty SSID, so without this a wired-only machine has no supported way to
//! get online at all - the operator would have to know to hand-write
//! `vakt-net.conf` from a root shell, which nothing tells them to do.
//!
//! Gated on an actual carrier rather than assuming `eth0` exists: a machine
//! with no ethernet at all should stay `unconfigured`, which is both what the
//! runbook says that state means and quieter than retrying DHCP forever on an
//! interface that is never coming up.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

const SYS_NET: &str = "/sys/class/net";

/// How long to let a link settle after bringing it up. `carrier` reads 0 for
/// a moment while the PHY autonegotiates, so checking immediately would find
/// nothing on a cable that is plainly connected.
const CARRIER_SETTLE: Duration = Duration::from_secs(2);

/// Interfaces that could carry a wired link, in a stable order.
///
/// Split from the sysfs root so it can be tested against a fake tree rather
/// than whatever the build machine happens to have plugged in.
pub fn wired_candidates_in(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };

    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|entry| {
            let path = entry.path();
            // Wireless interfaces carry their own configuration requirements;
            // they are the panel's business, not this fallback's. The kernel
            // marks them with either of these, depending on the driver's age.
            !path.join("wireless").exists() && !path.join("phy80211").exists()
        })
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name != "lo")
        .collect();

    names.sort();
    names
}

/// Whether `iface` reports a cable.
///
/// Reading `carrier` on a down interface fails outright, which is why callers
/// bring the link up first - an unreadable carrier is reported as no cable
/// rather than guessed at.
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

    /// A cable is the whole point: an interface that exists but has nothing
    /// plugged in must not be treated as a network, or a Wi-Fi-only machine
    /// spends forever retrying DHCP on a dead port.
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
