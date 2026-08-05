use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Repository {
    pub name: String,
    pub url: String,
    pub public_key: String,
}

/// A single entry in the repository's `index.json`, written by mkrepo.sh.
#[derive(Debug, Serialize, Deserialize)]
pub struct IndexEntry {
    pub name: String,
    pub version: String,
    pub description: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RepoIndex {
    pub packages: Vec<IndexEntry>,
}

pub fn add_repo(name: &str, url: &str, _public_key: &str) -> Result<()> {
    println!("Adding repository '{}': {}", name, url);
    // Write to /etc/vakt/repos.d/
    Ok(())
}

/// Fetches the repository index and lists what is available to install.
pub async fn sync_repos() -> Result<()> {
    let repo_url =
        std::env::var("ZRPKG_REPO_URL").unwrap_or_else(|_| "http://10.0.2.2:8080".to_string());
    println!("Syncing package database from {}...", repo_url);

    let body = reqwest::Client::new()
        .get(format!("{}/index.json", repo_url))
        .send()
        .await
        .context("Failed to reach repository (is zrpkg-server running on the host?)")?
        .error_for_status()
        .context("Repository has no index.json")?
        .text()
        .await?;

    let index: RepoIndex = serde_json::from_str(&body).context("Malformed repository index")?;

    if index.packages.is_empty() {
        println!("Repository is empty.");
        return Ok(());
    }

    println!("\n{} package(s) available:\n", index.packages.len());
    for pkg in &index.packages {
        println!("  {:<18} {:<8} {}", pkg.name, pkg.version, pkg.description);
    }
    println!("\nInstall with: zrpkg install <name>");

    Ok(())
}
