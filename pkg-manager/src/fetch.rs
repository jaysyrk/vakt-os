use anyhow::{Context, Result};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use reqwest::Client;
use sha2::{Digest, Sha256};
use std::path::Path;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

/// Streams a package to `dest`.
///
/// The flush and sync at the end are not belt-and-braces. `tokio::fs::File`
/// buffers, and hands the buffer to a blocking pool that may not have written
/// it by the time the handle is dropped - so the very next read of the file can
/// see a short archive. Signature verification reads the file back immediately,
/// which turns a dropped tail into "this package is not signed by the trusted
/// key": a real failure, reported as a security error, that appears and
/// disappears between runs.
pub async fn download_package(client: &Client, url: &str, dest: &Path) -> Result<()> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    let mut file = File::create(dest).await.context("Failed to create file")?;

    while let Some(chunk) = response.chunk().await? {
        file.write_all(&chunk).await?;
    }

    file.flush().await.context("Failed to flush the download")?;
    file.sync_all()
        .await
        .context("Failed to commit the download to disk")?;
    Ok(())
}

pub fn verify_signature(data: &[u8], signature_hex: &str, public_key_hex: &str) -> Result<()> {
    let sig_bytes = hex::decode(signature_hex).context("Invalid signature hex")?;
    let pk_bytes = hex::decode(public_key_hex).context("Invalid public key hex")?;

    let signature = Signature::from_slice(&sig_bytes).context("Invalid signature length")?;
    let public_key = VerifyingKey::from_bytes(
        pk_bytes
            .as_slice()
            .try_into()
            .context("Invalid public key length")?,
    )?;

    let mut hasher = Sha256::new();
    hasher.update(data);
    let hash = hasher.finalize();

    public_key
        .verify(&hash, &signature)
        .context("Signature verification failed")?;
    Ok(())
}
