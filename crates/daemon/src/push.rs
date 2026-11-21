use std::fmt;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use data_encoding::{BASE64URL_NOPAD, HEXLOWER};
use host::log::FreshRequest;
use host::push::{Devices, Registration};
use host::serve::PushOrder;
use proto::{CommandResult, PushSubscription};
use sad_web_push::{
    ContentEncoding, HyperWebPushClient, PartialVapidSignatureBuilder,
    SubscriptionInfo, VapidSignatureBuilder, WebPushClient, WebPushError,
    WebPushMessage, WebPushMessageBuilder,
};
use serde::Serialize;
use tokio::sync::{Mutex, broadcast};

use crate::log::Log;

const TTL: u32 = 300;
const SEND_WINDOW: Duration = Duration::from_secs(8);

#[derive(Clone)]
pub struct Push {
    inner: Arc<Sender>,
}

impl fmt::Debug for Push {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str("Push")
    }
}

struct Sender {
    host: String,
    signing_key: PartialVapidSignatureBuilder,
    client: HyperWebPushClient,
    path: PathBuf,
    devices: Mutex<Devices>,
}

impl Push {
    pub(super) async fn open(
        dir: &Path,
        endpoint_id: &[u8; 32],
        log: &Log,
    ) -> anyhow::Result<Self> {
        let key = crate::load_or_create_secret(dir, "vapid.key")?;
        let signing_key = VapidSignatureBuilder::from_base64_no_sub(
            &BASE64URL_NOPAD.encode(&key),
        )?;
        let path = dir.join("push-subscriptions");
        let devices = load_devices(&path)?;
        let inner = Arc::new(Sender {
            host: HEXLOWER.encode(endpoint_id),
            signing_key,
            client: HyperWebPushClient::new()?,
            path,
            devices: Mutex::new(devices),
        });
        let live = log.follow_requests().await?;
        tokio::spawn(deliver(Arc::clone(&inner), live));
        Ok(Self { inner })
    }

    pub(super) async fn command(&self, order: PushOrder) -> CommandResult {
        match order {
            PushOrder::PublicKey => CommandResult::PushPublicKey(
                BASE64URL_NOPAD
                    .encode(&self.inner.signing_key.get_public_key()),
            ),
            PushOrder::Register(subscription) => {
                if self.inner.register(subscription).await {
                    CommandResult::Accepted
                } else {
                    CommandResult::PushUnavailable
                }
            }
        }
    }
}

impl Sender {
    async fn register(&self, subscription: PushSubscription) -> bool {
        let mut devices = self.devices.lock().await;
        match host::push::register(&devices, subscription) {
            Registration::Rejected => false,
            Registration::Unchanged => true,
            Registration::Updated(updated) => {
                self.update(&mut devices, updated)
            }
        }
    }

    async fn forget_expired(&self, subscription: &PushSubscription) {
        let mut devices = self.devices.lock().await;
        if devices.get(&subscription.endpoint) == Some(subscription) {
            let mut updated = devices.clone();
            updated.remove(&subscription.endpoint);
            self.update(&mut devices, updated);
        }
        drop(devices);
    }

    fn update(&self, devices: &mut Devices, updated: Devices) -> bool {
        let saved = postcard::to_stdvec(&updated)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| {
                save_devices(&self.path, &bytes).map_err(Into::into)
            });
        if saved.is_err() {
            tracing::warn!("could not save Web Push subscriptions");
            return false;
        }
        *devices = updated;
        true
    }

    async fn notify(
        &self,
        notice: &Notice<'_>,
        subscription: PushSubscription,
    ) {
        let subscription_info = SubscriptionInfo::new(
            subscription.endpoint.as_str(),
            subscription.p256dh.as_str(),
            subscription.auth.as_str(),
        );
        let Ok(message) =
            push_message(&self.signing_key, &subscription_info, notice)
        else {
            tracing::warn!("could not build a Web Push message");
            return;
        };
        match tokio::time::timeout(SEND_WINDOW, self.client.send(message)).await
        {
            Ok(Ok(())) => {}
            Ok(Err(
                WebPushError::EndpointNotValid(_)
                | WebPushError::EndpointNotFound(_),
            )) => {
                self.forget_expired(&subscription).await;
            }
            _ => tracing::warn!("could not deliver a Web Push message"),
        }
    }
}

fn load_devices(path: &Path) -> anyhow::Result<Devices> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(postcard::from_bytes(&bytes)?),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(Devices::new()),
        Err(error) => Err(error.into()),
    }
}

fn save_devices(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let next = path.with_extension("new");
    crate::write_private(&next, contents)?;
    std::fs::rename(next, path)?;
    if let Some(dir) = path.parent() {
        crate::sync_dir(dir)?;
    }
    Ok(())
}

#[derive(Serialize)]
struct Notice<'a> {
    host: &'a str,
    session: String,
    event: String,
    expires: u64,
}

impl<'a> Notice<'a> {
    fn from_request(host: &'a str, request: FreshRequest, now: u64) -> Self {
        Self {
            host,
            session: format!("{:032x}", request.session.0),
            event: format!("{:016x}", request.seq.0),
            expires: now.saturating_add(u64::from(TTL)),
        }
    }
}

async fn deliver(
    sender: Arc<Sender>,
    mut live: broadcast::Receiver<FreshRequest>,
) {
    loop {
        match live.recv().await {
            Ok(request) => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |time| time.as_secs());
                let notice = Notice::from_request(&sender.host, request, now);
                let devices = sender.devices.lock().await.clone();
                let _ = n0_future::join_all(
                    devices
                        .into_values()
                        .map(|device| sender.notify(&notice, device)),
                )
                .await;
            }
            Err(broadcast::error::RecvError::Lagged(_)) => {
                tracing::warn!("Web Push missed live requests");
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

fn push_message(
    key: &PartialVapidSignatureBuilder,
    subscription: &SubscriptionInfo,
    notice: &Notice<'_>,
) -> anyhow::Result<WebPushMessage> {
    let mut vapid = key.clone().add_sub_info(subscription);
    vapid.add_claim("sub", "https://jimlundin.github.io/conductor/");
    let bytes = serde_json::to_vec(notice)?;
    let mut builder = WebPushMessageBuilder::new(subscription);
    builder.set_ttl(TTL);
    builder.set_topic(notice.session.clone());
    builder.set_payload(ContentEncoding::Aes128Gcm, &bytes);
    builder.set_vapid_signature(vapid.build()?);
    Ok(builder.build()?)
}

#[cfg(test)]
mod tests {

    use proto::{EventKind, HostSeq, PushSubscription, RequestId, SessionId};
    use sad_web_push::{
        ContentEncoding, SubscriptionInfo, VapidSignatureBuilder,
    };

    use super::{
        Devices, FreshRequest, Log, Notice, TTL, load_devices, push_message,
        save_devices,
    };

    fn subscription(endpoint: &str) -> PushSubscription {
        PushSubscription {
            endpoint: endpoint.to_owned(),
            p256dh: String::from(
                "BGa4N1PI79lboMR_YrwCiCsgp35DRvedt7opHcf0yM3iOBTSoQYqQLwWxAfRKE6tsDnReWmhsImkhDF_DBdkNSU",
            ),
            auth: String::from("EvcWjEgzr4rbvhfi3yds0A"),
        }
    }

    #[test]
    fn notice_carries_only_encrypted_routing_metadata() -> anyhow::Result<()> {
        let request = FreshRequest {
            seq: HostSeq(17),
            session: SessionId(42),
        };
        let host = "0a".repeat(32);
        let notice = Notice::from_request(&host, request, 10);
        let plain = serde_json::to_value(&notice)?;
        assert_eq!(
            plain,
            serde_json::json!({
                "host": host,
                "session": "0000000000000000000000000000002a",
                "event": "0000000000000011",
                "expires": 310,
            }),
        );
        let key = VapidSignatureBuilder::from_base64_no_sub(
            "IQ9Ur0ykXoHS9gzfYX0aBjy9lvdrjx_PFUXmie9YRcY",
        )?;
        let device = subscription("https://web.push.apple.com/fixture");
        let message = push_message(
            &key,
            &SubscriptionInfo::new(device.endpoint, device.p256dh, device.auth),
            &notice,
        )?;
        let payload = message
            .payload
            .ok_or_else(|| anyhow::anyhow!("missing encrypted payload"))?;
        assert_eq!(message.ttl, TTL);
        assert_eq!(payload.content_encoding, ContentEncoding::Aes128Gcm);
        assert!(payload.content.len() > serde_json::to_vec(&notice)?.len());
        Ok(())
    }

    #[tokio::test]
    async fn replay_is_for_clients_and_only_new_requests_reach_push()
    -> anyhow::Result<()> {
        let log = Log::spawn(String::from("push test"), proto::Epoch(1));
        let session = SessionId(42);
        let question = |request| EventKind::QuestionAsked {
            request: RequestId(request),
            questions: Vec::new(),
        };
        log.append(session, question(1)).await?;
        let mut live = log.follow_requests().await?;
        let replay = log.subscribe(HostSeq(0)).await?.replay;
        assert_eq!(replay.len(), 1);
        assert!(matches!(
            live.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        log.append(session, EventKind::SessionReady).await?;
        assert!(matches!(
            live.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        log.append(session, question(2)).await?;
        let opened = live.try_recv()?;
        assert_eq!(
            opened,
            FreshRequest {
                session,
                seq: HostSeq(3)
            }
        );
        Ok(())
    }

    #[test]
    fn only_browser_push_service_endpoints_pass_validation() {
        assert!(host::push::valid(&subscription(
            "https://web.push.apple.com/fixture"
        )));
        assert!(host::push::valid(&subscription(
            "https://fcm.googleapis.com/fcm/send/fixture"
        )));
        assert!(!host::push::valid(&subscription(
            "https://www.example.com/fixture"
        )));
        assert!(!host::push::valid(&subscription(
            "https://fcm.googleapis.com/unrelated/fixture"
        )));
    }

    #[test]
    fn subscriptions_are_replaced_as_one_private_snapshot() -> anyhow::Result<()>
    {
        let binary = std::env::current_exe()?;
        let root = binary.ancestors().nth(3).ok_or_else(|| {
            anyhow::anyhow!("the test binary is not in target/debug/deps")
        })?;
        let nonce = getrandom::u64()?;
        let dir = root
            .join("tmp")
            .join(format!("conductor-push-{nonce:016x}"));
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("push-subscriptions");
        let device = subscription("https://web.push.apple.com/fixture");
        let saved = Devices::from([(device.endpoint.clone(), device)]);
        save_devices(&path, &postcard::to_stdvec(&saved)?)?;
        assert_eq!(load_devices(&path)?, saved);
        save_devices(&path, &postcard::to_stdvec(&Devices::new())?)?;
        assert!(load_devices(&path)?.is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path)?.permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
}
