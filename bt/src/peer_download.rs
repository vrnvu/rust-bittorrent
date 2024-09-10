use std::{
    fs::{self, File},
    io::Write,
    net::SocketAddr,
    path::Path,
    time::Duration,
};

use anyhow::{bail, Context};
use log::{debug, error, info};
use sha1::{Digest, Sha1};
use tokio::{net::TcpStream, time::timeout};

use crate::torrent::{self, PeerId, PeerMessage};

const BLOCK_MAX_SIZE: u32 = 1 << 14;
const PEER_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug)]
pub struct TorrentFileMetadata {
    length: i64,
    pieces: Vec<Vec<u8>>,
    info_hash: String,
    info_hash_bytes: [u8; 20],
    piece_length: i64,
}

impl TorrentFileMetadata {
    pub fn new(
        length: i64,
        pieces: Vec<Vec<u8>>,
        info_hash: String,
        info_hash_bytes: [u8; 20],
        piece_length: i64,
    ) -> Self {
        TorrentFileMetadata {
            length,
            pieces,
            info_hash,
            info_hash_bytes,
            piece_length,
        }
    }
}

pub struct PeerDownload {
    pub peer_id: PeerId,
    pub output_path: String,
    torrent_metadata: TorrentFileMetadata,
}

impl PeerDownload {
    pub fn new(output_path: &str, torrent_metadata: TorrentFileMetadata) -> Self {
        PeerDownload {
            peer_id: PeerId::new(),
            output_path: output_path.to_string(),
            torrent_metadata,
        }
    }

    pub async fn init_peer(&self, peer: &SocketAddr) -> anyhow::Result<TcpStream> {
        let mut stream = TcpStream::connect(peer).await?;
        let peer_id = timeout(PEER_TIMEOUT, async {
            torrent::HandshakeMessage::new(self.peer_id, self.torrent_metadata.info_hash_bytes)
                .initiate(&mut stream)
                .await
                .with_context(|| {
                    format!(
                        "error handshake initiate for info_hash: {}",
                        self.torrent_metadata.info_hash
                    )
                })
        })
        .await
        .map_err(|_| anyhow::anyhow!("Handshake timed out"))??;
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
        Ok(stream)
    }

    pub async fn download(&self, mut streams: Vec<TcpStream>) -> anyhow::Result<()> {
        let mut downloaded_torrent = Vec::new();
        let streams_len = streams.len();
        for piece_index in 0..self.torrent_metadata.pieces.len() {
            // TODO: round robin strategy for downloading pieces from peers
            let stream = streams
                .get_mut(piece_index % streams_len)
                .context("no stream available")?;
            let piece_index = piece_index as i64;
            let piece = Self::download_piece(stream, piece_index, &self.torrent_metadata).await?;
            downloaded_torrent.push(piece);
            debug!(
                "piece_index: {} downloaded successfully for info_hash: {}",
                piece_index, self.torrent_metadata.info_hash
            );
        }

        let bytes = downloaded_torrent.concat();
        if let Some(parent) = Path::new(&self.output_path).parent() {
            fs::create_dir_all(parent).context("cannot create output directory")?;
        }

        let mut f = File::create(&self.output_path).context("cannot create output file")?;
        f.write_all(&bytes).with_context(|| {
            format!(
                "failed to write downloaded data to file {}",
                &self.output_path
            )
        })?;
        info!(
            "torrent info_hash: {} downloaded successfully to {}",
            self.torrent_metadata.info_hash, &self.output_path
        );

        info!("success");
        Ok(())
    }

    pub async fn download_piece(
        stream: &mut TcpStream,
        piece_index: i64,
        torrent_metadata: &TorrentFileMetadata,
    ) -> anyhow::Result<Vec<u8>> {
        let piece_length = torrent_metadata
            .piece_length
            .min(torrent_metadata.length - (torrent_metadata.piece_length * piece_index));

        let mut piece: Vec<u8> = Vec::with_capacity(piece_length as usize);
        let mut begin_offset: u32 = 0;
        let mut remain: u32 = piece_length as u32;
        while remain != 0 {
            let block_size = BLOCK_MAX_SIZE.min(remain);
            PeerMessage::Request {
                index: piece_index as u32,
                begin: begin_offset,
                length: block_size,
            }
            .send(stream)
            .await
            .context("failed to send PeerMessage::Request")?;

            if let PeerMessage::Piece {
                index,
                begin,
                block,
            } = PeerMessage::receive(stream)
                .await
                .context("failed to receive PeerMessage::Piece")?
            {
                assert_eq!(piece_index as u32, index);
                assert_eq!(begin_offset, begin);
                piece.splice(begin as usize..begin as usize, block.into_iter());
            } else {
                error!("expected piece message from peer");
                bail!("expected piece message from peer");
            }
            begin_offset += block_size;
            remain -= block_size;
        }

        let piece_hash = torrent_metadata
            .pieces
            .get(piece_index as usize)
            .context("invalid index for piece hash")?;

        let current_hash: [u8; 20] = {
            let mut hasher = Sha1::new();
            hasher.update(&piece);
            hasher.finalize().into()
        };

        if *piece_hash != current_hash {
            bail!(
                "hash mismatch for index {}: expected {}, got {}",
                piece_index,
                hex::encode(piece_hash),
                hex::encode(current_hash)
            )
        }

        Ok(piece)
    }
}
