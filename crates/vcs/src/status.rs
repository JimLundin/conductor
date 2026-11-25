use alloc::string::String;
use core::fmt;

/// Where a checkout stands, from what
/// `git status --porcelain=v2 --branch` printed in it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Checkout {
    /// The commit it is at; none before its first.
    pub commit: Option<String>,
    /// The branch it is on; none when detached.
    pub branch: Option<String>,
    pub upstream: Option<Upstream>,
    /// How many of its files changed or are untracked.
    pub changed: usize,
}

/// The branch a checkout's branch tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    /// As `<remote>/<branch>`.
    pub name: String,
    /// None when the upstream branch is gone.
    pub distance: Option<Distance>,
}

/// How many commits a branch has that its upstream hasn't, and the other
/// way round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Distance {
    pub ahead: u32,
    pub behind: u32,
}

impl Checkout {
    /// Whether it is on `default`, level with `origin/<default>`, with
    /// nothing changed or untracked.
    #[must_use]
    pub fn is_level(&self, default: &str) -> bool {
        self.commit.is_some()
            && self.changed == 0
            && self.branch.as_deref() == Some(default)
            && self.upstream.as_ref().is_some_and(|upstream| {
                upstream.name.strip_prefix("origin/") == Some(default)
                    && upstream.distance
                        == Some(Distance {
                            ahead: 0,
                            behind: 0,
                        })
            })
    }
}

impl fmt::Display for Checkout {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(self.branch.as_deref().unwrap_or("HEAD (no branch)"))?;
        let Some(upstream) = &self.upstream else {
            return Ok(());
        };
        write!(out, "...{}", upstream.name)?;
        match upstream.distance {
            None => out.write_str(" [gone]"),
            Some(Distance {
                ahead: 0,
                behind: 0,
            }) => Ok(()),
            Some(Distance { ahead, behind: 0 }) => {
                write!(out, " [ahead {ahead}]")
            }
            Some(Distance { ahead: 0, behind }) => {
                write!(out, " [behind {behind}]")
            }
            Some(Distance { ahead, behind }) => {
                write!(out, " [ahead {ahead}, behind {behind}]")
            }
        }
    }
}
