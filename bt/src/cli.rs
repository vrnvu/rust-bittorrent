use clap::{Parser, Subcommand};

/// rust-bittorrent cli
#[derive(Parser, Debug)]
#[clap(
    version = "0.1.0",
    author = "Arnau Diaz <arnaudiaz@duck.com>",
    about = "A simple BitTorrent client written in Rust."
)]
pub struct Cli {
    /// Sets logging to "debug" level, defaults to "info"
    #[clap(short, long, global = true)]
    pub verbose: bool,

    #[clap(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Download a file using a .torrent file
    Download {
        /// sets the path to the .torrent file to download
        #[clap(short, long)]
        torrent_file: String,

        /// Sets the output for downloaded files
        #[clap(short, long)]
        output_path: String,
    },
    /// Upload a file directly to a peer, *custom implementation*
    Upload {
        /// Sets the path to file to upload
        #[clap(short, long)]
        file: String,

        /// Sets peer port
        #[clap(short, long)]
        port: String,
    },
    /// Interactive mode
    Interactive, // TODO: we need a download interactive session and upload mode
}
