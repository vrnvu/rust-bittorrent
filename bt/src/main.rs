use std::time::Duration;
use std::{env, sync::Arc};

use anyhow::{bail, Context};
use clap::Parser;
use cli::Commands;
use dialoguer::theme::ColorfulTheme;
use dialoguer::{Confirm, FuzzySelect, Input};
use http::AnnounceRequest;
use log::{debug, error, info, warn};
use tokio::io::{self};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use torrent::PeerMessage;

mod cli;
mod http;
mod peer_download;
mod torrent;
mod udp;

async fn download(file: &str, output_path: &str) -> anyhow::Result<()> {
    let torrent: torrent::TorrentFile = torrent::TorrentFile::from_path(file)
        .with_context(|| format!("failed to read {} file", file))?;

    let announce_response = match torrent.tracker_protocol {
        torrent::TrackerProtocol::Udp => udp::try_announce(AnnounceRequest::from(&torrent)).await,
        torrent::TrackerProtocol::Tcp => http::try_announce(AnnounceRequest::from(&torrent)).await,
    }
    .with_context(|| format!("failed to get announce information for torrent: {}", file))?;

    let peer_download = peer_download::PeerDownload::new(file, output_path, torrent);

    let peer = announce_response
        .peers
        .first()
        .with_context(|| format!("expected one peer at least for torrent: {}", file))?;

    // TODO: download from multiple peers
    peer_download.download(&[*peer]).await?;

    Ok(())
}

async fn upload(file: &str, port: &str) -> anyhow::Result<()> {
    info!("starting uploader for file: {}", file);

    let torrent = torrent::TorrentFile::from_path(file)?;
    http::try_register(&torrent).await?;

    let listener = TcpListener::bind(format!("127.0.0.1:{}", port)).await?;
    info!("Uploader listening on {}", listener.local_addr()?);
    let torrent = Arc::new(torrent);

    while let Ok((stream, _)) = listener.accept().await {
        let torrent_ref = Arc::clone(&torrent);
        tokio::spawn(async move {
            if let Err(e) = handle_peer(stream, &torrent_ref).await {
                error!("Error handling peer: {:?}", e);
            }
        });
    }

    Ok(())
}

const PEER_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_CONSECUTIVE_TIMEOUTS: usize = 3;

async fn handle_peer(mut stream: TcpStream, torrent: &torrent::TorrentFile) -> anyhow::Result<()> {
    let peer_addr = stream.peer_addr()?;
    info!("New peer connection established from {}", peer_addr);

    // Perform handshake
    timeout(PEER_TIMEOUT, perform_handshake(&mut stream, torrent))
        .await
        .map_err(|_| anyhow::anyhow!("Handshake timed out"))??;

    info!("Handshake completed with peer {}", peer_addr);

    // TODO: send bitfield to peer
    PeerMessage::Bitfield(vec![0]).send(&mut stream).await?;
    info!("bitfield send to peer");

    match PeerMessage::receive(&mut stream).await? {
        PeerMessage::Interested => {
            info!("peer is interested");
        }
        other => {
            error!("expected: Interested, got:{:?}", other);
            bail!("expected: Interested, got:{:?}", other);
        }
    }

    PeerMessage::Unchoke.send(&mut stream).await?;
    info!("unchoke send to peer");

    let mut consecutive_timeouts = 0;
    loop {
        match timeout(PEER_TIMEOUT, torrent::PeerMessage::receive(&mut stream)).await {
            Ok(Ok(message)) => {
                consecutive_timeouts = 0;
                debug!("Received message from {}: {:?}", peer_addr, message);

                if let Err(e) = handle_message(message, &mut stream, torrent).await {
                    warn!("Error handling message from {}: {}", peer_addr, e);
                    break;
                }
            }
            Ok(Err(e)) => {
                if let Some(io_err) = e.downcast_ref::<io::Error>() {
                    if io_err.kind() == io::ErrorKind::UnexpectedEof {
                        info!("Peer {} disconnected", peer_addr);
                        break;
                    }
                }
                error!("Error receiving message from {}: {}", peer_addr, e);
                break;
            }
            Err(_) => {
                consecutive_timeouts += 1;
                warn!("Timeout while waiting for message from {}", peer_addr);
                if consecutive_timeouts >= MAX_CONSECUTIVE_TIMEOUTS {
                    warn!(
                        "Max consecutive timeouts reached for {}, closing connection",
                        peer_addr
                    );
                    break;
                }
            }
        }
    }

    info!("Peer handling completed for {}", peer_addr);
    Ok(())
}

async fn handle_message(
    message: torrent::PeerMessage,
    stream: &mut TcpStream,
    torrent: &torrent::TorrentFile,
) -> anyhow::Result<()> {
    match message {
        torrent::PeerMessage::Request {
            index,
            begin,
            length,
        } => {
            let piece = torrent.read_piece(index, begin, length).await?;
            torrent::PeerMessage::Piece {
                index,
                begin,
                block: piece,
            }
            .send(stream)
            .await?;
        }
        _ => {
            debug!("Unhandled message type: {:?}", message);
        }
    }
    Ok(())
}

async fn perform_handshake(
    stream: &mut TcpStream,
    torrent: &torrent::TorrentFile,
) -> anyhow::Result<String> {
    torrent::HandshakeMessage::new(torrent.info_hash_bytes)
        .initiate(stream)
        .await
        .with_context(|| {
            format!(
                "error handshake initiate for info_hash: {}",
                torrent.info_hash
            )
        })
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
        Commands::Download { file, output_path } => download(file, output_path).await,
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
