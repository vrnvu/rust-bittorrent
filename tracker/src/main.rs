use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use warp::Filter;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AnnounceRequest {
    pub info_hash: String,
    pub peer_id: String,
    pub ip: String,
    pub port: u16,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AnnounceResponse {
    pub interval: u32,
    pub peers: Vec<PeerInfo>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PeerInfo {
    pub peer_id: String,
    pub ip: String,
    pub port: u16,
}

pub type PeersDb = Arc<Mutex<HashMap<String, Vec<PeerInfo>>>>;

fn with_peers_db(
    peers_db: PeersDb,
) -> impl Filter<Extract = (PeersDb,), Error = std::convert::Infallible> + Clone {
    warp::any().map(move || peers_db.clone())
}

fn handle_announce(announce_request: AnnounceRequest, peers_db: PeersDb) -> impl warp::Reply {
    let mut db = peers_db.lock().unwrap();
    let peers = db.entry(announce_request.info_hash.clone()).or_default();

    let peer_info = PeerInfo {
        peer_id: announce_request.peer_id.clone(),
        ip: announce_request.ip.clone(),
        port: announce_request.port,
    };

    peers.retain(|peer| peer.peer_id != announce_request.peer_id);
    peers.push(peer_info.clone());

    let response = AnnounceResponse {
        interval: 1800,
        peers: peers.clone(),
    };
    let encoded = serde_bencode::to_bytes(&response).unwrap();

    warp::reply::with_header(encoded, "Content-Type", "application/x-bencode")
}

pub fn announce_filter(
    peers_db: PeersDb,
) -> impl Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone {
    warp::path("announce")
        .and(warp::get())
        .and(warp::query::<AnnounceRequest>())
        .and(with_peers_db(peers_db))
        .map(handle_announce)
}

#[tokio::main]
pub async fn main() {
    let peers_db: PeersDb = Arc::new(Mutex::new(HashMap::new()));
    let announce = announce_filter(peers_db);

    warp::serve(announce).run(([127, 0, 0, 1], 3030)).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;
    use warp::test::request;
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };

    #[tokio::test]
    async fn test_announce() {
        let peers_db: PeersDb = Arc::new(Mutex::new(HashMap::new()));
        let filter = announce_filter(peers_db);

        let response = request()
            .method("GET")
            .path("/announce?info_hash=testhash&peer_id=testpeer&ip=127.0.0.1&port=8080")
            .reply(&filter)
            .await;

        assert_eq!(response.status(), 200);

        let announce_response: AnnounceResponse =
            serde_bencode::from_bytes(response.body()).unwrap();
        assert_eq!(announce_response.interval, 1800);
        assert_eq!(announce_response.peers.len(), 1);
        assert_eq!(announce_response.peers[0].peer_id, "testpeer");
        assert_eq!(announce_response.peers[0].ip, "127.0.0.1");
        assert_eq!(announce_response.peers[0].port, 8080);
    }

    #[tokio::test]
    async fn test_try_announce_failure_invalid_response() {
        let server = MockServer::start().await;
        let mock_invalid_response = serde_bencode::to_bytes(&String::from("some invalid content"));
        assert!(mock_invalid_response.is_ok());

        let mock_response = mock_invalid_response.unwrap();
        Mock::given(method("GET"))
            .and(path("/announce"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(mock_response, "application/x-bencode"),
            )
            .mount(&server)
            .await;

        let request = AnnounceRequest {
            info_hash: "mock_info_hash".to_string(),
            peer_id: "mock_peer_id".to_string(),
            ip: "127.0.0.1".to_string(),
            port: 8080,
        };

        let result = try_announce(&server.uri(), request).await;
        assert!(result.is_err());

        assert_eq!(
            result.unwrap_err().to_string(),
            "failed to deserialize announce response"
        );
    }

    async fn try_announce(
        base_url: &str,
        request: AnnounceRequest,
    ) -> Result<AnnounceResponse, Box<dyn std::error::Error>> {
        let url = format!(
            "{}/announce?info_hash={}&peer_id={}&ip={}&port={}",
            base_url, request.info_hash, request.peer_id, request.ip, request.port
        );
        let resp = reqwest::get(&url).await?;
        let bytes = resp.bytes().await?;
        let announce_response: AnnounceResponse = serde_bencode::de::from_bytes(&bytes)
            .context("failed to deserialize announce response")?;
        Ok(announce_response)
    }
}
