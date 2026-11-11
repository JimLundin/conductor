/// Reassembles frames from data channel messages, which split and join
/// frames arbitrarily.
#[derive(Debug)]
pub struct FrameBuffer {
    bytes: Vec<u8>,
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

    pub fn push(&mut self, message: &[u8]) {
        self.bytes.extend_from_slice(message);
    }

    /// The next complete frame's body, if all of it has arrived.
    pub fn next_body(&mut self) -> Result<Option<Vec<u8>>, FrameError> {
        match self.bytes.split_first_chunk::<HEADER_LEN>() {
            None => Ok(None),
            Some((header, rest)) => match rest.split_at_checked(body_len(header.to_owned())?) {
                Some((body, after)) => {
                    let body = body.to_vec();
                    self.bytes = after.to_vec();
                    Ok(Some(body))
                }
                None => Ok(None),
            },
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
            if let Some(body) = self.buffer.next_body()? {
                return Ok(Some(body));
            }
            match self.messages.recv().await {
                Some(message) => self.buffer.push(&message),
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
            if let Some(body) = self.buffer.next_body()? {
                return Ok(Some(body));
            }
            match self.messages.recv().await {
                Some(message) => self.buffer.push(&message),
                None if self.buffer.remainder() == proto::Remainder::Nothing => return Ok(None),
                None => return Err(ChannelError::Truncated.into()),
            }
        }
    }
}
