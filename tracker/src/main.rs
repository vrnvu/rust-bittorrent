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

#[tokio::main]
pub async fn main() {
    let peers_db: PeersDb = Arc::new(Mutex::new(HashMap::new()));

    let peers_db_filter = warp::any().map(move || peers_db.clone());

    let announce = warp::path("announce")
        .and(warp::get())
        .and(warp::query::<AnnounceRequest>())
        .and(peers_db_filter.clone())
        .map(|announce_request: AnnounceRequest, peers_db: PeersDb| {
            let mut db = peers_db.lock().unwrap();

            let peers = db
                .entry(announce_request.info_hash.clone())
                .or_insert_with(Vec::new);

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

            warp::reply::json(&response)
        });

    warp::serve(announce).run(([127, 0, 0, 1], 3030)).await;
}
