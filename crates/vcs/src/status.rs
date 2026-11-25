use alloc::string::String;
use core::fmt;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Checkout {
    pub commit: Option<String>,
    pub branch: Option<String>,
    pub upstream: Option<Upstream>,
    pub changed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    pub name: String,
    pub distance: Option<Distance>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Distance {
    pub ahead: u32,
    pub behind: u32,
}

impl Checkout {
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
