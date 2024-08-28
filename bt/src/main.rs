use std::env;

use anyhow::Context;
use clap::Parser;
use cli::Commands;
use dialoguer::theme::ColorfulTheme;
use dialoguer::{Confirm, FuzzySelect, Input};
use http::AnnounceRequest;
use log::info;
use peer_download::TorrentFileMetadata;

mod cli;
mod http;
mod peer_download;
mod peer_upload;
mod torrent;
mod udp;

async fn download(output_path: &str, torrent_file: &str) -> anyhow::Result<()> {
    let torrent: torrent::TorrentFile = torrent::TorrentFile::from_path(torrent_file)
        .with_context(|| format!("failed to read {} file", torrent_file))?;

    let tracker_protocol = torrent.tracker_protocol.clone();
    let torrent_metadata = TorrentFileMetadata::new(
        torrent.length(),
        torrent.pieces(),
        torrent.info_hash.clone(),
        torrent.info_hash_bytes,
        torrent.piece_length(),
    );
    let peer_download = peer_download::PeerDownload::new(output_path, torrent_metadata);

    let announce_response = match tracker_protocol {
        torrent::TrackerProtocol::Udp => {
            udp::try_announce(AnnounceRequest::from(&torrent), &peer_download.peer_id).await
        }
        torrent::TrackerProtocol::Tcp => {
            http::try_announce(AnnounceRequest::from(&torrent), &peer_download.peer_id).await
        }
    }
    .with_context(|| {
        format!(
            "failed to get announce information for torrent file: {}",
            torrent_file
        )
    })?;

    let peer = announce_response
        .peers
        .first()
        .with_context(|| format!("expected one peer at least for torrent: {}", torrent_file))?;

    // TODO: download from multiple peers
    peer_download.download(&[*peer]).await?;

    Ok(())
}

async fn upload(file: &str, port: &str) -> anyhow::Result<()> {
    let peer_upload = peer_upload::PeerUpload::new();
    info!(
        "starting uploader with peer_id: {} for file: {}",
        peer_upload.peer_id, file
    );
    let torrent = torrent::TorrentFile::from_path(file)?;
    http::try_register(&peer_upload.peer_id, &torrent).await?;
    peer_upload.upload(torrent, port).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = cli::Cli::parse();
    if args.verbose {
        env::set_var("RUST_LOG", "debug")
    } else {
        env::set_var("RUST_LOG", "info")
    }
    env_logger::init();

    match &args.command {
        Commands::Download {
            torrent_file,
            output_path,
        } => download(output_path, torrent_file).await,
        Commands::Upload { file, port } => upload(file, port).await,
        Commands::Interactive => {
            let announce_url = "http://localhost:9999/announce";
            let files = http::try_list_files(announce_url).await?;
            if files.is_empty() {
                info!("no files available to download");
                return Ok(());
            }

            let selected_file = FuzzySelect::with_theme(&ColorfulTheme::default())
                .with_prompt("Select a file to download from available peers")
                .default(0)
                .items(&files[..])
                .interact()
                .unwrap();

            let default_output_path: String = match files.get(selected_file) {
                Some(file) => file.to_string(),
                None => {
                    return Err(anyhow::anyhow!("selected file invalid"));
                }
            };

            let output_path: String = Input::with_theme(&ColorfulTheme::default())
                .with_prompt("The file will be downloaded in your current directory as")
                .default(default_output_path)
                .interact_text()?;

            match Confirm::with_theme(&ColorfulTheme::default())
                .with_prompt(format!(
                    "Do you want to download file: {} and save it as {}?",
                    files[selected_file], output_path
                ))
                .interact()?
            {
                true => todo!(),
                false => return Ok(()),
            }
        }
    }
}
