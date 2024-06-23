use std::{io::Write, net::SocketAddr, path::Path};

use anyhow::{bail, Context, Ok};
use sha1::{Digest, Sha1};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

const BLOCK_MAX_SIZE: u32 = 1 << 14;

pub struct Torrent {
    pub torrent: lava_torrent::torrent::v1::Torrent,
    pub info_hash: String,
    pub info_hash_bytes: [u8; 20],
}

impl Torrent {
    pub fn from_path<P>(path: P) -> anyhow::Result<Self>
    where
        P: AsRef<Path>,
    {
        let torrent = lava_torrent::torrent::v1::Torrent::read_from_file(path)
            .expect("cannot read torrent from file");

        let vec_info_hash_bytes = torrent.info_hash_bytes();
        let info_hash: String = form_urlencoded::byte_serialize(&vec_info_hash_bytes).collect();
        let mut info_hash_bytes = [0u8; 20];
        info_hash_bytes.copy_from_slice(&vec_info_hash_bytes);

        Ok(Torrent {
            torrent,
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
            .await?;

            if let PeerMessage::Piece {
                index,
                begin,
                block,
            } = PeerMessage::receive(stream).await?
            {
                assert_eq!(piece_index, index);
                assert_eq!(begin_offset, begin);
                piece.splice(begin as usize..begin as usize, block.into_iter());
            } else {
                bail!("expected piece message from peer")
            }
            begin_offset += block_size;
            remain -= block_size as u32;
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
                "want: {}, got: {}",
                hex::encode(piece_hash),
                hex::encode(current_hash)
            )
        }

        Ok(piece)
    }

    pub async fn download(&self, stream: &mut TcpStream) -> anyhow::Result<()> {
        let mut downloaded_torrent = Vec::new();
        for (index, _) in self.torrent.pieces.iter().enumerate() {
            let piece = self.download_piece(stream, index as u32).await?;
            downloaded_torrent.push(piece);
            dbg!("piece downloaded successfully");
        }
        let output_path = "done.txt";
        let mut f = std::fs::File::create(output_path)?;
        let bytes = downloaded_torrent.concat();
        f.write_all(&bytes)?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct HandshakeMessage {
    peer_id: [u8; 20],
    info_hash_bytes: [u8; 20],
    buffer: Vec<u8>,
    stream: Option<TcpStream>,
}

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

    pub async fn send(&mut self, peer: &SocketAddr) -> anyhow::Result<&mut Self> {
        let mut stream = TcpStream::connect(peer).await?;
        self.buffer.push(19);
        self.buffer.extend("BitTorrent protocol".as_bytes());
        self.buffer.extend(&[0_u8; 8]);
        self.buffer.extend(self.info_hash_bytes);
        self.buffer.extend(self.peer_id);
        stream.write_all(&self.buffer).await?;
        self.stream = Some(stream);
        Ok(self)
    }

    pub async fn receive(&mut self) -> anyhow::Result<PeerStream> {
        if let Some(mut stream) = self.stream.take() {
            stream.read_exact(&mut self.buffer).await?;
            assert!(self.buffer.len() == 68);
            let peer_id = &self.buffer[48..];
            Ok(PeerStream {
                peer_id: hex::encode(&peer_id),
                stream: stream,
            })
        } else {
            bail!("stream was not initialized in Handshake")
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
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have,
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
    Cancel,
}

impl PeerMessage {
    pub async fn send(&self, stream: &mut TcpStream) -> anyhow::Result<()> {
        let mut buffer = Vec::new();
        match self {
            PeerMessage::Choke => todo!(),
            PeerMessage::Unchoke => todo!(),
            PeerMessage::Interested => {
                buffer.write_u8(2).await?;
            }
            PeerMessage::NotInterested => todo!(),
            PeerMessage::Have => todo!(),
            PeerMessage::Bitfield(_) => todo!(),
            PeerMessage::Request {
                index,
                begin,
                length,
            } => {
                buffer.write_u8(6).await?;
                buffer.write_u32(*index).await?;
                buffer.write_u32(*begin).await?;
                buffer.write_u32(*length).await?;
            }
            PeerMessage::Piece {
                index,
                begin,
                block,
            } => todo!(),
            PeerMessage::Cancel => todo!(),
        }
        stream.write_u32(buffer.len() as u32).await?;
        stream.write_all(&buffer).await?;
        Ok(())
    }

    pub async fn receive(stream: &mut TcpStream) -> anyhow::Result<Self> {
        let mut size = stream.read_u32().await?;
        while size == 0 {
            size = stream.read_u32().await?;
        }

        let tag = stream.read_u8().await?;
        match tag {
            1 => Ok(Self::Unchoke),
            5 => {
                let mut buff = vec![0; size as usize - 1];
                stream.read_exact(&mut buff).await?;
                Ok(Self::Bitfield(buff))
            }
            7 => {
                let index = stream.read_u32().await?;
                let begin = stream.read_u32().await?;
                let mut block = vec![0; size as usize - 8 - 1]; // 2 * u32 - 1
                stream.read_exact(&mut block).await?;
                Ok(Self::Piece {
                    index,
                    begin,
                    block,
                })
            }
            _ => bail!("unexpected tag message received, not implemented yet"),
        }
    }
}
