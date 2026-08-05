mod cli;
mod db;
mod fetch;
mod install;
mod manifest;
mod pack;
mod remove;
mod repo;
mod resolve;

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
        Commands::Install { packages } => {
            let root = install_root();
            println!("Install root: {}", root.display());
            install::Installer::new(&root).install(packages).await?;
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
        Commands::Verify { package } => {
            install::Installer::new(&install_root())
                .verify(package)
                .await?;
        }
        Commands::Pack {
            source_dir,
            private_key_hex,
            out_dir,
            version,
            description,
            dependencies,
        } => {
            pack::pack_directory(
                source_dir,
                private_key_hex,
                out_dir.as_deref(),
                version,
                description,
                dependencies,
            )?;
        }
    }

    Ok(())
}
