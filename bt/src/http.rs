use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use anyhow::Context;
use log::{debug, error, info};
use serde_bytes::ByteBuf;
use serde_derive::{Deserialize, Serialize};

use crate::torrent::TorrentFile;

#[derive(Debug, Serialize, Clone)]
pub struct RegisterRequest {
    pub info_hash: String,
    pub peer_id: String,
    pub ip: String,
    pub port: u16,
}

#[derive(Debug, Serialize)]
pub struct AnnounceRequest {
    announce_url: String,
    info_hash: String,
    left: i64,
}

impl From<&TorrentFile> for AnnounceRequest {
    fn from(value: &TorrentFile) -> Self {
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
    let announce_url = request.announce_url;
    let info_hash = request.info_hash;
    let left = request.left;

    info!("sending announce request to {}", announce_url);

    let response = reqwest::Client::new()
        .get(&announce_url)
        .query(&[("info_hash", info_hash)])
        .query(&[("peer_id", "00112233445566778899")])
        .query(&[("port", 6881)])
        .query(&[("uploaded", 0)])
        .query(&[("downloaded", 0)])
        .query(&[("left", left)])
        .query(&[("compact", 1)])
        .send()
        .await
        .with_context(|| format!("failed to send request to {}", announce_url))?;

    if response.status().is_success() {
        let bytes = response
            .bytes()
            .await
            .context("failed to read response bytes")?;
        let resp = match serde_bencode::de::from_bytes::<AnnounceResponseRaw>(&bytes) {
            Ok(resp) => resp,
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "failed to deserialize announce response: {}. Response body: {:?}",
                    e,
                    String::from_utf8_lossy(&bytes)
                ));
            }
        };
        debug!("announce response: {:?}", resp);
        Ok(AnnounceResponse::from(&resp))
    } else {
        let status = response.status();
        let error_body = response
            .text()
            .await
            .unwrap_or_else(|e| format!("Failed to read error body: {}", e));
        error!(
            "Announce request failed with status: {}. Error body: {}",
            status, error_body
        );
        Err(anyhow::anyhow!(
            "Announce request failed with status: {}. Error body: {}",
            status,
            error_body
        ))
    }
}

pub async fn try_register(torrent: &TorrentFile) -> anyhow::Result<()> {
    let client = reqwest::Client::new();

    let announce_url = torrent.announce_url.clone();
    let info_hash = torrent.info_hash.clone();

    let register_request = RegisterRequest {
        info_hash,
        peer_id: "33333333336666666666".to_string(), // TODO peer_id
        ip: "127.0.0.1".to_string(),
        port: 6881,
    };
    info!(
        "sending register request {:?} to {announce_url}",
        register_request
    );

    let body = serde_bencode::to_bytes(&register_request)?;
    let response = client
        .post(&announce_url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .with_context(|| format!("failed to send request to {}", announce_url))?;

    if response.status().is_success() {
        Ok(())
    } else {
        let status = response.status();
        error!("register request failed with status: {}", status);
        Err(anyhow::anyhow!(
            "register request failed with status: {}",
            status
        ))
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
            "Announce request failed with status: 500 Internal Server Error. Error body: "
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

        assert!(result
            .unwrap_err()
            .to_string()
            .contains("failed to deserialize announce response"));
    }
}
