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

/// What is installed under `root`, as the operator sees it.
///
/// Built as a string rather than printed directly so the formatting is
/// testable: this is the answer to "what is on this appliance", which is the
/// first question anyone asks after an IDS finding names a file, and it should
/// not be the one output nobody ever checks.
fn installed_report(root: &std::path::Path) -> String {
    use std::fmt::Write;

    let packages = db::Database::new(root).installed();
    if packages.is_empty() {
        return format!(
            "Nothing installed under {}.\n\nSee what is available with: zrpkg update\n",
            root.display()
        );
    }

    let widest = packages.iter().map(|p| p.name.len()).max().unwrap_or(0);
    let mut out = String::new();
    let _ = writeln!(out, "Installed under {}:\n", root.display());
    for package in &packages {
        let _ = write!(
            out,
            "  {:width$}  {}",
            package.name,
            package.version,
            width = widest
        );
        if !package.dependencies.is_empty() {
            let _ = write!(out, "   (needs {})", package.dependencies.join(", "));
        }
        let _ = writeln!(out);
    }
    let _ = writeln!(out, "\n{} package(s).", packages.len());
    out
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
        Commands::List => {
            print!("{}", installed_report(&install_root()));
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

#[cfg(test)]
mod tests {
    use super::*;
    use db::{Database, InstalledPackage};

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zrpkg-list-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn record(root: &std::path::Path, name: &str, version: &str, deps: &[&str]) {
        Database::new(root)
            .record(&InstalledPackage {
                name: name.to_string(),
                version: version.to_string(),
                dependencies: deps.iter().map(|s| s.to_string()).collect(),
                files: vec![format!("usr/bin/{}", name)],
                installed_at: db::now(),
            })
            .unwrap();
    }

    /// An appliance with nothing installed must say so plainly, and point
    /// somewhere useful - an empty listing that looks like a failure sends an
    /// operator hunting for a problem that is not there.
    #[test]
    fn an_empty_install_root_says_so_rather_than_printing_nothing() {
        let root = scratch("empty");
        let report = installed_report(&root);
        assert!(report.contains("Nothing installed"), "{}", report);
        assert!(report.contains("zrpkg update"), "{}", report);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn installed_packages_are_listed_with_versions_and_dependencies() {
        let root = scratch("listing");
        record(&root, "vakt-ids", "1.0.0", &[]);
        record(&root, "tool", "2.1.0", &["vakt-ids"]);

        let report = installed_report(&root);
        assert!(report.contains("vakt-ids"), "{}", report);
        assert!(report.contains("1.0.0"), "{}", report);
        assert!(report.contains("2.1.0"), "{}", report);
        assert!(
            report.contains("needs vakt-ids"),
            "a package's dependencies are why it cannot simply be removed: {}",
            report
        );
        assert!(report.contains("2 package(s)"), "{}", report);

        // Sorted, so the same appliance always reports the same order.
        let ids = report.find("vakt-ids").unwrap();
        let tool = report
            .find("tool  ")
            .or_else(|| report.find("tool "))
            .unwrap();
        assert!(tool < ids, "expected alphabetical order:\n{}", report);

        let _ = std::fs::remove_dir_all(&root);
    }
}
