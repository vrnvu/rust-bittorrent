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

pub struct PieceRequest {
    piece_index: i64,
    piece_length: u32,
    piece_hash: Vec<u8>,
}

impl PieceRequest {
    pub fn new(torrent_metadata: &TorrentFileMetadata, piece_index: i64) -> Self {
        let piece_length = torrent_metadata
            .piece_length
            .min(torrent_metadata.length - (torrent_metadata.piece_length * piece_index))
            as u32;
        let piece_hash = torrent_metadata.pieces[piece_index as usize].clone();
        PieceRequest {
            piece_index,
            piece_length,
            piece_hash,
        }
    }
}

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

    pub async fn download(&self, streams: Vec<TcpStream>) -> anyhow::Result<()> {
        let mut piece_requests = Vec::new();
        for piece_index in 0..self.torrent_metadata.pieces.len() {
            let piece_request = PieceRequest::new(&self.torrent_metadata, piece_index as i64);
            piece_requests.push(piece_request);
        }
        let pieces_to_download = piece_requests.len();

        let (input_tx, input_rx) = async_channel::unbounded();
        let (output_tx, output_rx) = async_channel::unbounded();

        for stream in streams.into_iter() {
            let input_rx = input_rx.clone();
            let output_tx = output_tx.clone();

            tokio::spawn(async move {
                Self::start_stream_downloader(stream, input_rx, output_tx).await
            });
        }

        for piece_request in piece_requests {
            let input_tx = input_tx.clone();
            tokio::spawn(async move { input_tx.send(piece_request).await });
        }

        let mut downloaded_torrent = Vec::new();
        while let Ok(piece) = output_rx.recv().await {
            match piece {
                Ok(piece) => {
                    downloaded_torrent.push(piece);
                    if downloaded_torrent.len() == pieces_to_download {
                        break;
                    }
                }
                Err(e) => {
                    error!("failed to download piece: {}", e);
                }
            }
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

    async fn start_stream_downloader(
        mut stream: TcpStream,
        input_rx: async_channel::Receiver<PieceRequest>,
        output_tx: async_channel::Sender<anyhow::Result<Vec<u8>>>,
    ) -> anyhow::Result<()> {
        while let Ok(piece_request) = input_rx.recv().await {
            let piece = Self::download_piece(&mut stream, piece_request).await?;
            output_tx
                .send(Ok(piece))
                .await
                .with_context(|| "failed to send piece to output_tx")?;
        }
        Ok(())
    }

    pub async fn download_piece(
        stream: &mut TcpStream,
        piece_request: PieceRequest,
    ) -> anyhow::Result<Vec<u8>> {
        let PieceRequest {
            piece_index,
            piece_length,
            piece_hash,
        } = piece_request;

        let mut piece: Vec<u8> = Vec::with_capacity(piece_length as usize);
        let mut begin_offset: u32 = 0;
        let mut remain: u32 = piece_length;
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
