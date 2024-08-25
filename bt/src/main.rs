use std::time::Duration;
use std::{env, sync::Arc};

use anyhow::{bail, Context};
use clap::Parser;
use cli::Commands;
use http::AnnounceRequest;
use log::{debug, error, info, warn};
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

mod cli;
mod http;
mod torrent;
mod udp;

async fn download_from_peer(file: &str, output_path: &str, verbose: bool) -> anyhow::Result<()> {
    if verbose {
        env::set_var("RUST_LOG", "debug")
    } else {
        env::set_var("RUST_LOG", "info")
    }
    env_logger::init();

    let torrent: torrent::TorrentFile = torrent::TorrentFile::from_path(file)
        .with_context(|| format!("failed to read {} file", file))?;

    // TODO
    let peer_addr = "127.0.0.1:6881".parse()?;
    let mut peer_stream = torrent::HandshakeMessage::new(torrent.info_hash_bytes)
        .initiate(&peer_addr)
        .await
        .context("error handshake initiate with uploader")?;

    info!("handshake initiated with uploader");

    torrent
        .download(&mut peer_stream.stream, output_path)
        .await
        .context("failed to download torrent")?;

    info!("download completed successfully");

    Ok(())
}

async fn download(file: &str, output_path: &str, verbose: bool) -> anyhow::Result<()> {
    if verbose {
        env::set_var("RUST_LOG", "debug")
    } else {
        env::set_var("RUST_LOG", "info")
    }
    env_logger::init();

    let torrent: torrent::TorrentFile = torrent::TorrentFile::from_path(file)
        .with_context(|| format!("failed to read {} file", file))?;

    let announce_response = match torrent.tracker_protocol {
        torrent::TrackerProtocol::Udp => udp::try_announce(AnnounceRequest::from(&torrent)).await,
        torrent::TrackerProtocol::Tcp => http::try_announce(AnnounceRequest::from(&torrent)).await,
    }
    .with_context(|| format!("failed to get announce information for torrent: {}", file))?;

    let peer = announce_response
        .peers
        .first()
        .with_context(|| format!("expected one peer at least for torrent: {}", file))?;

    let mut peer_stream = torrent::HandshakeMessage::new(torrent.info_hash_bytes)
        .initiate(peer)
        .await
        .with_context(|| {
            format!(
                "error handshake initiate for info_hash: {} with peer: {}",
                torrent.info_hash, peer
            )
        })?;
    debug!("peer stream: {:?}", &peer_stream);
    info!("handshake initiate with peer {}", peer_stream.peer_id);

    match torrent::PeerMessage::receive(&mut peer_stream.stream).await? {
        torrent::PeerMessage::Bitfield(payload) => {
            debug!("bitfield payload: {:?}", &payload);
        }
        other => {
            error!("expected: Bitfield, got:{:?}", other);
            bail!("expected: Bitfield, got:{:?}", other);
        }
    }
    info!("bitfield received");

    torrent::PeerMessage::Interested
        .send(&mut peer_stream.stream)
        .await
        .with_context(|| "failed to send Interested")?;
    info!("interested send to peer");

    match torrent::PeerMessage::receive(&mut peer_stream.stream)
        .await
        .with_context(|| "failed to receive PeerMessage")?
    {
        torrent::PeerMessage::Unchoke => {
            debug!("unchoke received");
        }
        other => {
            error!("expected: Unchoke, got:{:?}", other);
            bail!("expected: Unchoke, got:{:?}", other);
        }
    }

    torrent
        .download(&mut peer_stream.stream, output_path)
        .await
        .with_context(|| "failed to download torrent")?;

    info!("success");
    Ok(())
}

async fn upload(file: &str, verbose: bool) -> anyhow::Result<()> {
    if verbose {
        env::set_var("RUST_LOG", "debug")
    } else {
        env::set_var("RUST_LOG", "info")
    }
    env_logger::init();

    info!("starting uploader for file: {}", file);

    // TODO
    let torrent = torrent::TorrentFile::from_path("sample-http.torrent")?;
    let listener = TcpListener::bind("127.0.0.1:6881").await?;
    info!("Uploader listening on 127.0.0.1:6881");
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
    timeout(PEER_TIMEOUT, perform_handshake(&mut stream))
        .await
        .map_err(|_| anyhow::anyhow!("Handshake timed out"))??;

    info!("Handshake completed with peer {}", peer_addr);

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

// TODO
async fn perform_handshake(stream: &mut TcpStream) -> anyhow::Result<()> {
    let mut buffer = vec![0u8; 68];
    stream.read_exact(&mut buffer).await?;
    stream.write_all(&buffer).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = cli::Cli::parse();
    match &args.command {
        Commands::Download {
            file,
            output_path,
            verbose,
        } => download(file, output_path, *verbose).await,
        Commands::Upload { file, verbose } => upload(file, *verbose).await,
        Commands::DownloadPeer {
            file,
            output_path,
            verbose,
        } => download_from_peer(file, output_path, *verbose).await,
    }
}
