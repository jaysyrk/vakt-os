mod cli;
mod config;
mod db;
mod fetch;
mod install;
mod manifest;
mod pack;
mod remove;
mod repo;
mod resolve;

use anyhow::Context;
use clap::Parser;
use cli::{Cli, Commands};
use std::path::PathBuf;

/// Where packages get unpacked. vakt-init points this at the persistent disk
/// when one is mounted, so installs survive a reboot; otherwise it is RAM only.
fn install_root() -> PathBuf {
    PathBuf::from(std::env::var("ZRPKG_ROOT").unwrap_or_else(|_| "/opt/vakt".to_string()))
}

/// Points this system at a different repository.
///
/// The setting goes on the persistent disk, not into the image: the root
/// filesystem is read-only by the time anything can run this, and a repository
/// URL is deployment configuration rather than part of the build.
fn set_repository(url: &str) -> anyhow::Result<()> {
    let url = config::normalise(url)?;
    let path = std::path::Path::new(config::PERSISTENT_CONF);

    let Some(parent) = path.parent() else {
        anyhow::bail!("{} has no parent directory", path.display());
    };
    std::fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create {}", parent.display()))?;
    std::fs::write(path, config::render(&url))
        .with_context(|| format!("Failed to write {}", path.display()))?;

    println!("Repository set to {}", url);
    println!("Saved to {}", path.display());
    if url.starts_with("http://") {
        println!(
            "\x1b[1;33mNote: this is plain HTTP. Signatures still protect what \
             you install, but anyone on the path can see what you install.\x1b[0m"
        );
    }
    println!("\nRun 'zrpkg update' to list what it offers.");
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Install { packages } => {
            let root = install_root();
            let installer = install::Installer::new(&root);
            println!("Repository:   {}", installer.repository());
            println!("Install root: {}", root.display());
            installer.install(packages).await?;
        }
        Commands::Remove { package, force } => {
            let root = install_root();
            let removal = remove::remove_package(package, &root, *force)?;

            println!(
                "Removed {}: {} file(s), {} director(ies).",
                package, removal.files, removal.directories
            );
            if removal.missing > 0 {
                println!("{} recorded path(s) were already gone.", removal.missing);
            }
            for refused in &removal.refused {
                println!("\x1b[1;33mLeft in place - {}\x1b[0m", refused);
            }
        }
        Commands::Update => {
            repo::sync_repos().await?;
        }
        Commands::Repo { url } => match url {
            Some(url) => set_repository(url)?,
            None => {
                let repository = config::load();
                println!("Repository: {}", repository.repo_url);
                println!("Set in:     {}", repository.source);
            }
        },
        Commands::Verify { package } => {
            install::Installer::new(&install_root())
                .verify(package)
                .await?;
        }
        Commands::Pack {
            source_dir,
            private_key_hex,
            key_file,
            out_dir,
            version,
            description,
            dependencies,
        } => {
            let key = match (key_file, private_key_hex) {
                (Some(path), None) => std::fs::read_to_string(path)
                    .with_context(|| format!("Failed to read the signing key from {}", path))?
                    .trim()
                    .to_string(),
                (None, Some(hex)) => hex.clone(),
                (Some(_), Some(_)) => {
                    anyhow::bail!("Pass either --key-file or a key on the command line, not both")
                }
                (None, None) => anyhow::bail!(
                    "No signing key. Use --key-file <path> (the command line is \
                     world-readable via /proc)"
                ),
            };
            pack::pack_directory(
                source_dir,
                &key,
                out_dir.as_deref(),
                version,
                description,
                dependencies,
            )?;
        }
    }

    Ok(())
}
