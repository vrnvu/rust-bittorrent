use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use anyhow::{bail, Context};
use log::{debug, error, info};
use serde_bytes::ByteBuf;
use serde_derive::Deserialize;

use crate::torrent::Torrent;

#[derive(Debug)]
#[allow(dead_code)]
pub struct AnnounceResponse {
    pub interval: i64,
    pub peers: Vec<SocketAddr>,
}

#[derive(Debug, Deserialize)]
struct AnnounceResponseRaw {
    interval: i64,
    peers: ByteBuf,
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
            peers,
        }
    }
}

pub async fn try_announce(torrent: &Torrent) -> anyhow::Result<AnnounceResponse> {
    let client = reqwest::Client::new();
    let announce_url = &torrent
        .torrent
        .announce
        .clone()
        .context("announce URL is missing")?;

    let info_hash = &torrent.info_hash;
    let url = format!("{announce_url}/?info_hash={info_hash}");

    info!("sending announce request to {}", url);

    let r = client
        .get(&url)
        .query(&[("peer_id", "00112233445566778899")])
        .query(&[("port", 6881)])
        .query(&[("uploaded", 0)])
        .query(&[("downloaded", 0)])
        .query(&[("left", &torrent.torrent.length)])
        .query(&[("compact", 1)])
        .send()
        .await
        .with_context(|| format!("failed to send request to {}", url))?;

    if r.status().is_success() {
        let bytes = r.bytes().await.context("failed to read response bytes")?;
        let resp = serde_bencode::de::from_bytes::<AnnounceResponseRaw>(&bytes)
            .context("failed to deserialize announce response")?;
        debug!("announce response: {:?}", resp);
        Ok(AnnounceResponse::from(resp))
    } else {
        let status = r.status();
        error!("announce request failed with status: {}", status);
        bail!("announce request failed with status: {}", status);
    }
}
