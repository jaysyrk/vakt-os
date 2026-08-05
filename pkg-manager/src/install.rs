use crate::fetch::{download_package, verify_signature};
use crate::manifest::PackageManifest;
use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use reqwest::Client;
use std::fs::File;
use std::path::Path;
use tar::Archive;
use tokio::fs as async_fs;

/// Path to the hex-encoded ed25519 public key this system trusts for packages.
const TRUSTED_KEY_PATH: &str = "/etc/vakt/trusted.key";

fn trusted_public_key() -> Option<String> {
    if let Ok(key) = std::env::var("ZRPKG_PUBKEY") {
        return Some(key.trim().to_string());
    }
    std::fs::read_to_string(TRUSTED_KEY_PATH)
        .ok()
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
}

pub async fn install_package(package_name: &str, target_dir: &Path) -> Result<()> {
    let repo_url =
        std::env::var("ZRPKG_REPO_URL").unwrap_or_else(|_| "http://10.0.2.2:8080".to_string());
    let url = format!("{}/{}.zrp", repo_url, package_name);
    let manifest_url = format!("{}/{}.json", repo_url, package_name);

    let tmp_dir = Path::new("/tmp");
    std::fs::create_dir_all(tmp_dir).context("Failed to create /tmp")?;
    let tmp_path = tmp_dir.join(format!("{}.zrp", package_name));

    let client = Client::new();
    println!("Fetching {}...", package_name);
    download_package(&client, &url, &tmp_path).await?;

    println!("Verifying signature for {}...", package_name);
    let data = async_fs::read(&tmp_path).await?;
    match trusted_public_key() {
        Some(public_key) => {
            let manifest_json = client
                .get(&manifest_url)
                .send()
                .await?
                .error_for_status()
                .context("Failed to fetch package manifest")?
                .text()
                .await?;
            let manifest = PackageManifest::parse(&manifest_json)?;

            // A package that fails verification must never reach the disk.
            if let Err(e) = verify_signature(&data, &manifest.signature, &public_key) {
                let _ = async_fs::remove_file(&tmp_path).await;
                return Err(e).context(format!("Refusing to install {}", package_name));
            }
            println!("Signature OK ({} v{})", manifest.name, manifest.version);
        }
        None => {
            println!(
                "\x1b[1;33mWarning: no trusted key at {} (and ZRPKG_PUBKEY unset). \
                 Installing unverified.\x1b[0m",
                TRUSTED_KEY_PATH
            );
        }
    }

    println!("Unpacking {}...", package_name);
    let tar_gz = File::open(&tmp_path).context("Failed to open downloaded archive")?;
    let tar = GzDecoder::new(tar_gz);
    let mut archive = Archive::new(tar);

    archive.unpack(target_dir).context("Failed to unpack archive")?;

    async_fs::remove_file(&tmp_path).await?;
    println!("Successfully installed {}!", package_name);

    Ok(())
}
