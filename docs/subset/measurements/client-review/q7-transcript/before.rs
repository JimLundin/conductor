impl Transcript {
    fn append(self, text: String) -> Transcript {
        let mut items = self.items;
        match items.pop() {
            Some(Item::Reply {
                text: open,
                state: ReplyState::Streaming,
            }) => items.push(Item::Reply {
                text: format!("{open}{text}"),
                state: ReplyState::Streaming,
            }),
            Some(last) => {
                items.push(last);
                items.push(Item::Reply {
                    text,
                    state: ReplyState::Streaming,
                });
            }
            None => items.push(Item::Reply {
                text,
                state: ReplyState::Streaming,
            }),
        }
        Transcript { items }
    }
}
