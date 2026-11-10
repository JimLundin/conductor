//! Code volume for proposals: token leaves and code lines, both blind to
//! comments and doc comments.

use std::collections::BTreeSet;

use proc_macro2::{Delimiter, TokenStream, TokenTree};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Volume {
    pub tokens: usize,
    pub lines: usize,
}

/// Idents, puncts and literals count one each, and so does each delimited
/// group. A line counts when a counted token starts, ends or spans it.
pub fn measure(src: &str) -> Result<Volume, proc_macro2::LexError> {
    let stream: TokenStream = src.parse()?;
    let mut tally = Tally {
        src,
        tokens: 0,
        lines: BTreeSet::new(),
    };
    tally.walk(stream);
    Ok(Volume {
        tokens: tally.tokens,
        lines: tally.lines.len(),
    })
}

struct Tally<'a> {
    src: &'a str,
    tokens: usize,
    lines: BTreeSet<usize>,
}

impl Tally<'_> {
    fn walk(&mut self, stream: TokenStream) {
        let mut trees = stream.into_iter().peekable();
        while let Some(tree) = trees.next() {
            if self.is_doc_comment(&tree) {
                // A doc comment lexes as `#`, an optional `!`, and a bracket group.
                if matches!(trees.peek(), Some(TokenTree::Punct(p)) if p.as_char() == '!') {
                    trees.next();
                }
                trees.next();
                continue;
            }
            match tree {
                TokenTree::Group(group) => {
                    if group.delimiter() != Delimiter::None {
                        self.tokens += 1;
                        self.lines.insert(group.span_open().start().line);
                        self.lines.insert(group.span_close().end().line);
                    }
                    self.walk(group.stream());
                }
                leaf => {
                    self.tokens += 1;
                    let span = leaf.span();
                    self.lines.extend(span.start().line..=span.end().line);
                }
            }
        }
    }

    fn is_doc_comment(&self, tree: &TokenTree) -> bool {
        let TokenTree::Punct(punct) = tree else {
            return false;
        };
        let rest = &self.src[punct.span().byte_range().start..];
        punct.as_char() == '#' && (rest.starts_with("//") || rest.starts_with("/*"))
    }
}
