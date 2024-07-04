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

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct InfoHashRequest {
    pub info_hash: String,
    pub peer_id: Option<String>,
    pub ip: Option<String>,
    pub port: Option<u16>,
    pub uploaded: Option<u64>,
    pub downloaded: Option<u64>,
    pub left: Option<u64>,
    pub compact: Option<u8>,
}

pub type PeersDb = Arc<Mutex<HashMap<String, Vec<PeerInfo>>>>;

fn with_peers_db(
    peers_db: PeersDb,
) -> impl Filter<Extract = (PeersDb,), Error = std::convert::Infallible> + Clone {
    warp::any().map(move || peers_db.clone())
}

fn handle_announce_get(info_hash_request: InfoHashRequest, peers_db: PeersDb) -> impl warp::Reply {
    let db = peers_db.lock().unwrap();
    let peers = db
        .get(&info_hash_request.info_hash)
        .cloned()
        .unwrap_or_default();

    let response = AnnounceResponse {
        interval: 1800,
        peers,
    };

    let encoded = serde_bencode::to_bytes(&response).unwrap();

    warp::http::Response::builder()
        .status(warp::http::StatusCode::OK)
        .header("Content-Type", "application/x-bencode")
        .body(encoded)
}

fn handle_announce_post(announce_request: AnnounceRequest, peers_db: PeersDb) -> impl warp::Reply {
    let mut db = peers_db.lock().unwrap();
    let peers = db.entry(announce_request.info_hash.clone()).or_default();

    let peer_info = PeerInfo {
        peer_id: announce_request.peer_id.clone(),
        ip: announce_request.ip.clone(),
        port: announce_request.port,
    };

    peers.retain(|peer| peer.peer_id != announce_request.peer_id);
    peers.push(peer_info);

    warp::http::Response::builder()
        .status(warp::http::StatusCode::NO_CONTENT)
        .body("")
}

pub fn announce_filter(
    peers_db: PeersDb,
) -> impl Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone {
    let get = warp::path("announce")
        .and(warp::get())
        .and(warp::query::<InfoHashRequest>())
        .and(with_peers_db(peers_db.clone()))
        .map(handle_announce_get);

    let post = warp::path("announce")
        .and(warp::post())
        .and(warp::body::json::<AnnounceRequest>())
        .and(with_peers_db(peers_db))
        .map(handle_announce_post);

    get.or(post)
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
    use warp::test::request;

    #[tokio::test]
    async fn test_announce_get() {
        let peers_db: PeersDb = Arc::new(Mutex::new(HashMap::new()));
        let filter = announce_filter(peers_db.clone());

        // Add a peer
        {
            let mut db = peers_db.lock().unwrap();
            db.entry("testhash".to_string())
                .or_default()
                .push(PeerInfo {
                    peer_id: "testpeer".to_string(),
                    ip: "127.0.0.1".to_string(),
                    port: 8080,
                });
        }

        let response = request()
            .method("GET")
            .path("/announce?info_hash=testhash")
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
    async fn test_announce_post() {
        let peers_db: PeersDb = Arc::new(Mutex::new(HashMap::new()));
        let filter = announce_filter(peers_db.clone());

        let announce_request = AnnounceRequest {
            info_hash: "info_hash_1".to_string(),
            peer_id: "peer1".to_string(),
            ip: "127.0.0.1".to_string(),
            port: 8080,
        };

        let response = request()
            .method("POST")
            .path("/announce")
            .json(&announce_request)
            .reply(&filter)
            .await;
        assert_eq!(response.status(), warp::http::StatusCode::NO_CONTENT);

        let db = peers_db.lock().unwrap();
        let peers = db.get("info_hash_1").unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].peer_id, "peer1");
        assert_eq!(peers[0].ip, "127.0.0.1");
        assert_eq!(peers[0].port, 8080);
    }

    #[tokio::test]
    async fn test_multiple_peers_announce() {
        let peers_db: PeersDb = Arc::new(Mutex::new(HashMap::new()));
        let filter = announce_filter(peers_db.clone());

        let peer_1 = AnnounceRequest {
            info_hash: "info_hash_1".to_string(),
            peer_id: "peer1".to_string(),
            ip: "192.168.1.2".to_string(),
            port: 6881,
        };

        let peer_2 = AnnounceRequest {
            info_hash: "info_hash_1".to_string(),
            peer_id: "peer2".to_string(),
            ip: "192.168.1.3".to_string(),
            port: 6882,
        };

        let peer_3 = AnnounceRequest {
            info_hash: "info_hash_2".to_string(),
            peer_id: "peer3".to_string(),
            ip: "192.168.1.4".to_string(),
            port: 6883,
        };

        // Announce for peer 1
        let response_1 = request()
            .method("POST")
            .path("/announce")
            .json(&peer_1)
            .reply(&filter)
            .await;
        assert_eq!(response_1.status(), warp::http::StatusCode::NO_CONTENT);

        // Announce for peer 2
        let response_2 = request()
            .method("POST")
            .path("/announce")
            .json(&peer_2)
            .reply(&filter)
            .await;
        assert_eq!(response_2.status(), warp::http::StatusCode::NO_CONTENT);

        // Announce for peer 3
        let response_3 = request()
            .method("POST")
            .path("/announce")
            .json(&peer_3)
            .reply(&filter)
            .await;
        assert_eq!(response_3.status(), warp::http::StatusCode::NO_CONTENT);

        // Check the state of peers_db
        let db = peers_db.lock().unwrap();

        // Check peers for info_hash_1
        let peers_info_hash_1 = db.get("info_hash_1").unwrap();
        assert_eq!(peers_info_hash_1.len(), 2);
        assert!(peers_info_hash_1
            .iter()
            .any(|peer| peer.peer_id == "peer1" && peer.ip == "192.168.1.2" && peer.port == 6881));
        assert!(peers_info_hash_1
            .iter()
            .any(|peer| peer.peer_id == "peer2" && peer.ip == "192.168.1.3" && peer.port == 6882));

        // Check peers for info_hash_2
        let peers_info_hash_2 = db.get("info_hash_2").unwrap();
        assert_eq!(peers_info_hash_2.len(), 1);
        assert!(peers_info_hash_2
            .iter()
            .any(|peer| peer.peer_id == "peer3" && peer.ip == "192.168.1.4" && peer.port == 6883));
    }

    #[tokio::test]
    async fn test_announce_get_with_optional_params() {
        let peers_db: PeersDb = Arc::new(Mutex::new(HashMap::new()));
        let filter = announce_filter(peers_db.clone());

        // Add a peer
        {
            let mut db = peers_db.lock().unwrap();
            db.entry("testhash".to_string())
                .or_default()
                .push(PeerInfo {
                    peer_id: "testpeer".to_string(),
                    ip: "127.0.0.1".to_string(),
                    port: 8080,
                });
        }

        let response = request()
            .method("GET")
            .path("/announce?info_hash=testhash&peer_id=00112233445566778899&port=6881&uploaded=0&downloaded=0&left=100&compact=1")
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
}
