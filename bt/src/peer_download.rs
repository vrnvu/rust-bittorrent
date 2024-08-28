use std::net::SocketAddr;

use anyhow::{bail, Context};
use log::{debug, error, info};
use tokio::net::TcpStream;

use crate::torrent::{self, PeerId, TorrentFile};

pub struct PeerDownload {
    pub peer_id: PeerId,
    pub file: String,
    pub output_path: String,
    torrent: TorrentFile,
}

impl PeerDownload {
    pub fn new(file: &str, output_path: &str, torrent: TorrentFile) -> Self {
        PeerDownload {
            peer_id: PeerId::new(),
            file: file.to_string(),
            output_path: output_path.to_string(),
            torrent,
        }
    }

    pub async fn download(&self, peers: &[SocketAddr]) -> anyhow::Result<()> {
        let peer = peers
            .first()
            .with_context(|| "expected one peer at least")?;

        let mut stream = TcpStream::connect(peer).await?;
        let peer_id = torrent::HandshakeMessage::new(self.torrent.info_hash_bytes)
            .initiate(&mut stream)
            .await
            .with_context(|| {
                format!(
                    "error handshake initiate for info_hash: {}",
                    self.torrent.info_hash
                )
            })?;
        info!("handshake success with peer {}", peer_id);

        match torrent::PeerMessage::receive(&mut stream).await? {
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
            .send(&mut stream)
            .await
            .with_context(|| "failed to send Interested")?;
        info!("interested send to peer");

        match torrent::PeerMessage::receive(&mut stream)
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

        self.torrent
            .download(&mut stream, &self.output_path)
            .await
            .with_context(|| "failed to download torrent")?;

        info!("success");

        Ok(())
    }
}
