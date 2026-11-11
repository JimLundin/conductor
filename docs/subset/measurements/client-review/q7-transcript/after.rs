impl Transcript {
    fn append(self, text: String) -> Transcript {
        let mut items = self.items;
        let (kept, text) = match items.pop() {
            Some(Item::Reply {
                text: open,
                state: ReplyState::Streaming,
            }) => (None, format!("{open}{text}")),
            last => (last, text),
        };
        items.extend(kept);
        items.push(Item::Reply {
            text,
            state: ReplyState::Streaming,
        });
        Transcript { items }
    }
}
