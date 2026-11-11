//! Code volume for proposals: token leaves and code lines, both blind to
//! comments and doc comments.

use std::collections::BTreeSet;

use proc_macro2::{Delimiter, TokenStream, TokenTree};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Volume {
    pub tokens: usize,
    pub lines: usize,
}

/// One token `measure` counts.
pub struct Token {
    /// The byte offset of the token, or of a delimited group's opening
    /// delimiter.
    pub start: usize,
    /// The lines the token counts on: those a leaf spans, or a group's
    /// opening and closing lines.
    pub lines: Vec<usize>,
}

/// Idents, puncts and literals count one each, and so does each delimited
/// group. A line counts when a counted token starts, ends or spans it.
pub fn measure(src: &str) -> Result<Volume, proc_macro2::LexError> {
    let tokens = tokens(src)?;
    let lines: BTreeSet<usize> = tokens
        .iter()
        .flat_map(|t| t.lines.iter().copied())
        .collect();
    Ok(Volume {
        tokens: tokens.len(),
        lines: lines.len(),
    })
}

/// Each token `measure` counts, in source order.
pub fn tokens(src: &str) -> Result<Vec<Token>, proc_macro2::LexError> {
    let stream: TokenStream = src.parse()?;
    let mut tally = Tally {
        src,
        tokens: Vec::new(),
    };
    tally.walk(stream);
    Ok(tally.tokens)
}

struct Tally<'a> {
    src: &'a str,
    tokens: Vec<Token>,
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
                        let open = group.span_open().start().line;
                        let close = group.span_close().end().line;
                        self.tokens.push(Token {
                            start: group.span_open().byte_range().start,
                            lines: match open == close {
                                true => vec![open],
                                false => vec![open, close],
                            },
                        });
                    }
                    self.walk(group.stream());
                }
                leaf => {
                    let span = leaf.span();
                    self.tokens.push(Token {
                        start: span.byte_range().start,
                        lines: (span.start().line..=span.end().line).collect(),
                    });
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
