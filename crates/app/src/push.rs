use std::rc::Rc;

use data_encoding::HEXLOWER;
use dioxus::prelude::*;
use net::HostLink;
use proto::{CommandBody, CommandResult, PushSubscription, SessionId};
use ui::{HostId, Intent, Place};

use crate::Shell;

pub fn activation(hash: &str) -> Option<(HostId, SessionId)> {
    let (host_hex, session_hex) =
        hash.strip_prefix("#session=")?.split_once('/')?;
    let host =
        HostId(HEXLOWER.decode(host_hex.as_bytes()).ok()?.try_into().ok()?);
    if session_hex.len() != 32 {
        return None;
    }
    let session = u128::from_str_radix(session_hex, 16).ok()?;
    Some((host, SessionId(session)))
}

pub fn follow_activations(shell: Shell) {
    use_future(move || async move {
        let mut browser =
            document::eval("return await window.conductorPush?.watch(dioxus);");
        while let Ok(hash) = browser.recv::<String>().await {
            if let Some((host, session)) =
                activation(&hash).filter(|(host, _)| shell.is_paired(*host))
            {
                let current_link = shell.host_link(host);
                current_link
                    .as_ref()
                    .map_or_else(
                        || n0_future::Either::Right(std::future::ready(())),
                        |link| n0_future::Either::Left(link.reconnect()),
                    )
                    .await;
                shell.dispatch(Intent::Show(Place::Session(host, session)));
            }
        }
    });
}

async fn public_key(link: &HostLink) -> Option<String> {
    match link.send(CommandBody::PushPublicKey).await {
        Ok(CommandResult::PushPublicKey(key)) => Some(key),
        _ => None,
    }
}

pub fn enable(
    link: Rc<HostLink>,
    complete: impl FnOnce(Result<(), String>) + 'static,
) {
    let mut browser =
        document::eval("return await window.conductorPush.enable(dioxus);");
    spawn(async move {
        let result = async {
            browser
                .recv::<Result<(), String>>()
                .await
                .map_err(|_| browser_error())??;
            let key = public_key(&link).await;
            browser.send(&key).map_err(|_| browser_error())?;
            if key.is_none() {
                return Err(String::from(
                    "This host could not provide a notification key.",
                ));
            }
            let subscription = browser
                .join::<Result<PushSubscription, String>>()
                .await
                .map_err(|_| browser_error())??;
            match link.send(CommandBody::RegisterPush(subscription)).await {
                Ok(CommandResult::Accepted) => Ok(()),
                _ => Err(String::from(
                    "The host could not save the notification subscription.",
                )),
            }
        }
        .await;
        complete(result);
    });
}

fn browser_error() -> String {
    String::from("This browser could not enable notifications.")
}

#[cfg(test)]
mod tests {
    use proto::SessionId;
    use ui::HostId;

    use super::activation;

    #[test]
    fn activation_names_only_a_host_and_session() {
        let host = "a0".repeat(32);
        let session = "00000000000000000000000000000003";
        assert_eq!(
            activation(&format!("#session={host}/{session}")),
            Some((HostId([0xa0; 32]), SessionId(3))),
        );
        assert_eq!(
            activation(&format!("#session={host}/{session}?join=secret")),
            None
        );
    }
}
