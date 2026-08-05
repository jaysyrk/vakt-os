use crate::manifest::PackageManifest;
use anyhow::{Context, Result};
use ed25519_dalek::{Signer, SigningKey};
use flate2::write::GzEncoder;
use flate2::Compression;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::path::{Path, PathBuf};

/// Packs a directory into a signed `.zrp` archive plus a `.json` manifest.
///
/// The archive name is authoritative: `install::install_package` fetches
/// `<name>.zrp` from the repository, so both sides must agree on it.
pub fn pack_directory(
    source_dir: &str,
    private_key_hex: &str,
    out_dir: Option<&str>,
    version: &str,
    description: &str,
) -> Result<()> {
    let source_path = Path::new(source_dir);
    if !source_path.is_dir() {
        anyhow::bail!("Source directory does not exist: {}", source_dir);
    }

    let pkg_name = source_path
        .file_name()
        .context("Source directory has no name")?
        .to_string_lossy()
        .into_owned();

    let out_dir = PathBuf::from(out_dir.unwrap_or("."));
    std::fs::create_dir_all(&out_dir).context("Failed to create output directory")?;

    let archive_path = out_dir.join(format!("{}.zrp", pkg_name));
    let manifest_path = out_dir.join(format!("{}.json", pkg_name));

    println!("Packing {} into {}...", source_dir, archive_path.display());

    let tar_gz = File::create(&archive_path).context("Failed to create output file")?;
    let enc = GzEncoder::new(tar_gz, Compression::default());
    let mut tar = tar::Builder::new(enc);

    tar.append_dir_all(".", source_path)
        .context("Failed to append directory to tarball")?;
    // into_inner() writes the tar trailer and hands back the encoder; the gzip
    // stream must then be finished explicitly or the archive is truncated.
    tar.into_inner()
        .context("Failed to finish tarball")?
        .finish()
        .context("Failed to flush gzip stream")?;

    println!("Signing package...");
    let key_bytes = hex::decode(private_key_hex).context("Invalid private key hex")?;
    let key_array: &[u8; 32] = key_bytes
        .as_slice()
        .try_into()
        .context("Invalid key length (expected 32 bytes / 64 hex characters)")?;
    let signing_key = SigningKey::from_bytes(key_array);

    let archive_data = std::fs::read(&archive_path)?;
    let mut hasher = Sha256::new();
    hasher.update(&archive_data);
    let hash = hasher.finalize();

    let signature = signing_key.sign(&hash);
    let sig_hex = hex::encode(signature.to_bytes());

    let manifest = PackageManifest {
        name: pkg_name,
        version: version.to_string(),
        description: description.to_string(),
        dependencies: Vec::new(),
        signature: sig_hex.clone(),
    };
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)
        .context("Failed to write manifest")?;

    let public_key_hex = hex::encode(signing_key.verifying_key().to_bytes());

    println!("Signature:  {}", sig_hex);
    println!("Public key: {}", public_key_hex);
    println!("Manifest:   {}", manifest_path.display());
    println!("Done! Package is ready for distribution.");

    Ok(())
}
