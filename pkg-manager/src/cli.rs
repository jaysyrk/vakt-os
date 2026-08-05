use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    Install { package: String },
    Remove { package: String },
    Update,
    Verify { package: String },
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
    },
}
