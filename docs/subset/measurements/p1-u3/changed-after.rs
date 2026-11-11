impl Item {
    fn changed(self, change: &Change) -> Item {
        match (self, change) {
            (Item::Request { request, tool, detail, .. }, Change::Resolve(id, resolution))
                if &request == id =>
            {
                Item::Request {
                    request,
                    tool,
                    detail,
                    state: RequestState::Resolved(resolution.clone()),
                }
            }
            (Item::Tool { call, tool, .. }, Change::Finish(id, outcome)) if &call == id => {
                Item::Tool {
                    call,
                    tool,
                    state: ToolState::Finished(outcome.to_owned()),
                }
            }
            (item, _) => item,
        }
    }
}
