use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    net::SocketAddr,
    path::Path,
};

use anyhow::{bail, Context, Ok};
use log::{debug, error, info};
use sha1::{Digest, Sha1};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

const BLOCK_MAX_SIZE: u32 = 1 << 14;

pub enum TrackerProtocol {
    Udp,
    Tcp,
}

impl TryFrom<&str> for TrackerProtocol {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.starts_with("udp://") {
            Ok(TrackerProtocol::Udp)
        } else if value.starts_with("http://") || value.starts_with("https://") {
            Ok(TrackerProtocol::Tcp)
        } else {
            anyhow::bail!("unrecognized tracker protocol: {}", value)
        }
    }
}

pub struct TorrentFile {
    pub torrent: lava_torrent::torrent::v1::Torrent,
    pub announce_url: String,
    pub tracker_protocol: TrackerProtocol,
    pub info_hash: String,
    pub info_hash_bytes: [u8; 20],
}

impl TorrentFile {
    pub fn from_path<P>(path: P) -> anyhow::Result<Self>
    where
        P: AsRef<Path>,
    {
        let torrent =
            lava_torrent::torrent::v1::Torrent::read_from_file(&path).with_context(|| {
                format!("cannot read torrent from file: {}", path.as_ref().display())
            })?;

        let vec_info_hash_bytes = torrent.info_hash_bytes();
        let info_hash: String = form_urlencoded::byte_serialize(&vec_info_hash_bytes).collect();
        let info_hash_bytes: [u8; 20] = vec_info_hash_bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("Expected info hash to be exactly 20 bytes"))?;

        let announce_url = torrent
            .announce
            .as_ref()
            .context("announce url was expected in torrent file")?
            .to_string();

        let tracker_protocol = TrackerProtocol::try_from(announce_url.as_str())?;

        Ok(TorrentFile {
            torrent,
            announce_url,
            tracker_protocol,
            info_hash,
            info_hash_bytes,
        })
    }

    pub async fn download_piece(
        &self,
        stream: &mut TcpStream,
        piece_index: u32,
    ) -> anyhow::Result<Vec<u8>> {
        let piece_length = self
            .torrent
            .piece_length
            .min(self.torrent.length - (self.torrent.piece_length * piece_index as i64));

        let mut piece: Vec<u8> = Vec::with_capacity(piece_length as usize);
        let mut begin_offset: u32 = 0;
        let mut remain: u32 = piece_length as u32;
        while remain != 0 {
            let block_size = BLOCK_MAX_SIZE.min(remain);
            PeerMessage::Request {
                index: piece_index,
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
                assert_eq!(piece_index, index);
                assert_eq!(begin_offset, begin);
                piece.splice(begin as usize..begin as usize, block.into_iter());
            } else {
                error!("expected piece message from peer");
                bail!("expected piece message from peer");
            }
            begin_offset += block_size;
            remain -= block_size;
        }

        let piece_hash = self
            .torrent
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

    pub async fn download(&self, stream: &mut TcpStream, output_path: &str) -> anyhow::Result<()> {
        let mut downloaded_torrent = Vec::new();
        for (index, _) in self.torrent.pieces.iter().enumerate() {
            let piece = self.download_piece(stream, index as u32).await?;
            downloaded_torrent.push(piece);
            debug!(
                "piece_index: {} downloaded successfully for info_hash: {}",
                index, self.info_hash
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
            self.info_hash, output_path
        );

        Ok(())
    }

    pub async fn read_piece(&self, index: u32, begin: u32, length: u32) -> anyhow::Result<Vec<u8>> {
        let file_path = "test.txt"; // Hardcoded for simplicity
        let mut file = File::open(file_path)?;
        let offset = (index as u64 * self.torrent.piece_length as u64) + begin as u64;
        file.seek(SeekFrom::Start(offset))?;
        let mut buffer = vec![0u8; length as usize];
        file.read_exact(&mut buffer)?;
        Ok(buffer)
    }
}

#[derive(Debug)]
pub struct HandshakeMessage {
    peer_id: [u8; 20],
    info_hash_bytes: [u8; 20],
    buffer: Vec<u8>,
    stream: Option<TcpStream>,
}

// TODO: clean implementation
impl HandshakeMessage {
    pub fn new(info_hash_bytes: [u8; 20]) -> Self {
        let peer_id: [u8; 20] = [0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9];
        Self {
            peer_id,
            info_hash_bytes,
            buffer: Vec::with_capacity(68),
            stream: None,
        }
    }

    pub async fn initiate(&mut self, peer: &SocketAddr) -> anyhow::Result<PeerStream> {
        self.send(peer).await?.receive().await
    }

    async fn send(&mut self, peer: &SocketAddr) -> anyhow::Result<&mut Self> {
        let mut stream = TcpStream::connect(peer)
            .await
            .context("failed to connect to peer")?;
        self.buffer.push(19);
        self.buffer.extend("BitTorrent protocol".as_bytes());
        self.buffer.extend(&[0_u8; 8]);
        self.buffer.extend(self.info_hash_bytes);
        self.buffer.extend(self.peer_id);
        stream
            .write_all(&self.buffer)
            .await
            .context("failed to send handshake")?;
        self.stream = Some(stream);
        Ok(self)
    }

    async fn receive(&mut self) -> anyhow::Result<PeerStream> {
        if let Some(mut stream) = self.stream.take() {
            stream
                .read_exact(&mut self.buffer)
                .await
                .context("failed to receive handshake")?;
            let peer_id = &self.buffer[48..];
            Ok(PeerStream {
                peer_id: hex::encode(peer_id),
                stream,
            })
        } else {
            error!("stream was not initialized in Handshake");
            bail!("stream was not initialized in Handshake");
        }
    }
}

#[derive(Debug)]
pub struct PeerStream {
    pub peer_id: String,
    pub stream: TcpStream,
}

#[derive(Debug)]
pub enum PeerMessage {
    Unchoke,
    Interested,
    Bitfield(Vec<u8>),
    Request {
        index: u32,
        begin: u32,
        length: u32,
    },
    Piece {
        index: u32,
        begin: u32,
        block: Vec<u8>,
    },
}

#[derive(Debug)]
enum MessageTag {
    Unchoke = 1,
    Interested = 2,
    Bitfield = 5,
    Request = 6,
    Piece = 7,
}

impl TryFrom<u8> for MessageTag {
    type Error = anyhow::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Unchoke),
            2 => Ok(Self::Interested),
            5 => Ok(Self::Bitfield),
            6 => Ok(Self::Request),
            7 => Ok(Self::Piece),
            _ => panic!("invalid message tag"),
        }
    }
}

impl PeerMessage {
    pub async fn send(&self, stream: &mut TcpStream) -> anyhow::Result<()> {
        let buffer = match self {
            PeerMessage::Interested => PeerMessage::buffer_interested().await?,
            PeerMessage::Request {
                index,
                begin,
                length,
            } => PeerMessage::buffer_request(*index, *begin, *length).await?,
            PeerMessage::Piece {
                index,
                begin,
                block,
            } => PeerMessage::buffer_piece(*index, *begin, block).await?,
            _ => bail!("unexpected message type: {:?}, not implemented yet", self),
        };
        self.write_message(&buffer, stream).await?;
        Ok(())
    }

    async fn write_message(&self, buffer: &[u8], stream: &mut TcpStream) -> anyhow::Result<()> {
        stream.write_u32(buffer.len() as u32).await?;
        stream.write_all(buffer).await?;
        Ok(())
    }

    async fn buffer_piece(index: u32, begin: u32, block: &[u8]) -> anyhow::Result<Vec<u8>> {
        let mut buffer = Vec::with_capacity(9 + block.len()); // 1 byte for tag + 2 * 4 bytes for u32 values + block length
        buffer.push(MessageTag::Piece as u8);
        buffer.extend_from_slice(&index.to_be_bytes());
        buffer.extend_from_slice(&begin.to_be_bytes());
        buffer.extend_from_slice(block);
        Ok(buffer)
    }

    async fn buffer_interested() -> anyhow::Result<Vec<u8>> {
        let buffer = vec![MessageTag::Interested as u8];
        Ok(buffer)
    }

    async fn buffer_request(index: u32, begin: u32, length: u32) -> anyhow::Result<Vec<u8>> {
        let mut buffer = Vec::with_capacity(13); // 1 byte for tag + 3 * 4 bytes for u32 values
        buffer.push(MessageTag::Request as u8);
        buffer.extend_from_slice(&index.to_be_bytes());
        buffer.extend_from_slice(&begin.to_be_bytes());
        buffer.extend_from_slice(&length.to_be_bytes());
        Ok(buffer)
    }

    pub async fn receive(stream: &mut TcpStream) -> anyhow::Result<Self> {
        let message_size = Self::receive_message_size(stream).await?;
        let message_tag: MessageTag = {
            let message_tag = Self::receive_message_tag(stream).await?;
            message_tag.try_into().with_context(|| {
                format!("failed to convert message tag to enum: {}", message_tag)
            })?
        };

        let peer_message = match message_tag {
            MessageTag::Unchoke => PeerMessage::Unchoke,
            MessageTag::Interested => PeerMessage::Interested,
            MessageTag::Bitfield => {
                let buff = Self::receive_bitfield_payload(message_size, stream).await?;
                PeerMessage::Bitfield(buff)
            }
            MessageTag::Piece => {
                let index = stream
                    .read_u32()
                    .await
                    .context("failed to read Piece index")?;
                let begin = stream
                    .read_u32()
                    .await
                    .context("failed to read Piece begin offset")?;
                let block = Self::receive_piece_block(message_size, stream).await?;
                PeerMessage::Piece {
                    index,
                    begin,
                    block,
                }
            }
            MessageTag::Request => {
                let index = stream
                    .read_u32()
                    .await
                    .context("failed to read Piece index")?;
                let begin = stream
                    .read_u32()
                    .await
                    .context("failed to read Piece begin offset")?;
                let length = stream
                    .read_u32()
                    .await
                    .context("failed to read Piece length")?;
                PeerMessage::Request {
                    index,
                    begin,
                    length,
                }
            }
        };
        Ok(peer_message)
    }

    async fn receive_message_tag(stream: &mut TcpStream) -> anyhow::Result<u8> {
        let tag = stream
            .read_u8()
            .await
            .context("failed to read message tag")?;

        Ok(tag)
    }

    async fn receive_message_size(stream: &mut TcpStream) -> anyhow::Result<u32> {
        let mut size = stream
            .read_u32()
            .await
            .context("failed to read message size")?;

        while size == 0 {
            size = stream
                .read_u32()
                .await
                .context("failed to read message size")?;
        }

        Ok(size)
    }

    async fn receive_bitfield_payload(
        message_size: u32,
        stream: &mut TcpStream,
    ) -> anyhow::Result<Vec<u8>> {
        let mut buff = vec![0; message_size as usize - 1];
        stream
            .read_exact(&mut buff)
            .await
            .context("failed to read Bitfield payload")?;

        Ok(buff)
    }

    async fn receive_piece_block(
        message_size: u32,
        stream: &mut TcpStream,
    ) -> anyhow::Result<Vec<u8>> {
        // Allocate a buffer for the piece block data
        // message_size includes:
        // - 1 byte for the message ID
        // - 4 bytes for the piece index
        // - 4 bytes for the block offset
        // - The actual block data
        // So we subtract 9 (1 + 4 + 4) to get the size of the block data
        let mut block = vec![0; message_size as usize - 9];
        stream
            .read_exact(&mut block)
            .await
            .context("failed to read Piece block")?;

        Ok(block)
    }
}

#[cfg(test)]
mod tests {
    use tokio::{net::TcpListener, sync::oneshot};

    use super::*;

    #[tokio::test]
    async fn test_read_http_torrent_from_path() {
        let path = "tests/sample-http.torrent";

        let torrent = TorrentFile::from_path(path);
        assert!(torrent.is_ok());

        assert_eq!(
            "http://bittorrent-test-tracker.codecrafters.io/announce",
            torrent.unwrap().torrent.announce.unwrap()
        );
    }

    #[tokio::test]
    async fn test_peer_handshake() -> anyhow::Result<()> {
        let mock_server = TcpListener::bind("127.0.0.1:0").await?;
        let mock_addr = mock_server.local_addr()?;

        let (tx, rx) = oneshot::channel();

        tokio::spawn(async move {
            if let Result::Ok((mut socket, _)) = mock_server.accept().await {
                let mut buffer = [0; 68];
                let _ = socket.read_exact(&mut buffer).await;
                let _ = socket.write_all(&buffer).await;

                // Notify the test that the handshake was received
                let _ = tx.send(());
            }
        });

        let mock_info_hash_bytes = [0_u8; 20];
        let mut handshake = HandshakeMessage::new(mock_info_hash_bytes);

        handshake.send(&mock_addr).await?;

        // Wait for the mock peer to respond
        rx.await?;

        let peer_stream = handshake.receive().await?;
        assert_eq!(
            peer_stream.peer_id,
            hex::encode([0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9])
        );

        Ok(())
    }
}
