use clap::{Parser, Subcommand};

/// rust-bittorrent cli
#[derive(Parser, Debug)]
#[clap(
    version = "0.1.0",
    author = "Arnau Diaz <arnaudiaz@duck.com>",
    about = "A simple BitTorrent client written in Rust."
)]
pub struct Cli {
    #[clap(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Download a file using a .torrent file
    Download {
        /// Sets the path to the .torrent file to download
        #[clap(short, long)]
        file: String,

        /// Sets the output for downloaded files
        #[clap(short, long)]
        output_path: String,

        /// Sets logging to "debug" level, defaults to "info"
        #[clap(short, long)]
        verbose: bool,
    },
    /// Upload a file directly to a peer, *custom implementation*
    Upload {
        /// Sets the path to file to upload
        #[clap(short, long)]
        file: String,

        /// Sets logging to "debug" level, defaults to "info"
        #[clap(short, long)]
        verbose: bool,
    },
}
