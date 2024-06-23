use anyhow::bail;

mod http;
mod torrent;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let path = "sample.torrent";
    let torrent: torrent::Torrent = torrent::Torrent::from_path(path)?;

    let announce_response = http::try_announce(&torrent).await?;
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
    dbg!(&peer_stream);

    match torrent::PeerMessage::receive(&mut peer_stream.stream).await? {
        torrent::PeerMessage::Bitfield(payload) => {
            dbg!(&payload);
        }
        other => {
            bail!("expected: Bitfield, got:{:?}", other);
        }
    }

    torrent::PeerMessage::Interested
        .send(&mut peer_stream.stream)
        .await?;
    dbg!("interested send to peer");

    match torrent::PeerMessage::receive(&mut peer_stream.stream).await? {
        torrent::PeerMessage::Unchoke => {
            dbg!("unchoke received");
        }
        other => {
            bail!("expected: Unchoke, got:{:?}", other);
        }
    }

    let piece = torrent.download_piece(&mut peer_stream.stream, 0).await?;
    dbg!("piece downloaded successfully");

    Ok(())
}
