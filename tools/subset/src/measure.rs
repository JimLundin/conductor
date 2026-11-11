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
    let tally = Tally::of(src)?;
    Ok(Volume {
        tokens: tally.starts.len(),
        lines: tally.lines.len(),
    })
}

/// The byte offset of each token `measure` counts, in source order. A
/// delimited group's offset is that of its opening delimiter.
pub fn token_starts(src: &str) -> Result<Vec<usize>, proc_macro2::LexError> {
    Ok(Tally::of(src)?.starts)
}

struct Tally<'a> {
    src: &'a str,
    starts: Vec<usize>,
    lines: BTreeSet<usize>,
}

impl<'a> Tally<'a> {
    fn of(src: &'a str) -> Result<Tally<'a>, proc_macro2::LexError> {
        let stream: TokenStream = src.parse()?;
        let mut tally = Tally {
            src,
            starts: Vec::new(),
            lines: BTreeSet::new(),
        };
        tally.walk(stream);
        Ok(tally)
    }

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
                        self.starts.push(group.span_open().byte_range().start);
                        self.lines.insert(group.span_open().start().line);
                        self.lines.insert(group.span_close().end().line);
                    }
                    self.walk(group.stream());
                }
                leaf => {
                    let span = leaf.span();
                    self.starts.push(span.byte_range().start);
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
