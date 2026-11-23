use iroh::{EndpointAddr, EndpointId, RelayUrl};
use proto::fleet::{Certificate, DeviceHello, HostEntry, NodeId};

#[derive(Debug, Clone, Copy, thiserror::Error)]
pub enum IdentityError {
    #[error("could not open this browser's fleet identity")]
    Key,
    #[error("could not sign with this browser's fleet identity")]
    Signature,
    #[error("invalid fleet host address")]
    Address,
    #[error("fleet certificate is invalid")]
    Certificate,
}

/// Opens the browser's non-extractable fleet identity and returns its public key.
///
/// # Errors
///
/// When the browser cannot persist or access its device identity.
pub async fn owner_public_key() -> Result<NodeId, IdentityError> {
    browser::public_key().await
}

/// Builds a host's address from its fleet roster entry.
///
/// # Errors
///
/// When the host node id or relay URL is malformed.
pub fn address(host: &HostEntry) -> Result<EndpointAddr, IdentityError> {
    let id = EndpointId::from_bytes(&host.node)
        .map_err(|_| IdentityError::Address)?;
    let relay = host
        .relay
        .parse::<RelayUrl>()
        .map_err(|_| IdentityError::Address)?;
    Ok(EndpointAddr::new(id).with_relay_url(crate::without_root_dot(&relay)))
}

/// Checks the root signature on a browser certificate.
///
/// # Errors
///
/// If the root key or certificate signature is invalid.
pub fn verify_certificate(
    certificate: &Certificate,
) -> Result<(), IdentityError> {
    use iroh::{PublicKey, Signature};
    use proto::fleet::CERTIFICATE_DOMAIN;

    let root = PublicKey::from_bytes(&certificate.identity.fleet)
        .map_err(|_| IdentityError::Certificate)?;
    let signature = Signature::from_bytes(
        certificate
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| IdentityError::Certificate)?,
    );
    let message =
        postcard::to_allocvec(&(CERTIFICATE_DOMAIN, &certificate.identity))
            .map_err(|_| IdentityError::Certificate)?;
    root.verify(&message, &signature)
        .map_err(|_| IdentityError::Certificate)
}

/// Checks the root signature on the browser-visible fleet snapshot.
///
/// # Errors
///
/// If the snapshot signature is invalid.
pub fn verify_snapshot(
    signed: &proto::fleet::SignedSnapshot,
) -> Result<(), IdentityError> {
    use iroh::{PublicKey, Signature};

    let root = PublicKey::from_bytes(&signed.snapshot.fleet)
        .map_err(|_| IdentityError::Certificate)?;
    let signature = Signature::from_bytes(
        signed
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| IdentityError::Certificate)?,
    );
    let message = postcard::to_allocvec(&(
        proto::fleet::SNAPSHOT_DOMAIN,
        &signed.snapshot,
    ))
    .map_err(|_| IdentityError::Certificate)?;
    root.verify(&message, &signature)
        .map_err(|_| IdentityError::Certificate)
}

pub(crate) async fn hello(
    certificate: Certificate,
    host: EndpointId,
    local: EndpointId,
) -> Result<DeviceHello, IdentityError> {
    let data = proto::fleet::device_proof(
        &certificate.identity.fleet,
        host.as_bytes(),
        local.as_bytes(),
    );
    let proof = browser::signature(&data).await?;
    Ok(DeviceHello { certificate, proof })
}

#[cfg(not(target_family = "wasm"))]
mod browser {
    use super::{IdentityError, NodeId};

    pub async fn public_key() -> Result<NodeId, IdentityError> {
        core::future::ready(Err(IdentityError::Key)).await
    }

    pub async fn signature(_data: &[u8]) -> Result<Vec<u8>, IdentityError> {
        core::future::ready(Err(IdentityError::Signature)).await
    }
}

#[cfg(target_family = "wasm")]
mod browser {
    use js_sys::{Promise, Uint8Array};
    use tokio::sync::oneshot;
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;

    use super::{IdentityError, NodeId};

    #[wasm_bindgen(module = "/src/fleet.js")]
    extern "C" {
        #[wasm_bindgen(js_name = fleetPublicKey)]
        fn fleet_public_key() -> Promise;
        #[wasm_bindgen(js_name = signFleet)]
        fn sign_fleet(data: Uint8Array) -> Promise;
    }

    /// Awaits `promise` in a task of its own and hands back its bytes, so a
    /// caller's future holds only the `Send` receiver: a `JsFuture` is
    /// never `Send`. Not an `async fn`, whose future would hold the promise.
    fn bytes(promise: Promise) -> oneshot::Receiver<Option<Vec<u8>>> {
        let (reply, answer) = oneshot::channel();
        wasm_bindgen_futures::spawn_local(async move {
            let result = JsFuture::from(promise)
                .await
                .ok()
                .map(|value| Uint8Array::new(&value).to_vec());
            let _ = reply.send(result);
        });
        answer
    }

    pub async fn public_key() -> Result<NodeId, IdentityError> {
        bytes(fleet_public_key())
            .await
            .ok()
            .flatten()
            .ok_or(IdentityError::Key)?
            .try_into()
            .map_err(|_| IdentityError::Key)
    }

    pub async fn signature(data: &[u8]) -> Result<Vec<u8>, IdentityError> {
        bytes(sign_fleet(Uint8Array::from(data)))
            .await
            .ok()
            .flatten()
            .ok_or(IdentityError::Signature)
    }
}
