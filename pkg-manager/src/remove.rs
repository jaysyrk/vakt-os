//! Taking a package back off the system.
//!
//! Removal deletes exactly the paths the install recorded, and nothing else.
//! Two properties matter, this being the one command that destroys data:
//!
//! * **Every path is checked before it is touched.** Recorded paths came out of
//!   an archive off the network, so absolute paths and `..` are rejected rather
//!   than resolved - a crafted package cannot record `../../etc/passwd`.
//! * **Nothing follows a symlink.** `remove_file` unlinks the link itself and
//!   directories go only when already empty, so a package shipping a symlink to
//!   `/etc` deletes the link and never what it points at.

use crate::db::{DB_DIR, Database};
use anyhow::{Result, bail};
use std::path::{Component, Path, PathBuf};

/// What a removal did, so the caller can report it without re-deriving it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Removal {
    pub files: usize,
    pub directories: usize,
    /// Recorded paths that were already gone. Not an error - a file deleted by
    /// hand should not stop the rest of the package being cleaned up.
    pub missing: usize,
    /// Paths that were rejected as unsafe, or that would not delete.
    pub refused: Vec<String>,
}

/// Removes `name` from `root`.
///
/// Refuses while another installed package still depends on it, unless `force`
/// is set - breaking a dependent's requirements should be a decision, not an
/// accident.
pub fn remove_package(name: &str, root: &Path, force: bool) -> Result<Removal> {
    let db = Database::new(root);

    let Some(package) = db.get(name) else {
        bail!("Package '{}' is not installed", name);
    };

    let dependents = db.dependents(name);
    if !dependents.is_empty() && !force {
        bail!(
            "'{}' is required by {}. Remove {} first, or pass --force.",
            name,
            dependents.join(", "),
            if dependents.len() == 1 { "it" } else { "them" }
        );
    }

    let mut removal = Removal::default();
    let db_dir = root.join(DB_DIR);

    // Deepest paths first, so a directory is only considered once everything
    // the package put inside it has already gone.
    let mut paths = package.files.clone();
    paths.sort_by_key(|p| std::cmp::Reverse(p.matches('/').count()));

    for recorded in &paths {
        let target = match safe_target(root, recorded) {
            Ok(target) => target,
            Err(e) => {
                removal.refused.push(format!("{}: {}", recorded, e));
                continue;
            }
        };

        // The database is not the package's to delete, whatever it recorded.
        if target.starts_with(&db_dir) {
            removal
                .refused
                .push(format!("{}: inside the package database", recorded));
            continue;
        }

        let Ok(metadata) = std::fs::symlink_metadata(&target) else {
            removal.missing += 1;
            continue;
        };

        if metadata.is_dir() {
            // Only if empty. A directory this package created but that now
            // holds another package's files stays, and that is the ordinary
            // case for /usr/bin - not worth reporting.
            if std::fs::remove_dir(&target).is_ok() {
                removal.directories += 1;
            }
        } else {
            match std::fs::remove_file(&target) {
                Ok(()) => removal.files += 1,
                Err(e) => removal.refused.push(format!("{}: {}", recorded, e)),
            }
        }
    }

    // Directories the archive never listed explicitly, but that only exist
    // because this package's files were in them.
    for recorded in &paths {
        if let Ok(target) = safe_target(root, recorded) {
            prune_empty_parents(&target, root, &db_dir, &mut removal);
        }
    }

    db.forget(name)?;
    Ok(removal)
}

/// Reduces a path to a plain relative one, rejecting anything that could point
/// outside the install root.
///
/// Lexical on purpose: canonicalising would resolve symlinks, and a symlink
/// planted by the package is what this defends against. Install and remove both
/// go through it, so nothing can be recorded that removal would refuse to act
/// on. An empty result means `.`, the archive root - legal, but not a file.
pub fn safe_relative(recorded: &Path) -> Result<PathBuf> {
    let mut relative = PathBuf::new();

    for component in recorded.components() {
        match component {
            Component::Normal(part) => relative.push(part),
            // "./usr/bin" is how tar spells a top-level entry; harmless.
            Component::CurDir => {}
            Component::ParentDir => bail!("path escapes the install root"),
            Component::RootDir | Component::Prefix(_) => bail!("path is absolute"),
        }
    }
    Ok(relative)
}

/// Resolves a recorded path against the install root.
fn safe_target(root: &Path, recorded: &str) -> Result<PathBuf> {
    let relative = safe_relative(Path::new(recorded))?;
    if relative.as_os_str().is_empty() {
        // The install root itself is not something a package may remove.
        bail!("path is the install root");
    }
    Ok(root.join(relative))
}

/// Walks up from a removed path deleting directories that are now empty,
/// stopping at the install root.
fn prune_empty_parents(target: &Path, root: &Path, db_dir: &Path, removal: &mut Removal) {
    let mut current = target.parent();

    while let Some(directory) = current {
        if directory == root || !directory.starts_with(root) || directory.starts_with(db_dir) {
            return;
        }
        if std::fs::remove_dir(directory).is_err() {
            // Not empty, or already gone. Either way there is nothing above it
            // that could be empty either.
            return;
        }
        removal.directories += 1;
        current = directory.parent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::InstalledPackage;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zrpkg-remove-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Lays down a package's files and its database record.
    fn install(root: &Path, name: &str, files: &[&str], dependencies: &[&str]) {
        for file in files {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, format!("contents of {}", file)).unwrap();
        }
        Database::new(root)
            .record(&InstalledPackage {
                name: name.to_string(),
                version: "1.0.0".to_string(),
                dependencies: dependencies.iter().map(|s| s.to_string()).collect(),
                files: files.iter().map(|s| s.to_string()).collect(),
                installed_at: 0,
            })
            .unwrap();
    }

    #[test]
    fn removes_every_file_the_package_installed() {
        let root = scratch("basic");
        install(
            &root,
            "vakt-audit",
            &["usr/bin/vakt-audit", "usr/share/vakt-audit/rules.txt"],
            &[],
        );

        let removal = remove_package("vakt-audit", &root, false).unwrap();

        assert_eq!(removal.files, 2);
        assert!(removal.refused.is_empty());
        assert!(!root.join("usr/bin/vakt-audit").exists());
        assert!(
            !root.join("usr/share/vakt-audit").exists(),
            "empty dirs should be pruned"
        );
        assert_eq!(Database::new(&root).get("vakt-audit"), None);
    }

    /// Another package's files in a shared directory must survive.
    #[test]
    fn shared_directories_are_left_alone() {
        let root = scratch("shared");
        install(&root, "vakt-audit", &["usr/bin/vakt-audit"], &[]);
        install(&root, "vakt-ids", &["usr/bin/vakt-ids"], &[]);

        remove_package("vakt-audit", &root, false).unwrap();

        assert!(!root.join("usr/bin/vakt-audit").exists());
        assert!(
            root.join("usr/bin/vakt-ids").exists(),
            "the other package must be intact"
        );
        assert!(
            root.join("usr/bin").is_dir(),
            "a directory still in use must stay"
        );
    }

    #[test]
    fn refuses_while_something_still_depends_on_it() {
        let root = scratch("dependents");
        install(&root, "libvakt", &["usr/lib/libvakt.so"], &[]);
        install(&root, "vakt-audit", &["usr/bin/vakt-audit"], &["libvakt"]);

        let error = remove_package("libvakt", &root, false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("vakt-audit"), "got: {}", error);
        assert!(
            root.join("usr/lib/libvakt.so").exists(),
            "nothing should have been deleted"
        );

        // The user can still insist.
        remove_package("libvakt", &root, true).unwrap();
        assert!(!root.join("usr/lib/libvakt.so").exists());
    }

    #[test]
    fn removing_something_that_is_not_installed_is_an_error() {
        let root = scratch("absent");
        let error = remove_package("ghost", &root, false)
            .unwrap_err()
            .to_string();
        assert_eq!(error, "Package 'ghost' is not installed");
    }

    #[test]
    fn a_file_already_deleted_by_hand_is_not_fatal() {
        let root = scratch("partial");
        install(
            &root,
            "vakt-audit",
            &["usr/bin/vakt-audit", "usr/bin/helper"],
            &[],
        );
        std::fs::remove_file(root.join("usr/bin/helper")).unwrap();

        let removal = remove_package("vakt-audit", &root, false).unwrap();
        assert_eq!(removal.files, 1);
        assert_eq!(removal.missing, 1);
        assert_eq!(Database::new(&root).get("vakt-audit"), None);
    }

    /// The whole reason paths are validated rather than trusted.
    #[test]
    fn a_recorded_path_cannot_reach_outside_the_install_root() {
        let root = scratch("escape");
        let outside = root.parent().unwrap().join("zrpkg-escape-victim");
        std::fs::write(&outside, b"must survive").unwrap();

        install(&root, "evil", &["usr/bin/evil"], &[]);
        // Rewrite the record the way a malicious package would like it to read.
        let db = Database::new(&root);
        let mut package = db.get("evil").unwrap();
        package.files = vec![
            "../zrpkg-escape-victim".to_string(),
            "/etc/passwd".to_string(),
            "usr/../../zrpkg-escape-victim".to_string(),
            "usr/bin/evil".to_string(),
        ];
        db.record(&package).unwrap();

        let removal = remove_package("evil", &root, false).unwrap();

        assert!(outside.exists(), "a path outside the root was deleted");
        assert_eq!(removal.refused.len(), 3, "got: {:?}", removal.refused);
        assert_eq!(
            removal.files, 1,
            "the legitimate file should still be removed"
        );
        let _ = std::fs::remove_file(&outside);
    }

    /// Deleting a symlink must unlink the link, never the file it names.
    #[test]
    fn symlinked_paths_do_not_reach_their_targets() {
        let root = scratch("symlink");
        let victim = root.join("keep-me");
        std::fs::write(&victim, b"must survive").unwrap();

        std::fs::create_dir_all(root.join("usr/bin")).unwrap();
        std::os::unix::fs::symlink(&victim, root.join("usr/bin/link")).unwrap();
        Database::new(&root)
            .record(&InstalledPackage {
                name: "linker".to_string(),
                version: "1.0.0".to_string(),
                dependencies: vec![],
                files: vec!["usr/bin/link".to_string()],
                installed_at: 0,
            })
            .unwrap();

        let removal = remove_package("linker", &root, false).unwrap();

        assert_eq!(removal.files, 1);
        assert!(!root.join("usr/bin/link").exists());
        assert!(victim.exists(), "the symlink's target was deleted");
    }

    #[test]
    fn the_package_database_is_not_deletable_by_a_package() {
        let root = scratch("db-guard");
        install(&root, "greedy", &["usr/bin/greedy"], &[]);

        let db = Database::new(&root);
        let mut package = db.get("greedy").unwrap();
        package.files.push(format!("{}/greedy.json", DB_DIR));
        package.files.push(DB_DIR.to_string());
        db.record(&package).unwrap();

        let removal = remove_package("greedy", &root, false).unwrap();
        assert_eq!(removal.refused.len(), 2, "got: {:?}", removal.refused);
        assert!(
            root.join(DB_DIR).is_dir(),
            "the database directory was removed"
        );
    }

    #[test]
    fn tar_style_leading_dot_paths_are_accepted() {
        let root = scratch("dotslash");
        assert_eq!(
            safe_target(&root, "./usr/bin/vakt-audit").unwrap(),
            root.join("usr/bin/vakt-audit")
        );
    }
}
