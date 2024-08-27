use models::AnnounceResponse;

use crate::http::AnnounceRequest;

pub async fn try_announce(_: AnnounceRequest) -> anyhow::Result<AnnounceResponse> {
    unimplemented!("announce tracker for udp is not yet implemented")
}
