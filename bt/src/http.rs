use anyhow::Context;
use log::{debug, error, info};
use models::{AnnounceResponse, AnnounceResponseRaw, RegisterRequest};
use serde_derive::Serialize;

use crate::torrent::{PeerId, TorrentFile};

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

pub async fn try_announce(
    request: AnnounceRequest,
    peer_id: &PeerId,
) -> anyhow::Result<AnnounceResponse> {
    let announce_url = request.announce_url;
    let info_hash = request.info_hash;
    let left = request.left;

    info!("sending announce request to {}", announce_url);

    let response = reqwest::Client::new()
        .get(&announce_url)
        .query(&[("info_hash", info_hash)])
        .query(&[("peer_id", peer_id.to_string())])
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
        Ok(resp.into())
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

pub async fn try_register(
    peer_id: &PeerId,
    port: u16,
    torrent: &TorrentFile,
) -> anyhow::Result<()> {
    let announce_url = torrent.announce_url.clone();
    let info_hash = torrent.info_hash.clone();
    let name = torrent.torrent.name.clone();

    let register_request = RegisterRequest {
        name,
        info_hash,
        peer_id: peer_id.to_string(),
        ip: "127.0.0.1".to_string(),
        port,
    };
    info!(
        "sending register request {:?} to {announce_url}",
        register_request
    );

    let body = serde_bencode::to_bytes(&register_request)?;
    let response = reqwest::Client::new()
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

pub async fn try_list_files(announce_url: &str) -> anyhow::Result<Vec<String>> {
    let url = format!("{}/files", announce_url);
    let response = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .with_context(|| format!("failed to send list_files request to {}", url))?;
    if response.status().is_success() {
        let body = response
            .text()
            .await
            .context("failed to read response body")?;
        let files: Vec<String> = serde_json::from_str(&body)?;
        Ok(files)
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
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};

    use super::*;

    #[tokio::test]
    async fn test_try_announce_success() {
        let server = MockServer::start().await;
        let mock_peers = vec![
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 6881),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 0, 1)), 8080),
        ];
        let announce_response_raw = AnnounceResponseRaw::from_socket_addrs(123, &mock_peers);
        let mock_response = serde_bencode::to_bytes(&announce_response_raw);
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

        let peer_id = PeerId::new();
        let result = try_announce(request, &peer_id).await;
        assert!(result.is_ok());

        let announce_response = result.unwrap();
        assert_eq!(announce_response.interval, 123);
        assert_eq!(announce_response.peers.len(), 2);
        assert_eq!(
            announce_response.peers[0],
            "127.0.0.1:6881".parse().unwrap()
        );
        assert_eq!(
            announce_response.peers[1],
            "192.168.0.1:8080".parse().unwrap()
        );
    }

    #[tokio::test]
    async fn test_try_announce_failure() {
        let server = MockServer::start().await;
        let mock_peers = vec![
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 6881),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 0, 1)), 8080),
        ];
        let announce_response_raw = AnnounceResponseRaw::from_socket_addrs(123, &mock_peers);
        let mock_response = serde_bencode::to_bytes(&announce_response_raw);
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

        let peer_id = PeerId::new();
        let result = try_announce(request, &peer_id).await;
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

        let peer_id = PeerId::new();
        let result = try_announce(request, &peer_id).await;
        assert!(result.is_err());

        assert!(result
            .unwrap_err()
            .to_string()
            .contains("failed to deserialize announce response"));
    }
}
