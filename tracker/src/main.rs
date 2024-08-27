use anyhow::Context;
use log::{debug, info};
use models::{AnnounceResponseRaw, RegisterRequest};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env::{self, args};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, RwLock};
use warp::Filter;

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

impl InfoHashRequest {
    pub fn new(info_hash: &str) -> Self {
        InfoHashRequest {
            info_hash: info_hash.to_string(),
            peer_id: None,
            ip: None,
            port: None,
            uploaded: None,
            downloaded: None,
            left: None,
            compact: None,
        }
    }
}

type FileName = String;
type InfoHash = String;

#[derive(Debug, Default, Clone)]
pub struct FileListing {
    inner: Arc<RwLock<HashMap<FileName, InfoHash>>>,
}

impl FileListing {
    fn new() -> Self {
        FileListing::default()
    }

    fn add(&self, file_name: &FileName, info_hash: &InfoHash) -> anyhow::Result<()> {
        let mut inner = self.inner.write().unwrap();
        if inner.contains_key(file_name) {
            return Err(anyhow::anyhow!(format!(
                "file already exists: {}",
                file_name
            )));
        }
        inner.insert(file_name.clone(), info_hash.clone());
        Ok(())
    }

    fn all(&self) -> anyhow::Result<Vec<FileName>> {
        let inner = self
            .inner
            .read()
            .map_err(|_| anyhow::anyhow!("failed to lock file listing"))?;

        Ok(inner.keys().cloned().collect())
    }
}

#[derive(Debug, Default, Clone)]
pub struct PeersDb {
    inner: Arc<RwLock<HashMap<InfoHash, Vec<SocketAddr>>>>,
}

impl PeersDb {
    pub fn new() -> Self {
        PeersDb::default()
    }

    fn read(
        &self,
    ) -> anyhow::Result<std::sync::RwLockReadGuard<HashMap<InfoHash, Vec<SocketAddr>>>> {
        self.inner
            .read()
            .map_err(|_| anyhow::anyhow!("failed to lock peers db"))
    }

    fn write(
        &self,
    ) -> anyhow::Result<std::sync::RwLockWriteGuard<HashMap<InfoHash, Vec<SocketAddr>>>> {
        self.inner
            .write()
            .map_err(|_| anyhow::anyhow!("failed to lock peers db"))
    }
}

pub fn warp_handlers(
    peers_db: &PeersDb,
    file_listing: &FileListing,
) -> impl Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone {
    let get_announce = warp::path("announce")
        .and(warp::get())
        .and(with_peers_db(peers_db.clone()))
        .and(warp::query::<InfoHashRequest>())
        .map(handle_announce_get);

    let post_announce = warp::path("announce")
        .and(warp::post())
        .and(with_peers_db(peers_db.clone()))
        .and(with_file_listing(file_listing.clone()))
        .and(warp::body::bytes())
        .map(handle_announce_post);

    let get_file_listing = warp::path("files")
        .and(warp::get())
        .and(with_file_listing(file_listing.clone()))
        .map(handle_file_listing_get);

    let log = warp::log("tracker");
    get_announce
        .or(post_announce)
        .or(get_file_listing)
        .with(log)
}

fn handle_file_listing_get(file_listing: FileListing) -> impl warp::Reply {
    let files = match file_listing.all() {
        Ok(files) => files,
        Err(e) => {
            return warp::http::Response::builder()
                .status(warp::http::StatusCode::INTERNAL_SERVER_ERROR)
                .body(e.to_string());
        }
    };

    // TODO json or bencode
    warp::http::Response::builder()
        .status(warp::http::StatusCode::OK)
        .body(serde_json::to_string(&files).unwrap())
}

fn handle_announce_get(peers_db: PeersDb, info_hash_request: InfoHashRequest) -> impl warp::Reply {
    let db = match peers_db.read() {
        Ok(db) => db,
        Err(e) => {
            return warp::http::Response::builder()
                .status(warp::http::StatusCode::INTERNAL_SERVER_ERROR)
                .body(e.to_string());
        }
    };

    let response = match db.get(&info_hash_request.info_hash) {
        Some(peers) => AnnounceResponseRaw::from_socket_addrs(1800, peers),
        None => {
            return warp::http::Response::builder()
                .status(warp::http::StatusCode::NOT_FOUND)
                .body(format!(
                    "No peers found for the given info hash: {}",
                    info_hash_request.info_hash
                ))
        }
    };
    debug!("response: {:?}", response);

    let encoded = match serde_bencode::to_string(&response) {
        Ok(encoded) => encoded,
        Err(e) => {
            return warp::http::Response::builder()
                .status(warp::http::StatusCode::INTERNAL_SERVER_ERROR)
                .body(e.to_string());
        }
    };

    warp::http::Response::builder()
        .status(warp::http::StatusCode::OK)
        .header("Content-Type", "application/x-bencode")
        .body(encoded)
}

fn handle_announce_post(
    peers_db: PeersDb,
    file_listing: FileListing,
    body: bytes::Bytes,
) -> impl warp::Reply {
    let announce_register: RegisterRequest = match serde_bencode::from_bytes(&body) {
        Ok(request) => request,
        Err(e) => {
            return warp::http::Response::builder()
                .status(warp::http::StatusCode::BAD_REQUEST)
                .body(format!("Invalid bencode data: {}", e));
        }
    };
    info!("Received announce post request: {:?}", announce_register);

    match file_listing.add(&announce_register.name, &announce_register.info_hash) {
        Ok(_) => debug!("file added to listing: {:?}", announce_register.name),
        Err(e) => {
            return warp::http::Response::builder()
                .status(warp::http::StatusCode::BAD_REQUEST)
                .body(e.to_string());
        }
    }

    let mut db = match peers_db.write() {
        Ok(db) => db,
        Err(e) => {
            return warp::http::Response::builder()
                .status(warp::http::StatusCode::INTERNAL_SERVER_ERROR)
                .body(e.to_string());
        }
    };

    let peer_socket_addr = SocketAddr::new(
        IpAddr::V4(announce_register.ip.parse().unwrap()),
        announce_register.port,
    );

    db.entry(announce_register.info_hash.clone())
        .or_default()
        .push(peer_socket_addr);
    debug!("peer added to db: {:?}", announce_register);

    warp::http::Response::builder()
        .status(warp::http::StatusCode::NO_CONTENT)
        .body("".to_string())
}

fn with_peers_db(
    peers_db: PeersDb,
) -> impl Filter<Extract = (PeersDb,), Error = std::convert::Infallible> + Clone {
    warp::any().map(move || peers_db.clone())
}

fn with_file_listing(
    file_listing: FileListing,
) -> impl Filter<Extract = (FileListing,), Error = std::convert::Infallible> + Clone {
    warp::any().map(move || file_listing.clone())
}

#[tokio::main]
pub async fn main() -> anyhow::Result<()> {
    if args().len() != 2 {
        return Err(anyhow::anyhow!("Usage: tracker <port>"));
    }
    let port = args()
        .nth(1)
        .unwrap()
        .parse::<u16>()
        .context("Invalid port")?;

    env::set_var("RUST_LOG", "debug");
    env_logger::init();

    let peers_db = PeersDb::new();
    let handlers = warp_handlers(&peers_db, &FileListing::new());
    warp::serve(handlers).run(([127, 0, 0, 1], port)).await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use models::{AnnounceResponse, RegisterRequest};
    use warp::test::request;

    struct MockRequest {
        peers_db: PeersDb,
        file_listing: FileListing,
    }

    impl MockRequest {
        fn new() -> Self {
            let peers_db = PeersDb::new();
            let file_listing = FileListing::new();
            MockRequest {
                peers_db,
                file_listing,
            }
        }

        fn filter(
            &self,
        ) -> impl Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone {
            warp_handlers(&self.peers_db, &self.file_listing)
        }

        async fn post_announce(
            &self,
            announce_register: &RegisterRequest,
        ) -> warp::http::Response<bytes::Bytes> {
            let filter = self.filter();
            let body = serde_bencode::to_bytes(&announce_register).unwrap();
            request()
                .method("POST")
                .path("/announce")
                .body(body)
                .reply(&filter)
                .await
        }

        async fn get_announce(
            &self,
            info_hash_request: &InfoHashRequest,
        ) -> warp::http::Response<bytes::Bytes> {
            let filter = self.filter();
            let body = serde_bencode::to_bytes(&info_hash_request).unwrap();
            // TODO warp request() is not taking well the full query in the body
            request()
                .method("GET")
                .path(&format!(
                    "/announce?info_hash={}",
                    info_hash_request.info_hash
                ))
                .body(body)
                .reply(&filter)
                .await
        }
    }

    #[tokio::test]
    async fn test_announce_get_no_peers() -> anyhow::Result<()> {
        let mock_request = MockRequest::new();
        let info_hash_request = InfoHashRequest::new("testhash");
        let response = mock_request.get_announce(&info_hash_request).await;
        assert_eq!(response.status(), warp::http::StatusCode::NOT_FOUND);
        assert_eq!(
            response.body(),
            "No peers found for the given info hash: testhash"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_announce_get() -> anyhow::Result<()> {
        let mock_request = MockRequest::new();

        let peer_1 = RegisterRequest::new("test1", "info_hash_1", "peer1", "127.0.0.1", 8080);
        let response = mock_request.post_announce(&peer_1).await;
        assert_eq!(response.status(), warp::http::StatusCode::NO_CONTENT);

        let info_hash_request = InfoHashRequest::new("info_hash_1");
        let response_2 = mock_request.get_announce(&info_hash_request).await;
        assert_eq!(response_2.status(), warp::http::StatusCode::OK);

        let announce_response_raw: AnnounceResponseRaw =
            serde_bencode::from_bytes(response_2.body()).unwrap();

        let announce_response: AnnounceResponse = announce_response_raw.into();
        assert_eq!(announce_response.interval, 1800);
        assert_eq!(announce_response.peers.len(), 1);
        assert_eq!(announce_response.peers[0].ip().to_string(), "127.0.0.1");
        assert_eq!(announce_response.peers[0].port(), 8080);
        Ok(())
    }

    #[tokio::test]
    async fn test_announce_post() -> anyhow::Result<()> {
        let mock_request = MockRequest::new();
        let announce_register =
            RegisterRequest::new("test", "info_hash_1", "peer1", "127.0.0.1", 8080);
        let response = mock_request.post_announce(&announce_register).await;
        assert_eq!(response.status(), warp::http::StatusCode::NO_CONTENT);

        let db = mock_request.peers_db.read()?;
        let peers = db.get("info_hash_1").unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].ip().to_string(), "127.0.0.1");
        assert_eq!(peers[0].port(), 8080);
        Ok(())
    }

    #[tokio::test]
    async fn test_multiple_peers_announce() -> anyhow::Result<()> {
        let mock_request = MockRequest::new();

        let peer_1 = RegisterRequest::new("test1", "info_hash_1", "peer1", "192.168.1.2", 6881);
        let peer_2 = RegisterRequest::new("test2", "info_hash_1", "peer2", "192.168.1.3", 6882);
        let peer_3 = RegisterRequest::new("test3", "info_hash_2", "peer3", "192.168.1.4", 6883);

        // Announce for peer 1
        let response_1 = mock_request.post_announce(&peer_1).await;
        assert_eq!(response_1.status(), warp::http::StatusCode::NO_CONTENT);

        // Announce for peer 2
        let response_2 = mock_request.post_announce(&peer_2).await;
        assert_eq!(response_2.status(), warp::http::StatusCode::NO_CONTENT);

        // Announce for peer 3
        let response_3 = mock_request.post_announce(&peer_3).await;
        assert_eq!(response_3.status(), warp::http::StatusCode::NO_CONTENT);

        // Check the state of peers_db
        let db = mock_request.peers_db.read()?;

        // Check peers for info_hash_1
        let peers_info_hash_1 = db.get("info_hash_1").unwrap();
        assert_eq!(peers_info_hash_1.len(), 2);
        assert!(peers_info_hash_1
            .iter()
            .any(|peer| peer.ip().to_string() == "192.168.1.2" && peer.port() == 6881));
        assert!(peers_info_hash_1
            .iter()
            .any(|peer| peer.ip().to_string() == "192.168.1.3" && peer.port() == 6882));

        // Check peers for info_hash_2
        let peers_info_hash_2 = db.get("info_hash_2").unwrap();
        assert_eq!(peers_info_hash_2.len(), 1);
        assert!(peers_info_hash_2
            .iter()
            .any(|peer| peer.ip().to_string() == "192.168.1.4" && peer.port() == 6883));
        Ok(())
    }

    #[tokio::test]
    async fn test_announce_get_with_optional_params() -> anyhow::Result<()> {
        let mock_request = MockRequest::new();
        let peer_1 = RegisterRequest::new("test1", "testhash", "peer1", "127.0.0.1", 8080);
        let response_1 = mock_request.post_announce(&peer_1).await;
        assert_eq!(response_1.status(), warp::http::StatusCode::NO_CONTENT);

        let info_hash_request = InfoHashRequest::new("testhash");
        let response = mock_request.get_announce(&info_hash_request).await;
        assert_eq!(response.status(), warp::http::StatusCode::OK);

        let announce_response_raw: AnnounceResponseRaw =
            serde_bencode::from_bytes(response.body()).unwrap();

        let announce_response: AnnounceResponse = announce_response_raw.into();
        assert_eq!(announce_response.interval, 1800);
        assert_eq!(announce_response.peers.len(), 1);
        assert_eq!(announce_response.peers[0].ip().to_string(), "127.0.0.1");
        assert_eq!(announce_response.peers[0].port(), 8080);
        Ok(())
    }

    #[tokio::test]
    async fn test_empty_file_listing() -> anyhow::Result<()> {
        let mock_request = MockRequest::new();
        let response = request()
            .method("GET")
            .path("/files")
            .reply(&mock_request.filter())
            .await;
        assert_eq!(response.status(), warp::http::StatusCode::OK);
        assert_eq!(response.body(), "[]");
        Ok(())
    }

    #[tokio::test]
    async fn test_post_and_get_file_listing() -> anyhow::Result<()> {
        let mock_request = MockRequest::new();

        let peer_1 = RegisterRequest::new("test1", "info_hash_1", "peer1", "192.168.1.2", 6881);
        let response_1 = mock_request.post_announce(&peer_1).await;
        assert_eq!(response_1.status(), warp::http::StatusCode::NO_CONTENT);

        let response_2 = request()
            .method("GET")
            .path("/files")
            .reply(&mock_request.filter())
            .await;

        assert_eq!(response_2.status(), warp::http::StatusCode::OK);
        assert_eq!(response_2.body(), "[\"test1\"]");

        Ok(())
    }

    #[tokio::test]
    async fn test_post_and_get_file_listing_conflict_file_name() -> anyhow::Result<()> {
        let mock_request = MockRequest::new();

        let peer_1 = RegisterRequest::new("test", "info_hash_1", "peer1", "192.168.1.2", 6881);
        let response_1 = mock_request.post_announce(&peer_1).await;
        assert_eq!(response_1.status(), warp::http::StatusCode::NO_CONTENT);

        let peer_2 = RegisterRequest::new("test", "info_hash_2", "peer2", "192.168.1.3", 6882);
        let response_2 = mock_request.post_announce(&peer_2).await;
        assert_eq!(response_2.status(), warp::http::StatusCode::BAD_REQUEST);
        assert_eq!(
            response_2.body(),
            format!("file already exists: {}", peer_2.name).as_bytes()
        );

        Ok(())
    }
}
