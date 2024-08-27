use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RegisterRequest {
    pub name: String,
    pub info_hash: String,
    pub peer_id: String,
    pub ip: String,
    pub port: u16,
}

impl RegisterRequest {
    pub fn new(name: &str, info_hash: &str, peer_id: &str, ip: &str, port: u16) -> Self {
        RegisterRequest {
            name: name.to_string(),
            info_hash: info_hash.to_string(),
            peer_id: peer_id.to_string(),
            ip: ip.to_string(),
            port,
        }
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub struct AnnounceResponseRaw {
    pub interval: i64,
    pub peers: Vec<u8>,
}

impl AnnounceResponseRaw {
    pub fn from_socket_addrs(interval: i64, peers: &[SocketAddr]) -> Self {
        // IPv4: 4 bytes for the address, 2 bytes for the port
        let mut buf = Vec::with_capacity(peers.len() * 6);
        for peer in peers {
            if let SocketAddr::V4(addr) = peer {
                buf.extend_from_slice(&addr.ip().octets());
                buf.extend_from_slice(&addr.port().to_be_bytes());
            }
        }
        AnnounceResponseRaw {
            interval,
            peers: buf,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AnnounceResponse {
    pub interval: i64,
    pub peers: Vec<SocketAddr>,
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

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use super::*;

    #[test]
    fn test_announce_response_raw_from_socket_addrs_to_announce_response() {
        let peers = vec![
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 6881),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 0, 1)), 8080),
        ];
        let raw_response = AnnounceResponseRaw::from_socket_addrs(123, &peers);
        let announce_response: AnnounceResponse = raw_response.into();
        assert_eq!(announce_response.interval, 123);
        assert_eq!(announce_response.peers.len(), 2);
        assert_eq!(announce_response.peers[0].ip().to_string(), "127.0.0.1");
        assert_eq!(announce_response.peers[0].port(), 6881);
        assert_eq!(announce_response.peers[1].ip().to_string(), "192.168.0.1");
        assert_eq!(announce_response.peers[1].port(), 8080);
    }
}
