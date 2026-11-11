impl Session {
    pub(crate) async fn closed(&self) {
        let mut peer = self.inner.peer.clone();
        let mut liveness = self.inner.liveness.state.clone();
        tokio::select! {
            _ = peer.wait_for(|peer| Peer::Lost.eq(peer)) => {}
            _ = liveness.wait_for(|state| ChannelState::Closed.eq(state)) => {}
        }
    }
}
fn f() {
    let x = lost.wait_for(|peer| Peer::Lost.eq(peer));
    let y = state.wait_for(|state| ChannelState::Connecting.ne(state));
    let nominated = stats.iter().rev().find(|stat| matches!(text(stat, "type").as_deref(), Some("x")));
}
