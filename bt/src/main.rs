use std::{env, sync::Arc};

use anyhow::{bail, Context};
use clap::Parser;
use cli::Commands;
use http::AnnounceRequest;
use log::{debug, error, info};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

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

    let torrent: torrent::Torrent = torrent::Torrent::from_path(file)
        .with_context(|| format!("failed to read {} file", file))?;

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

    let torrent: torrent::Torrent = torrent::Torrent::from_path(file)
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

    let torrent = torrent::Torrent::from_path("sample-http.torrent")?;
    let listener = TcpListener::bind("127.0.0.1:6881").await?;
    info!("Uploader listening on 127.0.0.1:6881");
    let torrent = Arc::new(torrent);

    while let Ok((mut stream, _)) = listener.accept().await {
        let torrent_ref = Arc::clone(&torrent);
        tokio::spawn(async move {
            if let Err(e) = handle_peer(&mut stream, &torrent_ref).await {
                error!("Error handling peer: {:?}", e);
            }
        });
    }

    Ok(())
}

async fn handle_peer(stream: &mut TcpStream, torrent: &torrent::Torrent) -> anyhow::Result<()> {
    // Perform handshake
    let mut buffer = vec![0u8; 68];
    stream.read_exact(&mut buffer).await?;
    stream.write_all(&buffer).await?;

    // Read and respond to requests
    Ok(loop {
        match torrent::PeerMessage::receive(stream).await {
            Ok(message) => match message {
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
                    error!(
                        "unexpected message type: {:?}, not implemented yet",
                        message
                    );
                    bail!(
                        "unexpected message type: {:?}, not implemented yet",
                        message
                    );
                }
            },
            Err(e) => {
                if e.to_string().contains("UnexpectedEof") {
                    info!("Peer disconnected");
                    break;
                } else {
                    error!("Error receiving message: {:?}", e);
                    return Err(e);
                }
            }
        }
    })
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
