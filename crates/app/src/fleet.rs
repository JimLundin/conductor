use proto::fleet::{HostEntry, Pairing, Snapshot};
use proto::{AuthToken, Ticket};
use ui::{HostId, Saved};

const STORAGE: &str = "conductor.fleet.v1";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Fleet {
    pub pairing: Pairing,
    pub snapshot: Option<Snapshot>,
}

impl Fleet {
    #[must_use]
    pub fn new(pairing: Pairing) -> Self {
        let snapshot = Some(pairing.signed.snapshot.clone());
        Self { pairing, snapshot }
    }

    pub fn load() -> Option<Self> {
        let stored = local()?.get_item(STORAGE).ok().flatten()?;
        proto::fleet::decode_saved(&stored)
            .ok()
            .map(|(pairing, snapshot)| Self { pairing, snapshot })
    }

    pub fn save(&self) -> bool {
        let Ok(value) =
            proto::fleet::encode_saved(&self.pairing, self.snapshot.as_ref())
        else {
            return false;
        };
        local().is_some_and(|storage| storage.set_item(STORAGE, &value).is_ok())
    }

    pub fn host(&self, id: HostId) -> Option<&HostEntry> {
        self.hosts().iter().find(|entry| entry.node == id.0)
    }

    pub fn revoked(&self, id: HostId) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.revoked.contains(&id.0))
    }

    pub fn restored(&self, mut standalone: Vec<Saved>) -> Vec<Saved> {
        standalone.retain(|saved| {
            let id = HostId::of(&saved.ticket);
            self.host(id).is_none() && !self.revoked(id)
        });
        standalone.extend(self.saved());
        standalone
    }

    pub fn hosts(&self) -> &[HostEntry] {
        self.snapshot.as_ref().map_or_else(
            || std::slice::from_ref(&self.pairing.root),
            |snapshot| snapshot.hosts.as_slice(),
        )
    }

    pub fn saved(&self) -> Vec<Saved> {
        self.hosts()
            .iter()
            .map(|entry| Saved {
                ticket: Ticket {
                    host: entry.node,
                    relay: entry.relay.clone(),
                    token: AuthToken::from_bytes([0; 16]),
                },
                name: Some(entry.name.clone()),
            })
            .collect()
    }

    pub fn updated(
        &self,
        signed: proto::fleet::SignedSnapshot,
    ) -> Option<Self> {
        net::fleet::verify_snapshot(&signed).ok()?;
        let snapshot = signed.snapshot;
        let version =
            self.snapshot.as_ref().map_or(0, |current| current.version);
        if snapshot.fleet != self.pairing.certificate.identity.fleet
            || snapshot.root != self.pairing.root.node
            || snapshot.version <= version
        {
            return None;
        }
        Some(Self {
            pairing: self.pairing.clone(),
            snapshot: Some(snapshot),
        })
    }
}

pub fn pairing(text: &str, device: [u8; 32]) -> Option<Pairing> {
    let pairing = Pairing::parse_text(text).ok()?;
    net::fleet::verify_certificate(&pairing.certificate).ok()?;
    net::fleet::verify_snapshot(&pairing.signed).ok()?;
    pairing.matches_owner_browser(device).then_some(())?;
    net::fleet::address(&pairing.root).ok()?;
    Some(pairing)
}

pub fn pairing_image(text: &str) -> String {
    qrcode::QrCode::new(text).map_or_else(
        |_| String::new(),
        |code| {
            code.render::<qrcode::render::svg::Color>()
                .min_dimensions(160, 160)
                .build()
        },
    )
}

fn local() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

#[cfg(test)]
mod tests {
    use proto::fleet::{
        Certificate, HostEntry, Identity, Pairing, Role, SignedSnapshot,
        Snapshot,
    };
    use proto::{AuthToken, Ticket};
    use ui::{HostId, Saved, Stored};

    use super::Fleet;

    const ROOT: [u8; 32] = [1; 32];
    const REVOKED: [u8; 32] = [2; 32];
    const STANDALONE: [u8; 32] = [3; 32];

    fn fleet() -> Fleet {
        let root = HostEntry {
            node: ROOT,
            name: String::from("Fleet root"),
            relay: String::from("https://relay.example/"),
            added_at: 1,
        };
        Fleet::new(Pairing {
            certificate: Certificate {
                identity: Identity {
                    fleet: ROOT,
                    device: [4; 32],
                    name: String::from("Browser"),
                    role: Role::Owner,
                },
                signature: Vec::new(),
            },
            signed: SignedSnapshot {
                snapshot: Snapshot {
                    fleet: ROOT,
                    root: ROOT,
                    version: 2,
                    hosts: Vec::from([root.clone()]),
                    revoked: Vec::from([REVOKED]),
                },
                signature: Vec::new(),
            },
            root,
        })
    }

    fn standalone(host: [u8; 32]) -> Saved {
        Saved {
            ticket: Ticket {
                host,
                relay: String::from("https://relay.example/"),
                token: AuthToken::from_bytes([8; 16]),
            },
            name: None,
        }
    }

    #[test]
    fn reload_filters_only_revoked_standalone_records_and_keeps_legacy_migration()
     {
        let unrelated = standalone(STANDALONE);
        let revoked = standalone(REVOKED);
        let legacy = Stored {
            hosts: None,
            ticket: Some(unrelated.ticket.to_text()),
        };
        let saved = Stored {
            hosts: Some(ui::encode(&[unrelated.clone(), revoked])),
            ticket: None,
        };
        let fleet = fleet();
        let reload = |stored| fleet.restored(ui::restored(stored, Vec::new()));
        let expected =
            [unrelated, standalone(ROOT)].map(|host| HostId::of(&host.ticket));
        for record in [&legacy, &saved, &saved] {
            assert_eq!(
                reload(record)
                    .iter()
                    .map(|host| HostId::of(&host.ticket))
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }
}
