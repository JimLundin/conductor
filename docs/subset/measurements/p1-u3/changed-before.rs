impl Item {
    fn changed(self, change: &Change) -> Item {
        match (self, change) {
            (
                Item::Request {
                    request,
                    tool,
                    detail,
                    state,
                },
                Change::Resolve(id, resolution),
            ) => Item::Request {
                state: match Some(resolution).filter(|_| &request == id) {
                    Some(resolution) => RequestState::Resolved(resolution.clone()),
                    None => state,
                },
                request,
                tool,
                detail,
            },
            (Item::Tool { call, tool, state }, Change::Finish(id, outcome)) => Item::Tool {
                state: match Some(outcome).filter(|_| &call == id).copied() {
                    Some(outcome) => ToolState::Finished(outcome),
                    None => state,
                },
                call,
                tool,
            },
            (item, _) => item,
        }
    }
}
