impl SendHalf {
    pub async fn write(&mut self, msg: serde_json::Value) -> Result<(), WireError> {
        let frame = proto::encode(msg).map_err(WireError::Frame)?;
        self.write_frame(frame).await
    }
}

impl RecvHalf {
    pub async fn read(&mut self) -> Result<Option<serde_json::Value>, WireError> {
        let body = self.read_body().await?;
        body.map(|body| proto::decode(&body)).transpose().map_err(WireError::Frame)
    }
}

pub fn encode(msg: serde_json::Value) -> Result<Vec<u8>, FrameError> {
    let body = serde_json::to_vec(&msg).map_err(FrameError::Json)?;
    frame(body)
}

pub fn decode(body: &[u8]) -> Result<serde_json::Value, FrameError> {
    serde_json::from_slice(body).map_err(FrameError::Json)
}

async fn hello(send: &mut SendHalf, recv: &mut RecvHalf, hello: Hello) -> Result<Option<HelloReply>, Lost> {
    send.write(serde_json::to_value(Open::Hello(hello)).map_err(Lost::Json)?).await?;
    match recv.read().await.map_err(Lost::Wire)? {
        Some(value) => serde_json::from_value::<HelloReply>(value).map(Some).map_err(Lost::Json),
        None => Ok(None),
    }
}

async fn rpc(send: &mut SendHalf, recv: &mut RecvHalf, command: Command) -> Result<Option<CommandResult>, SendError> {
    send.write(serde_json::to_value(Open::Command(command)).map_err(SendError::Json)?).await?;
    match recv.read().await.map_err(SendError::Wire)? {
        Some(value) => serde_json::from_value::<CommandResult>(value).map(Some).map_err(SendError::Json),
        None => Ok(None),
    }
}

async fn serve(send: &mut SendHalf, recv: &mut RecvHalf) -> Result<Option<Open>, StreamError> {
    send.write(serde_json::to_value(Event::default_for_test()).map_err(StreamError::Json)?).await?;
    match recv.read().await.map_err(StreamError::Wire)? {
        Some(value) => serde_json::from_value::<Open>(value).map(Some).map_err(StreamError::Json),
        None => Ok(None),
    }
}
