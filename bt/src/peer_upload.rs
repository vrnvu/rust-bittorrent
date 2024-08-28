use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    sync::Arc,
    time::Duration,
};

use anyhow::{bail, Context};
use log::{debug, error, info, warn};
use tokio::{
    io,
    net::{TcpListener, TcpStream},
    time::timeout,
};

use crate::torrent::{self, PeerId, PeerMessage, TorrentFile};

const PEER_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_CONSECUTIVE_TIMEOUTS: usize = 3;

struct TorrentFileMetadata {
    file_path: String,
    info_hash: String,
    info_hash_bytes: [u8; 20],
    piece_length: i64,
}

impl TorrentFileMetadata {
    pub fn new(torrent: TorrentFile, file_path: &str) -> Self {
        Self {
            file_path: file_path.to_string(),
            info_hash: torrent.info_hash.clone(),
            info_hash_bytes: torrent.info_hash_bytes,
            piece_length: torrent.piece_length(),
        }
    }
}

pub struct PeerUpload {
    pub peer_id: PeerId,
}

impl PeerUpload {
    pub fn new() -> Self {
        PeerUpload {
            peer_id: PeerId::new(),
        }
    }

    pub async fn upload(
        self,
        torrent: TorrentFile,
        port: &str,
        file_path: &str,
    ) -> anyhow::Result<()> {
        let listener = TcpListener::bind(format!("127.0.0.1:{}", port)).await?;
        info!("Uploader listening on {}", listener.local_addr()?);
        let torrent_metadata = TorrentFileMetadata::new(torrent, file_path);
        let torrent_metadata = Arc::new(torrent_metadata);

        while let Ok((stream, _)) = listener.accept().await {
            let torrent_metadata_ref = Arc::clone(&torrent_metadata);
            tokio::spawn(async move {
                if let Err(e) = Self::handle_peer(stream, &torrent_metadata_ref, self.peer_id).await
                {
                    error!("Error handling peer: {:?}", e);
                }
            });
        }
        Ok(())
    }

    async fn handle_peer(
        mut stream: TcpStream,
        torrent_metadata: &TorrentFileMetadata,
        peer_id: PeerId,
    ) -> anyhow::Result<()> {
        let peer_addr = stream.peer_addr()?;
        info!("New peer connection established from {}", peer_addr);

        let peer_id = timeout(PEER_TIMEOUT, async {
            torrent::HandshakeMessage::new(peer_id, torrent_metadata.info_hash_bytes)
                .initiate(&mut stream)
                .await
                .with_context(|| {
                    format!(
                        "error handshake initiate for info_hash: {}",
                        torrent_metadata.info_hash
                    )
                })
        })
        .await
        .map_err(|_| anyhow::anyhow!("Handshake timed out"))??;
        info!("handshake success with peer {}", peer_id);

        // TODO: send bitfield to peer
        PeerMessage::Bitfield(vec![0]).send(&mut stream).await?;
        info!("bitfield send to peer");

        match PeerMessage::receive(&mut stream).await? {
            PeerMessage::Interested => {
                info!("peer is interested");
            }
            other => {
                error!("expected: Interested, got:{:?}", other);
                bail!("expected: Interested, got:{:?}", other);
            }
        }

        PeerMessage::Unchoke.send(&mut stream).await?;
        info!("unchoke send to peer");

        let mut consecutive_timeouts = 0;
        loop {
            match timeout(PEER_TIMEOUT, torrent::PeerMessage::receive(&mut stream)).await {
                Ok(Ok(message)) => {
                    consecutive_timeouts = 0;
                    debug!("Received message from {}: {:?}", peer_addr, message);

                    if let Err(e) = Self::handle_message(
                        torrent_metadata.file_path.as_str(),
                        message,
                        &mut stream,
                        torrent_metadata.piece_length,
                    )
                    .await
                    {
                        warn!("Error handling message from {}: {}", peer_addr, e);
                        break;
                    }
                }
                Ok(Err(e)) => {
                    if let Some(io_err) = e.downcast_ref::<io::Error>() {
                        if io_err.kind() == io::ErrorKind::UnexpectedEof {
                            info!("Peer {} disconnected", peer_addr);
                            break;
                        }
                    }
                    error!("Error receiving message from {}: {}", peer_addr, e);
                    break;
                }
                Err(_) => {
                    consecutive_timeouts += 1;
                    warn!("Timeout while waiting for message from {}", peer_addr);
                    if consecutive_timeouts >= MAX_CONSECUTIVE_TIMEOUTS {
                        warn!(
                            "Max consecutive timeouts reached for {}, closing connection",
                            peer_addr
                        );
                        break;
                    }
                }
            }
        }

        info!("Peer handling completed for {}", peer_addr);
        Ok(())
    }

    async fn handle_message(
        file_path: &str,
        message: torrent::PeerMessage,
        stream: &mut TcpStream,
        torrent_piece_length: i64,
    ) -> anyhow::Result<()> {
        match message {
            torrent::PeerMessage::Request {
                index,
                begin,
                length,
            } => {
                let piece = Self::read_piece(
                    file_path,
                    torrent_piece_length as u64,
                    index as u64,
                    begin as u64,
                    length as u64,
                )
                .await?;
                torrent::PeerMessage::Piece {
                    index,
                    begin,
                    block: piece,
                }
                .send(stream)
                .await?;
            }
            _ => {
                debug!("Unhandled message type: {:?}", message);
            }
        }
        Ok(())
    }

    async fn read_piece(
        file_path: &str,
        torrent_piece_length: u64,
        index: u64,
        begin: u64,
        length: u64,
    ) -> anyhow::Result<Vec<u8>> {
        // TODO keep files open in memory with mmap or open when needed and close when done
        // if we keep files open we can skip seek and just read the buffer
        // if users has a lot of large files it can run out of memory
        // we can try to use mmap to map the file into memory and keep files open for a memory lower than THRESHOLD
        let mut file = File::open(file_path)?;

        let offset = (index * torrent_piece_length) + begin;
        file.seek(SeekFrom::Start(offset))?;

        // TODO re-use buffers in a pool
        // because we know the max size of the buffer, we can use a fixed size
        // and instead of using a read_exact we can use a read_to_end and then
        // return a slice of the buffer
        // if length == MAX_LENGTH piece size
        let mut buffer = vec![0u8; length as usize];
        file.read_exact(&mut buffer)?;
        // TODO if length < MAX_LENGTH piece size
        // file.read_to_end(buf)
        Ok(buffer)
    }
}
