use crate::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exit {
    Status(String),
    Unknown(String),
}

impl Exit {
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Status(status) => status.clone(),
            Self::Unknown(error) => format!("unknown exit ({error})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Exit;

    #[test]
    fn an_exit_is_described_by_its_status_or_why_it_is_unknown() {
        assert_eq!(
            Exit::Status(String::from("exit status: 1")).describe(),
            "exit status: 1"
        );
        assert_eq!(
            Exit::Unknown(String::from("no child")).describe(),
            "unknown exit (no child)"
        );
    }
}
