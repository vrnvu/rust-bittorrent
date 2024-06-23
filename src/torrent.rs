use std::{net::SocketAddr, path::Path};

use anyhow::{bail, Context};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

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

pub enum PeerMessage {
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have,
    Bitfield(Vec<u8>),
    Request,
    Piece,
    Cancel,
}

impl PeerMessage {
    pub async fn send(&self, stream: &mut TcpStream) -> anyhow::Result<()> {
        match self {
            PeerMessage::Choke => todo!(),
            PeerMessage::Unchoke => todo!(),
            PeerMessage::Interested => {
                stream.write_u32(1).await?;
                stream.write_u8(2).await?;
            }
            PeerMessage::NotInterested => todo!(),
            PeerMessage::Have => todo!(),
            PeerMessage::Bitfield(_) => todo!(),
            PeerMessage::Request => todo!(),
            PeerMessage::Piece => todo!(),
            PeerMessage::Cancel => todo!(),
        }
        Ok(())
    }
    pub async fn receive(stream: &mut TcpStream) -> anyhow::Result<Self> {
        dbg!("receive fun call");
        let mut size = stream.read_u32().await?;
        dbg!(&size);
        while size == 0 {
            size = stream.read_u32().await?;
            dbg!(&size);
        }

        let tag = stream.read_u8().await?;
        match tag {
            1 => Ok(Self::Unchoke),
            5 => {
                dbg!(tag);
                let mut buff = vec![0; size as usize - 1];
                stream.read_exact(&mut buff).await?;
                dbg!(&buff);
                Ok(Self::Bitfield(buff))
            }
            _ => bail!("unexpected tag message received, not implemented yet"),
        }
    }
}
