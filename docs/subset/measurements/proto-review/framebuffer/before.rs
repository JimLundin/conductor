/// Reassembles frames from data channel messages, which split and join
/// frames arbitrarily.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameBuffer {
    bytes: Vec<u8>,
}

/// What [`FrameBuffer::next_body`] found at the front of the buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reassembly {
    /// A whole frame's body, and the buffer that remains after it.
    Frame { body: Vec<u8>, rest: FrameBuffer },
    /// The next frame has not fully arrived.
    Incomplete,
}

/// What is left in a [`FrameBuffer`] when its channel ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remainder {
    /// The buffer sits between frames: the channel can end here without
    /// losing data.
    Nothing,
    PartialFrame,
}

impl FrameBuffer {
    pub fn empty() -> FrameBuffer {
        FrameBuffer { bytes: Vec::new() }
    }

    pub fn push(self, message: &[u8]) -> FrameBuffer {
        let mut bytes = self.bytes;
        bytes.extend_from_slice(message);
        FrameBuffer { bytes }
    }

    /// The next complete frame's body, if all of it has arrived.
    pub fn next_body(&self) -> Result<Reassembly, FrameError> {
        match self.bytes.split_first_chunk::<HEADER_LEN>() {
            None => Ok(Reassembly::Incomplete),
            Some((header, rest)) => Ok(match rest.split_at_checked(body_len(header.to_owned())?) {
                Some((body, after)) => Reassembly::Frame {
                    body: body.to_vec(),
                    rest: FrameBuffer {
                        bytes: after.to_vec(),
                    },
                },
                None => Reassembly::Incomplete,
            }),
        }
    }

    pub fn remainder(&self) -> Remainder {
        match self.bytes.first() {
            None => Remainder::Nothing,
            Some(_) => Remainder::PartialFrame,
        }
    }
}

impl ChannelRecv {
    /// Cancel-safe: a partly received frame stays in the buffer.
    pub async fn read_body(&mut self) -> Result<Option<Vec<u8>>, WireError> {
        loop {
            if let proto::Reassembly::Frame { body, rest } = self.buffer.next_body()? {
                self.buffer = rest;
                return Ok(Some(body));
            }
            match self.messages.recv().await {
                Some(message) => {
                    let buffer = std::mem::replace(&mut self.buffer, proto::FrameBuffer::empty());
                    self.buffer = buffer.push(&message);
                }
                None if self.buffer.remainder() == proto::Remainder::Nothing => return Ok(None),
                None => {
                    return Err(WireError::Channel("closed in the middle of a frame".into()));
                }
            }
        }
    }
}

impl ChannelRecv {
    /// Cancel-safe: a partly received frame stays in the buffer.
    pub async fn read_body(&mut self) -> Result<Option<Vec<u8>>, crate::wire::WireError> {
        loop {
            if let proto::Reassembly::Frame { body, rest } = self.buffer.next_body()? {
                self.buffer = rest;
                return Ok(Some(body));
            }
            match self.messages.recv().await {
                Some(message) => {
                    let buffer = std::mem::replace(&mut self.buffer, proto::FrameBuffer::empty());
                    self.buffer = buffer.push(&message);
                }
                None if self.buffer.remainder() == proto::Remainder::Nothing => return Ok(None),
                None => return Err(ChannelError::Truncated.into()),
            }
        }
    }
}
