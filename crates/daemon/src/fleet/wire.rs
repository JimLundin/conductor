//! Frame I/O between hosts, in the fleet protocol's own framing
//! ([`host::fleet::frame`]).

use anyhow::Context;
use host::fleet::frame::{self, Frame};
use iroh::endpoint::{RecvStream, SendStream};

/// The largest fleet frame a host reads, well under the protocol's limit.
const MAX_FRAME: usize = 256 * 1024;

pub(super) async fn write<T: Frame + Sync>(
    send: &mut SendStream,
    msg: &T,
) -> anyhow::Result<()> {
    Ok(send.write_all(&frame::encode(msg)?).await?)
}

/// Reads one frame, which the peer must send before finishing the stream.
pub(super) async fn read<T: Frame>(recv: &mut RecvStream) -> anyhow::Result<T> {
    let header = net::wire::read_header::<{ frame::HEADER_LEN }>(recv)
        .await?
        .context("fleet stream closed before its reply")?;
    let body =
        net::wire::read_body(recv, frame::body_len(header)?, MAX_FRAME).await?;
    Ok(frame::decode(&body)?)
}
