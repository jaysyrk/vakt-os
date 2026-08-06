use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// A single entry in the repository's `index.json`, written by mkrepo.sh.
#[derive(Debug, Serialize, Deserialize)]
pub struct IndexEntry {
    pub name: String,
    pub version: String,
    pub description: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RepoIndex {
    pub packages: Vec<IndexEntry>,
}

/// Fetches the repository index and lists what is available to install.
pub async fn sync_repos() -> Result<()> {
    let repository = crate::config::load();
    println!(
        "Syncing package database from {} (set in {})...",
        repository.repo_url, repository.source
    );

    let body = reqwest::Client::new()
        .get(format!("{}/index.json", repository.repo_url))
        .send()
        .await
        .with_context(|| {
            format!(
                "Failed to reach {}. Check the server is running and reachable; \
                 change it with 'zrpkg repo <url>' or the panel's Packages page.",
                repository.repo_url
            )
        })?
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
        if !pkg.dependencies.is_empty() {
            println!(
                "  {:<18} {:<8} requires {}",
                "",
                "",
                pkg.dependencies.join(", ")
            );
        }
    }
    println!("\nInstall with: zrpkg install <name>");

    Ok(())
}
