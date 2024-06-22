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
        .await?;
    dbg!(&peer_stream);

    if let torrent::PeerMessage::Bitfield(payload) =
        torrent::PeerMessage::receive(&mut peer_stream.stream).await?
    {
        dbg!(&payload);
    } else {
        bail!("expected bitfield as first receive message from peer")
    }

    Ok(())
}
