use models::AnnounceResponse;

use crate::{http::AnnounceRequest, torrent::PeerId};

pub async fn try_announce(_: AnnounceRequest, _: &PeerId) -> anyhow::Result<AnnounceResponse> {
    unimplemented!("announce tracker for udp is not yet implemented")
}
