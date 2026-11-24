//! Frame I/O between hosts, in the fleet protocol's own codec
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
    Ok(net::wire::write(send, &frame::encode(msg)?).await?)
}

/// Reads one frame, which the peer must send before finishing the stream.
pub(super) async fn read<T: Frame>(recv: &mut RecvStream) -> anyhow::Result<T> {
    let body = net::wire::read(recv, MAX_FRAME)
        .await?
        .context("fleet stream closed before its reply")?;
    Ok(frame::decode(&body)?)
}
