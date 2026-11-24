#![cfg(test)]

use iroh::endpoint::{ConnectionError, presets};
use iroh::{Endpoint, RelayMode};
use net::wire::WireError;
use web_iroh::{Connection, PipeError};

const ALPN: &[u8] = b"pipe-test/0";

type TestResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

async fn endpoint() -> TestResult<Endpoint> {
    Ok(Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .alpns(Vec::from([ALPN.to_vec()]))
        .bind()
        .await?)
}

/// The host's endpoint, its side of the connection, and the pipe's.
async fn pair() -> TestResult<(Endpoint, iroh::endpoint::Connection, Connection)>
{
    let host = endpoint().await?;
    let client = endpoint().await?;
    let accept = async {
        let incoming = host
            .accept()
            .await
            .ok_or("the host closed before a connection")?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(incoming.await?)
    };
    let (dialled, accepted) =
        tokio::join!(Connection::dial(client, host.addr(), ALPN), accept);
    Ok((host, accepted?, dialled?))
}

/// The host's side of a stream the pipe opened with an 8-byte frame.
async fn opened(
    accepted: &iroh::endpoint::Connection,
    pipe: &web_iroh::Stream,
) -> TestResult<(iroh::endpoint::SendStream, iroh::endpoint::RecvStream)> {
    pipe.send(b"open".to_vec()).await?;
    let (send, mut recv) = accepted.accept_bi().await?;
    let mut opening = [0u8; 8];
    recv.read_exact(&mut opening).await?;
    Ok((send, recv))
}

#[tokio::test]
async fn a_sent_frame_is_its_length_big_endian_then_its_body() -> TestResult<()>
{
    let (_host, accepted, connection) = pair().await?;
    let stream = connection.open_bi().await?;
    stream.send(b"hi".to_vec()).await?;
    stream.send(Vec::new()).await?;
    stream.finish().await?;
    let (_send, mut recv) = accepted.accept_bi().await?;
    assert_eq!(
        recv.read_to_end(64).await?,
        [0, 0, 0, 2, b'h', b'i', 0, 0, 0, 0]
    );
    Ok(())
}

#[tokio::test]
async fn recv_gives_each_frame_body_then_none_at_the_end() -> TestResult<()> {
    let (_host, accepted, connection) = pair().await?;
    let stream = connection.open_bi().await?;
    let (mut send, _recv) = opened(&accepted, &stream).await?;
    send.write_all(&[0, 0, 0, 3, b'o', b'n', b'e', 0, 0, 0, 0])
        .await?;
    send.finish()?;
    assert_eq!(stream.recv().await?, Some(b"one".to_vec()));
    assert_eq!(stream.recv().await?, Some(Vec::new()));
    assert_eq!(stream.recv().await?, None);
    Ok(())
}

#[tokio::test]
async fn a_stream_ending_inside_a_frame_is_an_error() -> TestResult<()> {
    let (_host, accepted, connection) = pair().await?;
    let stream = connection.open_bi().await?;
    let (mut send, _recv) = opened(&accepted, &stream).await?;
    send.write_all(&[0, 0, 0, 5, b'h']).await?;
    send.finish()?;
    assert!(matches!(
        stream.recv().await,
        Err(PipeError::Wire(WireError::Read(_)))
    ));
    Ok(())
}

#[tokio::test]
async fn closing_the_pipe_closes_the_hosts_side() -> TestResult<()> {
    let (_host, accepted, connection) = pair().await?;
    connection.close().await;
    assert!(matches!(
        accepted.closed().await,
        ConnectionError::ApplicationClosed(_)
    ));
    assert!(connection.open_bi().await.is_err());
    Ok(())
}

#[tokio::test]
async fn closed_resolves_with_the_hosts_reason() -> TestResult<()> {
    let (_host, accepted, connection) = pair().await?;
    accepted.close(7u32.into(), b"bye");
    assert!(connection.closed().await.contains("bye"));
    Ok(())
}
