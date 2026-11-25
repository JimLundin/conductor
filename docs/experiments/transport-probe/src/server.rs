use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::Path,
    sync::mpsc::{self, Receiver, Sender},
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::{Context, Result};
use openssl::ssl::{SslAcceptor, SslFiletype, SslMethod};
use transport_probe_core::{Body, Request, body_framing, chunk_length};

pub struct Server {
    pub address: SocketAddr,
    stopped: Sender<()>,
    worker: Option<JoinHandle<Result<()>>>,
}

pub fn read_request(stream: impl Read) -> Result<Option<Request>> {
    let mut reader = BufReader::new(stream);
    let mut header = String::new();
    while !header.ends_with("\r\n\r\n") {
        let count = reader.read_line(&mut header).unwrap_or_default();
        if count == 0 {
            anyhow::ensure!(
                header.is_empty(),
                "incomplete fixture HTTP header"
            );
            return Ok(None);
        }
    }
    let body = read_body(
        &mut reader,
        body_framing(&header).context("invalid HTTP framing")?,
    )?;
    Ok(Some(Request { header, body }))
}

fn read_body(reader: &mut impl BufRead, framing: Body) -> Result<Vec<u8>> {
    match framing {
        Body::Length(length) => {
            let mut body = vec![0; length];
            reader.read_exact(&mut body)?;
            Ok(body)
        }
        Body::Chunked => read_chunks(reader),
    }
}

fn read_chunks(reader: &mut impl BufRead) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let length =
            chunk_length(&line).context("invalid HTTP chunk length")?;
        if length == 0 {
            let mut terminator = String::new();
            reader.read_line(&mut terminator)?;
            anyhow::ensure!(
                terminator == "\r\n",
                "unexpected fixture HTTP trailer"
            );
            return Ok(body);
        }
        let offset = body.len();
        body.resize(offset + length, 0);
        reader.read_exact(&mut body[offset..])?;
        let mut delimiter = [0; 2];
        reader.read_exact(&mut delimiter)?;
        anyhow::ensure!(delimiter == *b"\r\n", "invalid chunk delimiter");
    }
}

fn serve_stream(
    mut stream: impl Read + Write,
    handler: &mut impl FnMut(Request) -> Result<Vec<u8>>,
) -> Result<()> {
    if let Some(request) = read_request(&mut stream)? {
        stream.write_all(&handler(request)?)?;
        stream.flush()?;
    }
    Ok(())
}

fn serve_connection(
    stream: TcpStream,
    tls: Option<&SslAcceptor>,
    handler: &mut impl FnMut(Request) -> Result<Vec<u8>>,
) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    match tls {
        Some(acceptor) => {
            if let Ok(mut encrypted) = acceptor.accept(stream) {
                serve_stream(&mut encrypted, handler)?;
                let _ = encrypted.shutdown();
            }
        }
        None => serve_stream(stream, handler)?,
    }
    Ok(())
}

fn next_connection(
    listener: &TcpListener,
    stopped: &Receiver<()>,
    tls: Option<&SslAcceptor>,
    handler: &mut impl FnMut(Request) -> Result<Vec<u8>>,
) -> Result<bool> {
    let (stream, _) = listener.accept()?;
    if stopped.try_recv().is_ok() {
        return Ok(false);
    }
    serve_connection(stream, tls, handler)?;
    Ok(true)
}

impl Server {
    pub fn start(
        tls: Option<SslAcceptor>,
        mut handler: impl FnMut(Request) -> Result<Vec<u8>> + Send + 'static,
    ) -> Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        let address = listener.local_addr()?;
        let (stopped, worker_stop) = mpsc::channel();
        let worker = thread::spawn(move || {
            while next_connection(
                &listener,
                &worker_stop,
                tls.as_ref(),
                &mut handler,
            )? {}
            Ok(())
        });
        Ok(Self {
            address,
            stopped,
            worker: Some(worker),
        })
    }

    pub fn finish(mut self) -> Result<()> {
        self.stop();
        self.worker
            .take()
            .context("server already joined")?
            .join()
            .map_err(|_| anyhow::anyhow!("fixture thread panicked"))?
    }

    fn stop(&self) {
        let _ = self.stopped.send(());
        let _ = TcpStream::connect(self.address);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn tls(root: &Path) -> Result<SslAcceptor> {
    let mut acceptor = SslAcceptor::mozilla_intermediate(SslMethod::tls())?;
    acceptor.set_private_key_file(root.join("tls.key"), SslFiletype::PEM)?;
    acceptor.set_certificate_chain_file(root.join("tls.crt"))?;
    acceptor.check_private_key()?;
    Ok(acceptor.build())
}
