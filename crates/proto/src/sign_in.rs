//! An instance's sign-in from the app (architecture §6.3): how it goes,
//! as the host tells every subscribed client, and the code a person pastes.
//! None of it is an event: the host stores none of it, replays none, and a
//! subscription starts with the sign-ins running then.

use core::fmt;

use alloc::string::String;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::InstanceId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SignIn {
    pub instance: InstanceId,
    pub step: SignInStep,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum SignInStep {
    /// The host is starting the CLI's sign-in in the instance's home.
    Starting,
    /// The person signs in at `url`, entering `code` there if there is
    /// one. With `paste` the page then gives them a code to send back with
    /// `SubmitCode`.
    Open {
        url: String,
        code: Option<String>,
        paste: bool,
    },
    /// The CLI has what it needs: it finishes, and the host checks the
    /// home and lists the instance's models.
    Finishing,
    SignedIn,
    Failed {
        reason: String,
    },
    /// Cancelled from a client: no sign-in runs on the instance.
    Cancelled,
}

/// The code a person pastes from a sign-in page. It is a secret: its
/// `Debug` doesn't print it, so no log line carries it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SignInCode(pub String);

impl fmt::Debug for SignInCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SignInCode(..)")
    }
}

/// A command about an instance's sign-in from the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum SignInCommand {
    /// Runs the CLI's own sign-in in the instance's home; every client
    /// follows it as `Feed::SignIn`. Refused while one runs on it. Once
    /// the CLI names the account it signed in as, the instance is that
    /// account's, and its provider has a new own sign-in.
    Start { instance: InstanceId },
    /// The code the sign-in page gave, for a sign-in whose step asks for
    /// one to be pasted.
    SubmitCode {
        instance: InstanceId,
        code: SignInCode,
    },
    /// Stops the instance's sign-in, if one runs.
    Cancel { instance: InstanceId },
    /// Runs the provider's live credential setup through its own instance.
    StartCredential {
        instance: InstanceId,
        kind: String,
        label: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum SignInResult {
    Accepted,
    Refused(SignInRefusal),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum SignInRefusal {
    UnknownInstance,
    InstanceUnavailable {
        reason: String,
    },
    SignInRunning,
    /// No sign-in runs on the instance, or it takes no code now.
    NoCodeAsked,
    /// The sign-in can't make that credential, for this reason.
    InvalidCredential {
        reason: String,
    },
    /// The host could not store what the sign-in changed.
    CredentialNotSaved {
        reason: String,
    },
    /// The sign-in would change an instance sessions run on.
    InstanceInUse {
        sessions: u64,
    },
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
