//! Web Push subscription policy for a paired host.

use alloc::collections::BTreeMap;
use alloc::string::String;

use data_encoding::BASE64URL_NOPAD;
use proto::PushSubscription;
use url::Url;

const DEVICE_LIMIT: usize = 32;

pub type Devices = BTreeMap<String, PushSubscription>;

pub enum Registration {
    Rejected,
    Unchanged,
    Updated(Devices),
}

#[must_use]
pub fn register(
    devices: &Devices,
    subscription: PushSubscription,
) -> Registration {
    if !valid(&subscription) {
        return Registration::Rejected;
    }
    if devices.get(&subscription.endpoint) == Some(&subscription) {
        return Registration::Unchanged;
    }
    if devices.len() >= DEVICE_LIMIT
        && !devices.contains_key(&subscription.endpoint)
    {
        return Registration::Rejected;
    }
    let mut updated = devices.clone();
    updated.insert(subscription.endpoint.clone(), subscription);
    Registration::Updated(updated)
}

/// Accepts endpoints of the Web Push services supported by the app.
#[must_use]
pub fn valid(subscription: &PushSubscription) -> bool {
    if subscription.endpoint.len() > 2048
        || subscription.p256dh.len() != 87
        || subscription.auth.len() != 22
    {
        return false;
    }
    let public_key = BASE64URL_NOPAD.decode(subscription.p256dh.as_bytes());
    if !public_key.is_ok_and(|key| matches!(key.as_slice(), [4, ..]))
        || BASE64URL_NOPAD
            .decode(subscription.auth.as_bytes())
            .is_err()
    {
        return false;
    }
    let Ok(endpoint) = Url::parse(&subscription.endpoint) else {
        return false;
    };
    if endpoint.scheme() != "https"
        || endpoint.port().is_some()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.fragment().is_some()
    {
        return false;
    }
    match endpoint.host_str() {
        Some("fcm.googleapis.com") => {
            endpoint.path().starts_with("/fcm/send/")
                || endpoint.path().starts_with("/wp/")
        }
        Some("updates.push.services.mozilla.com") => {
            endpoint.path().starts_with("/wpush/")
        }
        Some(host) => host.ends_with(".push.apple.com"),
        None => false,
    }
}
