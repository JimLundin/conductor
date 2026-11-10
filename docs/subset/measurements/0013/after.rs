impl SendHalf {
    pub async fn write<T: Serialize>(&mut self, msg: &T) -> Result<(), WireError> {
        let frame = proto::encode(msg).map_err(WireError::Frame)?;
        self.write_frame(frame).await
    }
}

impl RecvHalf {
    pub async fn read<T: DeserializeOwned>(&mut self) -> Result<Option<T>, WireError> {
        let body = self.read_body().await?;
        body.map(|body| proto::decode(&body)).transpose().map_err(WireError::Frame)
    }
}

pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, FrameError> {
    let body = serde_json::to_vec(msg).map_err(FrameError::Json)?;
    frame(body)
}

pub fn decode<T: DeserializeOwned>(body: &[u8]) -> Result<T, FrameError> {
    serde_json::from_slice(body).map_err(FrameError::Json)
}

async fn hello(send: &mut SendHalf, recv: &mut RecvHalf, hello: Hello) -> Result<Option<HelloReply>, Lost> {
    send.write(&Open::Hello(hello)).await?;
    recv.read::<HelloReply>().await.map_err(Lost::Wire)
}

async fn rpc(send: &mut SendHalf, recv: &mut RecvHalf, command: Command) -> Result<Option<CommandResult>, SendError> {
    send.write(&Open::Command(command)).await?;
    recv.read::<CommandResult>().await.map_err(SendError::Wire)
}

async fn serve(send: &mut SendHalf, recv: &mut RecvHalf) -> Result<Option<Open>, StreamError> {
    send.write(&Event::default_for_test()).await?;
    recv.read::<Open>().await.map_err(StreamError::Wire)
}
