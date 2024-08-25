use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use anyhow::{bail, Context};
use log::{debug, error, info};
use serde_bytes::ByteBuf;
use serde_derive::{Deserialize, Serialize};

use crate::torrent::Torrent;

#[derive(Debug)]
pub struct AnnounceRequest {
    announce_url: String,
    info_hash: String,
    left: i64,
}

impl From<&Torrent> for AnnounceRequest {
    fn from(value: &Torrent) -> Self {
        let announce_url = value.announce_url.clone();
        let info_hash = value.info_hash.clone();
        let left = value.torrent.length;
        AnnounceRequest {
            announce_url,
            info_hash,
            left,
        }
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct AnnounceResponse {
    pub interval: i64,
    pub peers: Vec<SocketAddr>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AnnounceResponseRaw {
    interval: i64,
    peers: ByteBuf,
}

impl From<&AnnounceResponseRaw> for AnnounceResponse {
    fn from(value: &AnnounceResponseRaw) -> Self {
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

pub async fn try_announce(request: AnnounceRequest) -> anyhow::Result<AnnounceResponse> {
    let client = reqwest::Client::new();

    let announce_url = request.announce_url;
    let info_hash = request.info_hash;
    let left = request.left;
    let url = format!("{announce_url}/?info_hash={info_hash}");

    info!("sending announce request to {}", url);

    let r = client
        .get(&url)
        .query(&[("peer_id", "00112233445566778899")])
        .query(&[("port", 6881)])
        .query(&[("uploaded", 0)])
        .query(&[("downloaded", 0)])
        .query(&[("left", left)])
        .query(&[("compact", 1)])
        .send()
        .await
        .with_context(|| format!("failed to send request to {}", url))?;

    if r.status().is_success() {
        let bytes = r.bytes().await.context("failed to read response bytes")?;
        let resp = serde_bencode::de::from_bytes::<AnnounceResponseRaw>(&bytes)
            .context("failed to deserialize announce response")?;
        debug!("announce response: {:?}", resp);
        Ok(AnnounceResponse::from(&resp))
    } else {
        let status = r.status();
        error!("announce request failed with status: {}", status);
        bail!("announce request failed with status: {}", status);
    }
}

#[cfg(test)]
mod tests {
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};

    use super::*;

    #[tokio::test]
    async fn test_try_announce_success() {
        let server = MockServer::start().await;
        let mock_peers = vec![127, 0, 0, 1, 0x1A, 0xE1];
        let mock_response = serde_bencode::to_bytes(&AnnounceResponseRaw {
            interval: 123,
            peers: ByteBuf::from(mock_peers),
        });
        assert!(mock_response.is_ok());

        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(mock_response.unwrap(), "application/x-bencode"),
            )
            .mount(&server)
            .await;

        let request = AnnounceRequest {
            announce_url: format!("{}/announce", server.uri()),
            info_hash: "mock_info_hash".to_string(),
            left: 0,
        };

        let result = try_announce(request).await;
        assert!(result.is_ok());

        let announce_response = result.unwrap();
        assert_eq!(announce_response.interval, 123);
        assert_eq!(announce_response.peers.len(), 1);
        assert_eq!(
            announce_response.peers[0],
            "127.0.0.1:6881".parse().unwrap()
        );
    }

    #[tokio::test]
    async fn test_try_announce_failure() {
        let server = MockServer::start().await;
        let mock_peers = vec![127, 0, 0, 1, 0x1A, 0xE1];
        let mock_response = serde_bencode::to_bytes(&AnnounceResponseRaw {
            interval: 123,
            peers: ByteBuf::from(mock_peers),
        });
        assert!(mock_response.is_ok());

        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let request = AnnounceRequest {
            announce_url: format!("{}/announce", server.uri()),
            info_hash: "mock_info_hash".to_string(),
            left: 0,
        };

        let result = try_announce(request).await;
        assert!(result.is_err());

        assert_eq!(
            result.unwrap_err().to_string(),
            "announce request failed with status: 500 Internal Server Error"
        );
    }

    #[tokio::test]
    async fn test_try_announce_failure_invalid_response() {
        let server = MockServer::start().await;
        let mock_invalid_response = serde_bencode::to_bytes(&String::from("some inalid content"));
        assert!(mock_invalid_response.is_ok());

        let mock_response = mock_invalid_response.unwrap();
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(mock_response, "application/x-bencode"),
            )
            .mount(&server)
            .await;

        let request = AnnounceRequest {
            announce_url: format!("{}/announce", server.uri()),
            info_hash: "mock_info_hash".to_string(),
            left: 0,
        };

        let result = try_announce(request).await;
        assert!(result.is_err());

        assert_eq!(
            result.unwrap_err().to_string(),
            "failed to deserialize announce response"
        );
    }
}
