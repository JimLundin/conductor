//! An instance's sign-in from the app (architecture §6.3): how it goes,
//! as the host tells every subscribed client, and the code a person pastes.
//! None of it is an event: the host stores none of it, replays none, and a
//! subscription starts with the sign-ins running then.

use core::fmt;

use alloc::string::String;

use serde::{Deserialize, Serialize};

use crate::{InstanceId, Instances};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignIn {
    pub instance: InstanceId,
    pub step: SignInStep,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignInStep {
    /// The host is starting the CLI's sign-in in the instance's home.
    Starting,
    /// The person signs in at `url`, entering `code` there if there is
    /// one. With `paste` the page then gives them a code to send back with
    /// `SubmitSignInCode`.
    Open {
        url: String,
        code: Option<String>,
        paste: bool,
    },
    /// The CLI has what it needs: it finishes, and the host checks the
    /// home and lists the instance's models.
    Finishing,
    /// The home is signed in, and the host runs these instances now.
    SignedIn {
        instances: Instances,
    },
    Failed {
        reason: String,
    },
    /// Cancelled from a client: no sign-in runs on the instance.
    Cancelled,
}

/// The code a person pastes from a sign-in page. It is a secret: its
/// `Debug` doesn't print it, so no log line carries it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignInCode(pub String);

impl fmt::Debug for SignInCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SignInCode(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::SignInCode;

    #[test]
    fn a_code_never_prints() {
        assert_eq!(
            format!("{:?}", SignInCode(String::from("abc#def"))),
            "SignInCode(..)"
        );
    }
}
