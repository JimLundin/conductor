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
                state: match request.eq(id).then_some(resolution) {
                    Some(resolution) => RequestState::Resolved(resolution.clone()),
                    None => state,
                },
                request,
                tool,
                detail,
            },
            (Item::Tool { call, tool, state }, Change::Finish(id, outcome)) => Item::Tool {
                state: match call.eq(id).then_some(outcome).copied() {
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
