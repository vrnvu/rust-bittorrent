use clap::Parser;

/// rust-bittorrent cli
#[derive(Parser, Debug)]
#[clap(
    version = "0.1.0",
    author = "Arnau Diaz <arnaudiaz@duck.com>",
    about = "A simple BitTorrent client written in Rust."
)]
pub struct Cli {
    /// Sets the path to the .torrent file to download
    #[clap(short, long)]
    pub file: String,

    /// Sets logging to "debug" level, defaults to "info"
    #[clap(short, long)]
    pub verbose: bool,

    /// Sets the output for downloaded files
    #[clap(short, long)]
    pub output_path: String,
}
