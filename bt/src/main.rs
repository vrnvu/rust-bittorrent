use std::env;

use anyhow::{bail, Context};
use clap::Parser;
use cli::Commands;
use http::AnnounceRequest;
use log::{debug, error, info};

mod cli;
mod http;
mod torrent;
mod udp;

async fn download(file: &str, output_path: &str, verbose: bool) -> anyhow::Result<()> {
    if verbose {
        env::set_var("RUST_LOG", "debug")
    } else {
        env::set_var("RUST_LOG", "info")
    }
    env_logger::init();

    let torrent: torrent::Torrent = torrent::Torrent::from_path(&file)
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
        .download(&mut peer_stream.stream, &output_path)
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

    info!("upload file: {}", file);

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
    }
}
