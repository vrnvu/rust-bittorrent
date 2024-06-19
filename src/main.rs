use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use anyhow::{bail, Context};
use lava_torrent::torrent::v1::Torrent;
use serde_bytes::ByteBuf;
use serde_derive::Deserialize;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

#[derive(Debug, Deserialize)]
struct AnnounceResponseRaw {
    interval: i64,
    peers: ByteBuf,
}

#[derive(Debug)]
#[allow(dead_code)]
struct AnnounceResponse {
    interval: i64,
    peers: Vec<SocketAddr>,
}

impl From<AnnounceResponseRaw> for AnnounceResponse {
    fn from(value: AnnounceResponseRaw) -> Self {
        let mut peers = Vec::new();
        for chunk in value.peers.chunks(6) {
            let ip = Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]);
            // Extract the port part (last 2 bytes) and convert to u16
            let port = ((chunk[4] as u16) << 8) | (chunk[5] as u16);
            peers.push(SocketAddr::new(IpAddr::V4(ip), port));
        }
        AnnounceResponse {
            interval: value.interval,
            peers: peers,
        }
    }
}

async fn get_announce(torrent: &Torrent, info_hash: &str) -> anyhow::Result<AnnounceResponse> {
    let client = reqwest::Client::new();
    let announce_url = &torrent
        .announce
        .clone()
        .with_context(|| "Announce URL is missing")?;
    let url = format!("{announce_url}/?info_hash={info_hash}");
    let r = client
        .get(url)
        .query(&[("peer_id", "00112233445566778899")])
        .query(&[("port", 6881)])
        .query(&[("uploaded", 0)])
        .query(&[("downloaded", 0)])
        .query(&[("left", &torrent.length)])
        .query(&[("compact", 1)])
        .send()
        .await?;
    if r.status().is_success() {
        let bytes = r.bytes().await?;
        let resp = serde_bencode::de::from_bytes::<AnnounceResponseRaw>(&bytes)?;
        return Ok(AnnounceResponse::from(resp));
    } else {
        bail!(r.status())
    }
}

async fn handshake(info_hash_bytes: &[u8], peer: &SocketAddr) -> anyhow::Result<String> {
    let mut stream = TcpStream::connect(peer).await?;

    let peer_id: [u8; 20] = [0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9];
    let mut buffer: Vec<u8> = Vec::with_capacity(68);
    buffer.push(19);
    buffer.extend("BitTorrent protocol".as_bytes());
    buffer.extend(&[0_u8; 8]);
    buffer.extend(info_hash_bytes);
    buffer.extend(&peer_id);
    stream.write_all(&buffer).await?;

    stream.read_exact(&mut buffer).await?;
    assert!(buffer.len() == 68);
    let peer_id = &buffer[48..];
    Ok(hex::encode(&peer_id))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let path = "sample.torrent";
    let torrent: Torrent = Torrent::read_from_file(path).expect("cannot read from file");
    let info_hash_bytes = torrent.info_hash_bytes();
    let info_hash: String = form_urlencoded::byte_serialize(&info_hash_bytes).collect();
    let announce_response = get_announce(&torrent, &info_hash).await?;
    assert!(!announce_response.peers.is_empty());

    for peer in announce_response.peers {
        let peer_id = handshake(&info_hash_bytes, &peer).await?;
        dbg!(peer_id);
    }

    Ok(())
}
