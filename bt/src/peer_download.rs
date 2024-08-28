use std::{
    fs::{self, File},
    io::Write,
    net::SocketAddr,
    path::Path,
};

use anyhow::{bail, Context};
use log::{debug, error, info};
use sha1::{Digest, Sha1};
use tokio::net::TcpStream;

use crate::torrent::{self, PeerId, PeerMessage};

const BLOCK_MAX_SIZE: u32 = 1 << 14;

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

    pub async fn download(&self, peers: &[SocketAddr]) -> anyhow::Result<()> {
        let peer = peers
            .first()
            .with_context(|| "expected one peer at least")?;

        let mut stream = TcpStream::connect(peer).await?;
        let peer_id =
            torrent::HandshakeMessage::new(self.peer_id, self.torrent_metadata.info_hash_bytes)
                .initiate(&mut stream)
                .await
                .with_context(|| {
                    format!(
                        "error handshake initiate for info_hash: {}",
                        self.torrent_metadata.info_hash
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

        Self::download_from_stream(&mut stream, &self.output_path, &self.torrent_metadata)
            .await
            .with_context(|| "failed to download torrent")?;

        info!("success");

        Ok(())
    }

    async fn download_from_stream(
        stream: &mut TcpStream,
        output_path: &str,
        torrent_metadata: &TorrentFileMetadata,
    ) -> anyhow::Result<()> {
        // TODO download pieces in parallel batches and random order
        let mut downloaded_torrent = Vec::new();
        for piece_index in 0..torrent_metadata.pieces.len() {
            // TODO u32 vs usize indexing and conversion in protocol
            // what are safe assumptions to make about it
            let piece = Self::download_piece(stream, piece_index as i64, torrent_metadata).await?;
            downloaded_torrent.push(piece);
            debug!(
                "piece_index: {} downloaded successfully for info_hash: {}",
                piece_index, torrent_metadata.info_hash
            );
        }

        let bytes = downloaded_torrent.concat();
        if let Some(parent) = Path::new(output_path).parent() {
            fs::create_dir_all(parent).context("cannot create output directory")?;
        }

        let mut f = File::create(output_path).context("cannot create output file")?;
        f.write_all(&bytes)
            .with_context(|| format!("failed to write downloaded data to file {}", output_path))?;
        info!(
            "torrent info_hash: {} downloaded successfully to {}",
            torrent_metadata.info_hash, output_path
        );

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