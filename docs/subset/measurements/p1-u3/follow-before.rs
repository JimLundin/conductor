async fn run_link(
    ticket: Ticket,
    status: Signal<Option<Status>>,
    view: Signal<HostView>,
    mut link: Signal<Option<Rc<HostLink>>>,
) {
    let options = LinkOptions::new(ticket::client_name());
    match HostLink::spawn(Target::of_ticket(&ticket), options).await {
        Ok((handle, updates)) => {
            link.set(Some(Rc::new(handle)));
            let Err(err) = follow(updates, status, view).await;
            tracing::warn!(%err, "the link ended");
        }
        Err(err) => tracing::error!(%err, "could not start iroh"),
    }
}

async fn follow(
    mut updates: mpsc::Receiver<Update>,
    mut status: Signal<Option<Status>>,
    mut view: Signal<HostView>,
) -> Result<Infallible, SendError> {
    loop {
        match updates.recv().await {
            Some(Update::Status(next)) => {
                tracing::info!(state = ?next.state, path = ?next.path, error = ?next.error, "link status");
                status.set(Some(next));
            }
            Some(Update::Reset(_)) => view.set(HostView::empty()),
            Some(Update::Event(event)) => {
                let folded = view.replace(HostView::empty()).apply(event);
                view.set(folded);
            }
            None => break Err(SendError::Stopped),
        }
    }
}
