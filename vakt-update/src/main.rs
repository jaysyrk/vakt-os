//! vakt-update fetches, verifies, and stages an OS image update onto
//! `/persistent`, alongside the running system rather than in place of it.
//! Unvalidated on real hardware - see docs/OS_UPDATES.md. Reuses zrpkg's
//! download/verify/archive-safety code rather than a second implementation
//! of any of it; built with `zrpkg pack` (see build-system/mkupdate.sh).

mod envblock;

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use std::fs::File;
use std::path::{Path, PathBuf};
use tar::Archive;
use zrpkg::fetch::{download_package, verify_signature};
use zrpkg::manifest::PackageManifest;
use zrpkg::remove::safe_relative;

const TRUSTED_KEY_PATH: &str = "/etc/vakt/trusted.key";
const BUNDLE_NAME: &str = "vakt-update";

const SLOT_B_DIR: &str = "/persistent/vakt-update/B";
const STAGING_DIR: &str = "/persistent/vakt-update/.staging-B";
const VERSION_FILE: &str = "version";

const BOOTENV_PATH: &str = "/persistent/etc/vakt/bootenv";
const STATE_PATH: &str = "/persistent/etc/vakt-update-state.json";
/// Keep in sync with vakt-init/src/update.rs.
const MAX_BOOT_ATTEMPTS: u32 = 3;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let result = match args.get(1).map(String::as_str) {
        Some("check") => check(),
        Some("apply") => apply(args.get(2).map(String::as_str) == Some("--reboot")),
        _ => {
            eprintln!("usage: vakt-update check | vakt-update apply [--reboot]");
            std::process::exit(2);
        }
    };

    if let Err(e) = result {
        eprintln!("[!] {:#}", e);
        std::process::exit(1);
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Runtime::new().expect("failed to start async runtime")
}

fn trusted_public_key() -> Result<String> {
    let key = std::fs::read_to_string(TRUSTED_KEY_PATH)
        .with_context(|| format!("no trusted signing key at {}", TRUSTED_KEY_PATH))?
        .trim()
        .to_string();
    if key.is_empty() {
        bail!(
            "{} is empty; there is no key to verify an update against",
            TRUSTED_KEY_PATH
        );
    }
    Ok(key)
}

fn check() -> Result<()> {
    let active = envblock::read(Path::new(BOOTENV_PATH))
        .ok()
        .and_then(|entries| entries.get("vakt_active").cloned())
        .unwrap_or_else(|| "A".to_string());
    println!("Active slot: {}", active);

    let config = zrpkg::config::load();
    println!("Repository: {} (from {})", config.repo_url, config.source);

    let manifest = runtime().block_on(fetch_manifest(&config.repo_url))?;
    println!("Available:  {} {}", manifest.name, manifest.version);

    let current = std::fs::read_to_string(Path::new(SLOT_B_DIR).join(VERSION_FILE))
        .unwrap_or_else(|_| "none (running slot A)".to_string());
    println!("Staged:     {}", current.trim());
    Ok(())
}

fn apply(reboot: bool) -> Result<()> {
    let config = zrpkg::config::load();
    println!("Fetching update from {}...", config.repo_url);

    let pubkey = trusted_public_key()?;
    let manifest = runtime().block_on(fetch_manifest(&config.repo_url))?;

    let archive_url = format!("{}/{}.zrp", config.repo_url, BUNDLE_NAME);
    let tmp = std::env::temp_dir().join(format!("{}.zrp", BUNDLE_NAME));
    runtime()
        .block_on(async {
            let client = reqwest::Client::new();
            download_package(&client, &archive_url, &tmp).await
        })
        .context("downloading the update bundle")?;

    println!("Verifying signature...");
    let data = std::fs::read(&tmp).context("reading the downloaded bundle")?;
    verify_signature(&data, &manifest.signature, &pubkey)
        .context("update bundle failed signature verification")?;

    println!("Extracting...");
    let _ = std::fs::remove_dir_all(STAGING_DIR);
    std::fs::create_dir_all(STAGING_DIR)?;
    unpack_safely(&tmp, Path::new(STAGING_DIR))?;
    std::fs::remove_file(&tmp).ok();

    for required in ["vmlinuz", "initramfs.img"] {
        if !Path::new(STAGING_DIR).join(required).is_file() {
            bail!(
                "update bundle is missing {}; refusing to install a partial slot",
                required
            );
        }
    }
    std::fs::write(Path::new(STAGING_DIR).join(VERSION_FILE), &manifest.version)?;

    let _ = std::fs::remove_dir_all(SLOT_B_DIR);
    std::fs::rename(STAGING_DIR, SLOT_B_DIR).context("activating the staged slot")?;

    envblock::write(Path::new(BOOTENV_PATH), &[("vakt_active", "B")])
        .context("writing the GRUB environment block")?;

    let state = format!(r#"{{"tries_left":{}}}"#, MAX_BOOT_ATTEMPTS);
    std::fs::create_dir_all(Path::new(STATE_PATH).parent().unwrap())?;
    std::fs::write(STATE_PATH, state).context("writing the update state file")?;

    println!();
    println!("========================================");
    println!("  Update staged: {} {}", manifest.name, manifest.version);
    println!(
        "  Slot B has {} boot attempt(s) to reach readiness",
        MAX_BOOT_ATTEMPTS
    );
    println!("  before vakt-init rolls back to slot A automatically.");
    println!("========================================");

    if reboot {
        println!("Rebooting...");
        std::process::Command::new("reboot")
            .status()
            .context("running reboot")?;
    } else {
        println!("Reboot to apply: reboot");
    }
    Ok(())
}

async fn fetch_manifest(repo_url: &str) -> Result<PackageManifest> {
    let url = format!("{}/{}.json", repo_url, BUNDLE_NAME);
    let text = reqwest::get(&url)
        .await
        .and_then(|r| r.error_for_status())
        .context("fetching the update manifest")?
        .text()
        .await
        .context("reading the update manifest response")?;
    PackageManifest::parse(&text).context("parsing the update manifest")
}

/// Extracts `archive_path` into `target`, rejecting any entry `safe_relative`
/// would reject.
fn unpack_safely(archive_path: &Path, target: &Path) -> Result<()> {
    let file = File::open(archive_path).context("opening the downloaded bundle")?;
    let mut archive = Archive::new(GzDecoder::new(file));
    archive.set_overwrite(true);

    for entry in archive.entries().context("bundle is not readable")? {
        let mut entry = entry.context("bundle entry is corrupt")?;
        let path: PathBuf = entry
            .path()
            .context("bundle entry has an unreadable path")?
            .into_owned();

        safe_relative(&path)
            .with_context(|| format!("refusing bundle entry '{}'", path.display()))?;

        if !entry
            .unpack_in(target)
            .with_context(|| format!("failed to unpack '{}'", path.display()))?
        {
            bail!("bundle entry '{}' was rejected as unsafe", path.display());
        }
    }
    Ok(())
}
