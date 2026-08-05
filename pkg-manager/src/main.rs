mod cli;
mod fetch;
mod install;
mod manifest;
mod pack;
mod repo;

use clap::Parser;
use cli::{Cli, Commands};
use std::path::PathBuf;

/// Where packages get unpacked. vakt-init points this at the persistent disk
/// when one is mounted, so installs survive a reboot; otherwise it is RAM only.
fn install_root() -> PathBuf {
    PathBuf::from(std::env::var("ZRPKG_ROOT").unwrap_or_else(|_| "/opt/vakt".to_string()))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Install { package } => {
            let target_dir = install_root();
            println!("Installing package: {} -> {}", package, target_dir.display());
            std::fs::create_dir_all(&target_dir)?;
            install::install_package(package, &target_dir).await?;
        }
        Commands::Remove { package } => {
            println!("Removing package: {}", package);
        }
        Commands::Update => {
            repo::sync_repos().await?;
        }
        Commands::Verify { package } => {
            println!("Verifying signature for: {}", package);
        }
        Commands::Pack {
            source_dir,
            private_key_hex,
            out_dir,
            version,
            description,
        } => {
            pack::pack_directory(
                source_dir,
                private_key_hex,
                out_dir.as_deref(),
                version,
                description,
            )?;
        }
    }

    Ok(())
}
