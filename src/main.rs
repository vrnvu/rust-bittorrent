use std::env;

use anyhow::{bail, Context};
use clap::Parser;
use http::{AnnounceRequest, AnnounceResponse};
use lava_torrent::torrent::v1::AnnounceList;
use log::{debug, error, info};

mod cli;
mod http;
mod torrent;
mod udp;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = cli::Cli::parse();
    if args.verbose {
        env::set_var("RUST_LOG", "debug")
    } else {
        env::set_var("RUST_LOG", "info")
    }
    env_logger::init();

    let path = args.file;
    let torrent: torrent::Torrent = torrent::Torrent::from_path(&path)
        .with_context(|| format!("failed to read {} file", path))?;

    let announce_response = match torrent.tracker_protocol {
        torrent::TrackerProtocol::UDP => {
            let announce_response = udp::try_announce(AnnounceRequest::from(&torrent))
                .await
                .context("failed to get announce information for torrent")?;
            assert!(!announce_response.peers.is_empty());
            announce_response
        }
        torrent::TrackerProtocol::TCP => {
            let announce_response = http::try_announce(AnnounceRequest::from(&torrent))
                .await
                .context("failed to get announce information for torrent")?;
            assert!(!announce_response.peers.is_empty());
            announce_response
        }
    };

    let peer = announce_response
        .peers
        .first()
        .context("expected one peer at least")?;

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
        .context("failed to send Interested")?;
    info!("interested send to peer");

    match torrent::PeerMessage::receive(&mut peer_stream.stream)
        .await
        .context("failed to receive PeerMessage")?
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
        .download(&mut peer_stream.stream, &args.output_path)
        .await
        .context("failed to download torrent")?;

    info!("success");
    Ok(())
}
