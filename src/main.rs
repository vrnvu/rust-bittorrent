use anyhow::{bail, Context};
use log::{debug, error, info};

mod http;
mod torrent;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    let path = "sample.torrent";
    let torrent: torrent::Torrent = torrent::Torrent::from_path(path)
        .with_context(|| format!("failed to read {} file", path))?;

    let announce_response = http::try_announce(&torrent)
        .await
        .context("failed to get announce information for torrent")?;
    assert!(!announce_response.peers.is_empty());

    let peer = announce_response
        .peers
        .get(0)
        .expect("expected one peer at least");

    let mut peer_stream = torrent::HandshakeMessage::new(torrent.info_hash_bytes)
        .send(peer)
        .await?
        .receive()
        .await?;
    debug!("peer stream: {:?}", &peer_stream);
    info!("handshake with peer {}", peer_stream.peer_id);

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
        .download(&mut peer_stream.stream)
        .await
        .context("failed to download torrent")?;

    info!("success");
    Ok(())
}
