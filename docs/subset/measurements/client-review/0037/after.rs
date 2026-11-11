impl Session {
    pub(crate) async fn closed(&self) {
        let mut peer = self.inner.peer.clone();
        let mut liveness = self.inner.liveness.state.clone();
        tokio::select! {
            _ = peer.wait_for(|peer| peer == &Peer::Lost) => {}
            _ = liveness.wait_for(|state| state == &ChannelState::Closed) => {}
        }
    }
}
fn f() {
    let x = lost.wait_for(|peer| peer == &Peer::Lost);
    let y = state.wait_for(|state| state != &ChannelState::Connecting);
    let nominated = stats.iter().rfind(|stat| matches!(text(stat, "type").as_deref(), Some("x")));
}
