//! The record of what is installed.
//!
//! Every install leaves a file under `var/lib/zrpkg/` listing each path it
//! created, and `zrpkg remove` works entirely from that list. Nothing is
//! deleted because it looks like it belongs to a package - only because this
//! database says the package put it there.
//!
//! It lives inside the install root rather than `/etc` so it travels with the
//! packages it describes, and a wiped disk leaves no stale record.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Location of the database, relative to the install root.
pub const DB_DIR: &str = "var/lib/zrpkg";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstalledPackage {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    /// Every path the archive produced, relative to the install root, in the
    /// order it was unpacked.
    pub files: Vec<String>,
    /// Unix seconds, so two installs of the same version can be told apart.
    #[serde(default)]
    pub installed_at: u64,
}

pub struct Database {
    root: PathBuf,
}

impl Database {
    pub fn new(root: &Path) -> Self {
        Database {
            root: root.to_path_buf(),
        }
    }

    pub fn directory(&self) -> PathBuf {
        self.root.join(DB_DIR)
    }

    fn entry_path(&self, name: &str) -> PathBuf {
        self.directory().join(format!("{}.json", name))
    }

    /// Writes the record atomically: a half-written entry would describe a
    /// package that cannot be cleanly removed.
    pub fn record(&self, package: &InstalledPackage) -> Result<()> {
        let dir = self.directory();
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create {}", dir.display()))?;

        let path = self.entry_path(&package.name);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(package)?)
            .with_context(|| format!("Failed to write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("Failed to install record for {}", package.name))?;
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<InstalledPackage> {
        let data = std::fs::read(self.entry_path(name)).ok()?;
        serde_json::from_slice(&data).ok()
    }

    /// Every recorded package, sorted by name. Unreadable or corrupt entries
    /// are skipped rather than fatal: one bad record must not make the whole
    /// database unusable.
    pub fn installed(&self) -> Vec<InstalledPackage> {
        let Ok(entries) = std::fs::read_dir(self.directory()) else {
            return Vec::new();
        };

        let mut packages: Vec<InstalledPackage> = entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().extension().is_some_and(|e| e == "json"))
            .filter_map(|entry| std::fs::read(entry.path()).ok())
            .filter_map(|data| serde_json::from_slice(&data).ok())
            .collect();

        packages.sort_by(|a, b| a.name.cmp(&b.name));
        packages
    }

    /// Installed packages that depend on `name`, and would break if it went
    /// away.
    pub fn dependents(&self, name: &str) -> Vec<String> {
        self.installed()
            .into_iter()
            .filter(|p| p.dependencies.iter().any(|d| d == name))
            .map(|p| p.name)
            .collect()
    }

    pub fn forget(&self, name: &str) -> Result<()> {
        let path = self.entry_path(name);
        if path.exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("Failed to remove {}", path.display()))?;
        }
        Ok(())
    }
}

/// Seconds since the epoch, or zero on a system with no usable clock.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zrpkg-db-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn package(name: &str, dependencies: &[&str]) -> InstalledPackage {
        InstalledPackage {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            dependencies: dependencies.iter().map(|s| s.to_string()).collect(),
            files: vec![format!("usr/bin/{}", name)],
            installed_at: now(),
        }
    }

    #[test]
    fn a_recorded_package_can_be_read_back() {
        let root = scratch("roundtrip");
        let db = Database::new(&root);
        let vakt_audit = package("vakt-audit", &["libvakt"]);

        db.record(&vakt_audit).unwrap();
        assert_eq!(db.get("vakt-audit"), Some(vakt_audit));
    }

    #[test]
    fn an_unknown_package_is_not_installed() {
        let db = Database::new(&scratch("unknown"));
        assert_eq!(db.get("nothing"), None);
        assert!(db.installed().is_empty());
    }

    #[test]
    fn dependents_are_found_by_reverse_lookup() {
        let root = scratch("dependents");
        let db = Database::new(&root);
        db.record(&package("libvakt", &[])).unwrap();
        db.record(&package("vakt-audit", &["libvakt"])).unwrap();
        db.record(&package("vakt-ids", &["libvakt"])).unwrap();

        let mut dependents = db.dependents("libvakt");
        dependents.sort();
        assert_eq!(dependents, vec!["vakt-audit", "vakt-ids"]);
        assert!(db.dependents("vakt-audit").is_empty());
    }

    #[test]
    fn forgetting_a_package_removes_it_from_the_listing() {
        let root = scratch("forget");
        let db = Database::new(&root);
        db.record(&package("vakt-audit", &[])).unwrap();
        db.forget("vakt-audit").unwrap();

        assert_eq!(db.get("vakt-audit"), None);
        assert!(db.installed().is_empty());
        // Forgetting something that was never there is not an error.
        db.forget("vakt-audit").unwrap();
    }

    /// One corrupt record must not take the rest of the database with it.
    #[test]
    fn corrupt_entries_are_skipped() {
        let root = scratch("corrupt");
        let db = Database::new(&root);
        db.record(&package("good", &[])).unwrap();
        std::fs::write(db.directory().join("bad.json"), b"{not json").unwrap();

        let installed = db.installed();
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].name, "good");
    }
}
