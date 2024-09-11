use std::{
    fmt::{Display, Formatter},
    path::Path,
};

use anyhow::{Context, Ok};
use lava_torrent::{bencode::BencodeElem, torrent::v1::TorrentBuilder};
use rand::Rng;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

const PEER_ID_BT_VERSION: &str = "-BT0001-";

#[derive(Debug, Clone, Copy)]
pub struct PeerId([u8; 20]);

impl PeerId {
    pub fn new() -> Self {
        let mut peer_id = [0u8; 20];
        peer_id[..8].copy_from_slice(PEER_ID_BT_VERSION.as_bytes());
        peer_id.iter_mut().skip(8).for_each(|byte| {
            *byte = rand::thread_rng().gen_range(b'0'..=b'z');
            while !byte.is_ascii_alphanumeric() {
                *byte = rand::thread_rng().gen_range(b'0'..=b'z');
            }
        });
        PeerId(peer_id)
    }
}

impl Display for PeerId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", String::from_utf8_lossy(&self.0))
    }
}

#[derive(Debug, Clone)]
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
    // TODO
    #[allow(dead_code)]
    pub fn try_as_torrent_file<P>(path: P, tracker_port: u16) -> anyhow::Result<Self>
    where
        P: AsRef<Path>,
    {
        const PIECE_LENGTH: i64 = 32 * 1024; // n * 1024 KiB

        // TODO we set announce and tracker manually here to build a .torrent file
        let host = "127.0.0.1";
        let port = tracker_port;
        let announce_url = format!("http://{}:{}/announce", host, port);

        let torrent = TorrentBuilder::new(&path, PIECE_LENGTH)
            .set_announce(Some(announce_url.to_owned()))
            .add_extra_field(
                "encoding".to_owned(),
                BencodeElem::String("UTF-8".to_owned()),
            )
            .build()?;

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

    // TODO
    #[allow(dead_code)]
    fn write_torrent_file<P>(path: P, output_path: P) -> anyhow::Result<()>
    where
        P: AsRef<Path>,
    {
        const PIECE_LENGTH: i64 = 32 * 1024; // n * 1024 KiB

        // TODO
        let host = "127.0.0.1";
        let port = 9999;
        let announce_url = format!("http://{}:{}/announce", host, port);

        TorrentBuilder::new(&path, PIECE_LENGTH)
            .set_announce(Some(announce_url.to_owned()))
            .add_extra_field(
                "encoding".to_owned(),
                BencodeElem::String("UTF-8".to_owned()),
            )
            .build()?
            .write_into_file(output_path)
            .with_context(|| format!("cannot write file as .torrent: {}", path.as_ref().display()))
    }

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

    // TODO i64 is it safe to return as u64?
    pub fn piece_length(&self) -> i64 {
        self.torrent.piece_length
    }

    pub fn pieces(&self) -> Vec<Vec<u8>> {
        self.torrent.pieces.clone()
    }

    pub fn length(&self) -> i64 {
        self.torrent.length
    }
}

#[derive(Debug)]
pub struct HandshakeMessage {
    // TODO should we use PeerId instead of [u8; 20]?
    peer_id: [u8; 20],
    info_hash_bytes: [u8; 20],
}

impl HandshakeMessage {
    pub fn new(peer_id: PeerId, info_hash_bytes: [u8; 20]) -> Self {
        Self {
            peer_id: peer_id.0,
            info_hash_bytes,
        }
    }

    pub async fn initiate(&mut self, peer_stream: &mut TcpStream) -> anyhow::Result<String> {
        self.send(peer_stream).await?.receive(peer_stream).await
    }

    async fn send(&mut self, peer_stream: &mut TcpStream) -> anyhow::Result<&mut Self> {
        let mut buffer = Vec::with_capacity(68);
        buffer.push(19);
        buffer.extend("BitTorrent protocol".as_bytes());
        buffer.extend(&[0_u8; 8]);
        buffer.extend(self.info_hash_bytes);
        buffer.extend(self.peer_id);

        peer_stream
            .write_all(&buffer)
            .await
            .context("failed to send handshake")?;

        Ok(self)
    }

    async fn receive(&mut self, peer_stream: &mut TcpStream) -> anyhow::Result<String> {
        let mut buffer = [0; 68];
        peer_stream
            .read_exact(&mut buffer)
            .await
            .context("failed to receive handshake")?;

        let peer_id = &buffer[48..];
        // TODO we do not need hex representation of peer id for our own tracker as we control the representation
        let peer_id_string = String::from_utf8(peer_id.to_vec())?;
        Ok(peer_id_string)
    }
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
            PeerMessage::Bitfield(bitfield) => PeerMessage::buffer_bitfield(bitfield).await?,
            PeerMessage::Unchoke => PeerMessage::buffer_unchoke().await?,
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

    async fn buffer_bitfield(bitfield: &[u8]) -> anyhow::Result<Vec<u8>> {
        let mut buffer = Vec::with_capacity(1 + bitfield.len()); // 1 byte for tag + bitfield length
        buffer.push(MessageTag::Bitfield as u8);
        buffer.extend_from_slice(bitfield);
        Ok(buffer)
    }

    async fn buffer_unchoke() -> anyhow::Result<Vec<u8>> {
        let buffer = vec![MessageTag::Unchoke as u8];
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
    async fn test_write_torrent_file() {
        let path = "tests/test-file.txt";
        let temporal_file = tempfile::NamedTempFile::new().unwrap();
        let output_path = temporal_file.path().to_str().unwrap();
        let torrent = TorrentFile::write_torrent_file(path, output_path);
        assert!(torrent.is_ok());
    }

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
        let mock_peer_id = PeerId::new();
        let mut handshake = HandshakeMessage::new(mock_peer_id, mock_info_hash_bytes);

        let mut stream = TcpStream::connect(mock_addr).await?;
        handshake.send(&mut stream).await?;

        // Wait for the mock peer to respond
        rx.await?;

        let peer_id = handshake.receive(&mut stream).await?;
        assert_eq!(peer_id, mock_peer_id.to_string());

        Ok(())
    }

    #[tokio::test]
    async fn test_generate_peer_id() {
        let peer_id = PeerId::new();
        assert!(peer_id.to_string().contains(PEER_ID_BT_VERSION));
        assert!(peer_id.to_string().len() == 20);
    }
}
