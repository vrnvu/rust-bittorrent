use crate::http::{AnnounceRequest, AnnounceResponse};

pub async fn try_announce(_: AnnounceRequest) -> anyhow::Result<AnnounceResponse> {
    unimplemented!("announce tracker for udp is not yet implemented")
}
