//! Fetching, verifying, and unpacking packages.
//!
//! Trust is not optional here. An earlier version of this installer warned when
//! no trust anchor was present and installed anyway, which is the worst of both
//! worlds: the warning scrolls past on a console nobody is reading, and the
//! result is an unsigned binary running as a system service. There is now
//! exactly one path through this module, and it goes through signature
//! verification. A package that cannot be verified is not installed, and the
//! download is deleted rather than left on disk for someone to run by hand.

use crate::config;
use crate::db::{Database, InstalledPackage, now};
use crate::fetch::{download_package, verify_signature};
use crate::manifest::PackageManifest;
use crate::remove::safe_relative;
use crate::resolve;
use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use reqwest::Client;
use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use tar::Archive;

/// Path to the hex-encoded ed25519 public key this system trusts for packages.
const TRUSTED_KEY_PATH: &str = "/etc/vakt/trusted.key";

/// Guards against a repository that publishes a graph deep enough to be an
/// attack in itself. Nothing legitimate here is more than a few levels deep.
const MAX_RESOLUTION_ROUNDS: usize = 32;

/// The ed25519 public key packages must be signed with.
///
/// There is no fallback. If the image has no trust anchor then nothing about a
/// downloaded archive can be established, and the only safe thing to report is
/// that installing is not possible.
fn trusted_public_key() -> Result<String> {
    if let Ok(key) = std::env::var("ZRPKG_PUBKEY") {
        let key = key.trim().to_string();
        if !key.is_empty() {
            return Ok(key);
        }
    }

    let key = std::fs::read_to_string(TRUSTED_KEY_PATH)
        .with_context(|| format!("No trusted signing key at {}", TRUSTED_KEY_PATH))?
        .trim()
        .to_string();

    if key.is_empty() {
        bail!(
            "{} is empty; there is no key to verify packages against",
            TRUSTED_KEY_PATH
        );
    }
    Ok(key)
}

pub struct Installer {
    client: Client,
    repo_url: String,
    root: PathBuf,
    database: Database,
}

impl Installer {
    pub fn new(root: &Path) -> Self {
        let repository = config::load();
        Installer {
            client: Client::new(),
            repo_url: repository.repo_url,
            root: root.to_path_buf(),
            database: Database::new(root),
        }
    }

    pub fn repository(&self) -> &str {
        &self.repo_url
    }

    /// Installs `requested` and everything they depend on.
    pub async fn install(&self, requested: &[String]) -> Result<()> {
        let key = trusted_public_key()?;

        println!("Resolving dependencies...");
        let manifests = self.collect_manifests(requested).await?;
        let order = resolve::install_order(requested, &manifests)?;

        let planned: Vec<&PackageManifest> = order
            .iter()
            .filter_map(|name| manifests.get(name))
            .filter(|manifest| !self.already_current(manifest))
            .collect();

        if planned.is_empty() {
            println!("Nothing to do; everything requested is already installed.");
            return Ok(());
        }

        if planned.len() > requested.len() {
            println!(
                "Installing {} package(s): {}",
                planned.len(),
                planned
                    .iter()
                    .map(|m| m.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }

        let total = planned.len();
        for (index, manifest) in planned.iter().enumerate() {
            println!(
                "\n[{}/{}] {} {}",
                index + 1,
                total,
                manifest.name,
                manifest.version
            );
            self.install_one(manifest, &key).await?;
        }

        println!("\nDone.");
        Ok(())
    }

    /// Downloads and checks a package without installing it.
    pub async fn verify(&self, name: &str) -> Result<()> {
        let key = trusted_public_key()?;
        let manifest = self.fetch_manifest(name).await?;

        let staged = self.download(&manifest.name).await?;
        let result = self.check_signature(&staged, &manifest, &key).await;
        let _ = tokio::fs::remove_file(&staged).await;
        result?;

        println!(
            "{} {} is signed by the trusted repository key.",
            manifest.name, manifest.version
        );
        if !manifest.dependencies.is_empty() {
            println!("Depends on: {}", manifest.dependencies.join(", "));
        }
        Ok(())
    }

    /// Whether the installed copy is already the version the repository offers.
    fn already_current(&self, manifest: &PackageManifest) -> bool {
        match self.database.get(&manifest.name) {
            Some(installed) if installed.version == manifest.version => {
                println!(
                    "{} {} is already installed.",
                    manifest.name, manifest.version
                );
                true
            }
            _ => false,
        }
    }

    /// Walks the dependency graph one level at a time, fetching the manifests
    /// it has not seen yet, until nothing new is named.
    ///
    /// A cycle in the repository terminates this loop harmlessly - each package
    /// is fetched once - and is reported properly by the topological sort
    /// afterwards.
    async fn collect_manifests(
        &self,
        roots: &[String],
    ) -> Result<BTreeMap<String, PackageManifest>> {
        let mut manifests: BTreeMap<String, PackageManifest> = BTreeMap::new();

        for _ in 0..MAX_RESOLUTION_ROUNDS {
            let wanted = resolve::undiscovered(roots, &manifests);
            if wanted.is_empty() {
                return Ok(manifests);
            }
            for name in wanted {
                let manifest = self.fetch_manifest(&name).await?;
                manifests.insert(name, manifest);
            }
        }

        bail!(
            "Dependency graph is more than {} levels deep; refusing to keep fetching",
            MAX_RESOLUTION_ROUNDS
        )
    }

    async fn fetch_manifest(&self, name: &str) -> Result<PackageManifest> {
        let url = format!("{}/{}.json", self.repo_url, name);
        let body = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("Failed to reach the repository for {}", name))?
            .error_for_status()
            .with_context(|| format!("Repository has no manifest for '{}'", name))?
            .text()
            .await?;

        let manifest = PackageManifest::parse(&body)
            .with_context(|| format!("Malformed manifest for {}", name))?;

        // The manifest names the package it describes; a repository that
        // answers /a.json with a manifest for b would otherwise get b's
        // signature checked against a's archive.
        if manifest.name != name {
            bail!(
                "Repository returned a manifest for '{}' when asked for '{}'",
                manifest.name,
                name
            );
        }
        Ok(manifest)
    }

    async fn install_one(&self, manifest: &PackageManifest, key: &str) -> Result<()> {
        let staged = self.download(&manifest.name).await?;

        if let Err(e) = self.check_signature(&staged, manifest, key).await {
            let _ = tokio::fs::remove_file(&staged).await;
            return Err(e);
        }
        println!("  Signature OK.");

        std::fs::create_dir_all(&self.root)
            .with_context(|| format!("Failed to create {}", self.root.display()))?;

        let files = unpack_recording(&staged, &self.root)
            .with_context(|| format!("Failed to unpack {}", manifest.name))?;
        let _ = tokio::fs::remove_file(&staged).await;

        self.database.record(&InstalledPackage {
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            dependencies: manifest.dependencies.clone(),
            files,
            installed_at: now(),
        })?;

        println!("  Installed into {}.", self.root.display());
        Ok(())
    }

    async fn download(&self, name: &str) -> Result<PathBuf> {
        let staging = Path::new("/tmp");
        std::fs::create_dir_all(staging).context("Failed to create /tmp")?;
        let staged = staging.join(format!("{}.zrp", name));

        println!("  Fetching {}.zrp...", name);
        download_package(
            &self.client,
            &format!("{}/{}.zrp", self.repo_url, name),
            &staged,
        )
        .await
        .with_context(|| format!("Failed to download {}", name))?;

        Ok(staged)
    }

    async fn check_signature(
        &self,
        staged: &Path,
        manifest: &PackageManifest,
        key: &str,
    ) -> Result<()> {
        let data = tokio::fs::read(staged).await?;
        verify_signature(&data, &manifest.signature, key).with_context(|| {
            format!(
                "Refusing to install {}: the archive is not signed by the trusted repository key",
                manifest.name
            )
        })
    }
}

/// Unpacks an archive, returning every path it created.
///
/// Entries are extracted one at a time rather than with `Archive::unpack` so
/// there is a list to hand to the database - and so each path is checked
/// against the same rule `zrpkg remove` will apply to it later. An entry that
/// fails the check aborts the install rather than being skipped: a package that
/// contains a traversal attempt is not a package worth having half of.
fn unpack_recording(archive_path: &Path, target: &Path) -> Result<Vec<String>> {
    let file = File::open(archive_path).context("Failed to open the downloaded archive")?;
    let mut archive = Archive::new(GzDecoder::new(file));
    archive.set_overwrite(true);

    let mut recorded = Vec::new();
    for entry in archive.entries().context("Archive is not readable")? {
        let mut entry = entry.context("Archive entry is corrupt")?;
        let path = entry
            .path()
            .context("Archive entry has an unreadable path")?
            .into_owned();

        let relative = safe_relative(&path)
            .with_context(|| format!("Refusing archive entry '{}'", path.display()))?;

        if !entry
            .unpack_in(target)
            .with_context(|| format!("Failed to unpack '{}'", path.display()))?
        {
            bail!("Archive entry '{}' was rejected as unsafe", path.display());
        }

        // `zrpkg pack` writes the packed directory as a `./` entry. It is the
        // install root, which belongs to no package and must never be recorded
        // as something removal could delete.
        if relative.as_os_str().is_empty() {
            continue;
        }

        recorded.push(relative.to_string_lossy().into_owned());
    }

    Ok(recorded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::sync::Mutex;

    /// See the identical lock in config.rs: cargo test runs tests on separate
    /// threads by default, and both tests below touch the process-global
    /// ZRPKG_PUBKEY - without this they can race each other's set/remove.
    static ZRPKG_PUBKEY_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("zrpkg-install-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Builds a .zrp containing the given (path, contents) pairs.
    ///
    /// Entry names are written straight into the header rather than through
    /// `append_data`, which refuses to produce a `..` or absolute path. Half
    /// the tests here are about what happens when a repository serves an
    /// archive that a well-behaved packer would never have written, so the
    /// packer's own checks have to be out of the way.
    fn archive(path: &Path, entries: &[(&str, &[u8])]) {
        let encoder = GzEncoder::new(File::create(path).unwrap(), Compression::fast());
        let mut builder = tar::Builder::new(encoder);

        for (name, contents) in entries {
            let mut header = tar::Header::new_gnu();
            let raw = name.as_bytes();
            assert!(
                raw.len() < 100,
                "test entry names must fit an old-style header"
            );
            header.as_old_mut().name[..raw.len()].copy_from_slice(raw);
            header.set_size(contents.len() as u64);
            header.set_mode(0o755);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_cksum();
            builder.append(&header, *contents).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
    }

    #[test]
    fn unpacking_records_every_path_it_creates() {
        let dir = scratch("record");
        let zrp = dir.join("pkg.zrp");
        let root = dir.join("root");
        std::fs::create_dir_all(&root).unwrap();

        archive(
            &zrp,
            &[
                ("usr/bin/tool", b"binary"),
                ("usr/share/tool/data", b"data"),
            ],
        );
        let files = unpack_recording(&zrp, &root).unwrap();

        assert_eq!(files, vec!["usr/bin/tool", "usr/share/tool/data"]);
        assert_eq!(std::fs::read(root.join("usr/bin/tool")).unwrap(), b"binary");
    }

    #[test]
    fn tar_style_leading_dots_are_recorded_without_them() {
        let dir = scratch("dots");
        let zrp = dir.join("pkg.zrp");
        let root = dir.join("root");
        std::fs::create_dir_all(&root).unwrap();

        archive(&zrp, &[("./usr/bin/tool", b"binary")]);
        assert_eq!(unpack_recording(&zrp, &root).unwrap(), vec!["usr/bin/tool"]);
    }

    /// `zrpkg pack` writes the directory it packed as a `./` entry. Recording
    /// it would put the install root itself on the package's removal list.
    #[test]
    fn the_archive_root_entry_is_not_recorded() {
        let dir = scratch("archive-root");
        let zrp = dir.join("pkg.zrp");
        let root = dir.join("root");
        std::fs::create_dir_all(&root).unwrap();

        archive(&zrp, &[("./", b""), ("./usr/bin/tool", b"binary")]);
        assert_eq!(unpack_recording(&zrp, &root).unwrap(), vec!["usr/bin/tool"]);
    }

    /// An archive that tries to write outside the install root must fail the
    /// whole install, not quietly skip the entry.
    #[test]
    fn a_traversal_entry_aborts_the_install() {
        let dir = scratch("traversal");
        let zrp = dir.join("evil.zrp");
        let root = dir.join("root");
        std::fs::create_dir_all(&root).unwrap();

        archive(&zrp, &[("../escaped", b"should never be written")]);
        let error = unpack_recording(&zrp, &root).unwrap_err().to_string();

        assert!(error.contains("Refusing archive entry"), "got: {}", error);
        assert!(
            !dir.join("escaped").exists(),
            "a file was written outside the root"
        );
    }

    #[test]
    fn an_absolute_entry_aborts_the_install() {
        let dir = scratch("absolute");
        let zrp = dir.join("evil.zrp");
        let root = dir.join("root");
        std::fs::create_dir_all(&root).unwrap();

        archive(&zrp, &[("/etc/passwd", b"root::0:0::/:/bin/sh")]);
        assert!(unpack_recording(&zrp, &root).is_err());
    }

    /// Builds a .zrp whose first entry is a symlink and whose second writes
    /// through it. Neither entry *name* contains `..` or a leading `/`, so
    /// `safe_relative`'s lexical check passes both - an escape here would come
    /// entirely from the filesystem following the link during extraction.
    fn archive_with_symlink(path: &Path, link: &str, target: &str, then: (&str, &[u8])) {
        let encoder = GzEncoder::new(File::create(path).unwrap(), Compression::fast());
        let mut builder = tar::Builder::new(encoder);

        let mut header = tar::Header::new_gnu();
        let raw = link.as_bytes();
        header.as_old_mut().name[..raw.len()].copy_from_slice(raw);
        let raw_target = target.as_bytes();
        header.as_old_mut().linkname[..raw_target.len()].copy_from_slice(raw_target);
        header.set_size(0);
        header.set_mode(0o777);
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_cksum();
        builder.append(&header, std::io::empty()).unwrap();

        let (name, contents) = then;
        let mut header = tar::Header::new_gnu();
        let raw = name.as_bytes();
        header.as_old_mut().name[..raw.len()].copy_from_slice(raw);
        header.set_size(contents.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        builder.append(&header, contents).unwrap();

        builder.into_inner().unwrap().finish().unwrap();
    }

    /// The classic tar extraction attack: plant a symlink pointing out of the
    /// install root, then write "through" it with a later entry whose own name
    /// looks innocent. Nothing may appear outside the root, whether the
    /// install aborts or the entry is refused. Currently upheld by the tar
    /// crate's own `unpack_in` containment rather than by `safe_relative`,
    /// which is exactly why it is pinned here: a future change of extraction
    /// method would otherwise reopen it silently.
    #[test]
    fn a_symlink_cannot_be_used_to_write_outside_the_root() {
        let dir = scratch("symlink-escape");
        let zrp = dir.join("evil.zrp");
        let root = dir.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let outside = dir.join("outside");
        std::fs::create_dir_all(&outside).unwrap();

        archive_with_symlink(
            &zrp,
            "escape",
            "../outside",
            ("escape/pwned", b"written outside the install root"),
        );
        let result = unpack_recording(&zrp, &root);

        assert!(
            !outside.join("pwned").exists(),
            "a package wrote through a symlink and escaped the install root"
        );
        // Non-vacuity: the archive really does encode the attack, so a pass
        // above means containment rather than an unreadable archive that
        // never extracted anything. Either the extraction refused outright,
        // or it ran and confined the write inside the root.
        assert!(
            result.is_err() || root.join("escape").symlink_metadata().is_ok(),
            "neither refused nor extracted: the test archive is not exercising the attack"
        );
    }

    /// The same attack aimed at an absolute path rather than a relative one.
    #[test]
    fn an_absolute_symlink_cannot_be_used_to_write_outside_the_root() {
        let dir = scratch("symlink-absolute");
        let zrp = dir.join("evil.zrp");
        let root = dir.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let outside = dir.join("abs-outside");
        std::fs::create_dir_all(&outside).unwrap();

        archive_with_symlink(
            &zrp,
            "escape",
            outside.to_str().unwrap(),
            ("escape/pwned", b"written outside the install root"),
        );
        let result = unpack_recording(&zrp, &root);

        assert!(
            !outside.join("pwned").exists(),
            "a package wrote through an absolute symlink and escaped the install root"
        );
        assert!(
            result.is_err() || root.join("escape").symlink_metadata().is_ok(),
            "neither refused nor extracted: the test archive is not exercising the attack"
        );
    }

    /// The trust anchor is the whole basis for installing anything, so its
    /// absence has to be an error rather than a warning.
    #[test]
    fn a_missing_trust_anchor_is_an_error() {
        let _guard = ZRPKG_PUBKEY_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        unsafe { std::env::set_var("ZRPKG_PUBKEY", "   ") };
        let error = trusted_public_key().unwrap_err().to_string();
        unsafe { std::env::remove_var("ZRPKG_PUBKEY") };

        assert!(
            error.contains("No trusted signing key") || error.contains("is empty"),
            "got: {}",
            error
        );
    }

    #[test]
    fn an_explicit_key_in_the_environment_is_used() {
        let _guard = ZRPKG_PUBKEY_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        unsafe { std::env::set_var("ZRPKG_PUBKEY", " deadbeef ") };
        let key = trusted_public_key().unwrap();
        unsafe { std::env::remove_var("ZRPKG_PUBKEY") };
        assert_eq!(key, "deadbeef");
    }
}
