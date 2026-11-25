use anyhow::Context;
use host::fleet::frame::{self, Frame};
use iroh::endpoint::{RecvStream, SendStream};
use zeroize::Zeroizing;

const MAX_FRAME: usize = 256 * 1024;

pub(super) async fn write<T: Frame + Sync>(
    send: &mut SendStream,
    msg: &T,
) -> anyhow::Result<()> {
    let body = Zeroizing::new(frame::encode(msg)?);
    Ok(net::wire::write(send, &body).await?)
}

pub(super) async fn read<T: Frame>(recv: &mut RecvStream) -> anyhow::Result<T> {
    let body = net::wire::read(recv, MAX_FRAME)
        .await?
        .map(Zeroizing::new)
        .context("fleet stream closed before its reply")?;
    Ok(frame::decode(&body)?)
}
