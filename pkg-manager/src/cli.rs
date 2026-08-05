use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(author, version, about = "The Vakt OS package manager.", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Fetch, verify, and install packages and everything they depend on.
    Install {
        #[arg(required = true)]
        packages: Vec<String>,
    },
    /// Delete an installed package's files, using the manifest install wrote.
    Remove {
        package: String,
        /// Remove it even though other installed packages still need it.
        #[arg(long)]
        force: bool,
    },
    /// List what the repository offers.
    Update,
    /// Download a package and check its signature without installing it.
    Verify { package: String },
    /// Build and sign a .zrp archive from a directory.
    Pack {
        source_dir: String,
        private_key_hex: String,
        /// Directory to write <name>.zrp and <name>.json into.
        #[arg(long, short = 'o')]
        out_dir: Option<String>,
        #[arg(long, default_value = "0.1.0")]
        version: String,
        #[arg(long, default_value = "A Vakt OS package.")]
        description: String,
        /// Packages this one needs, as a comma-separated list.
        #[arg(long = "depends", value_delimiter = ',')]
        dependencies: Vec<String>,
    },
}
