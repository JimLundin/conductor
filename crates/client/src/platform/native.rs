//! The app runs only in the browser. A native build of the client is the
//! test harness: the daemon's e2e tests and the headless example.

use proto::ClientName;

/// Its OS. The headless example and the tests give their own names.
pub fn client_name() -> ClientName {
    ClientName(String::from(std::env::consts::OS))
}
