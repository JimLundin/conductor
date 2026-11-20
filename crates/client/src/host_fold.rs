use alloc::collections::BTreeMap;
use core::mem;

use crate::host_view::HostView;
use crate::link::{LinkState, Status, Update};
use crate::prelude::*;
use proto::{
    Catalog, CommandResult, Feed, Home, InstanceId, Instances, SignIn,
    SignInStep,
};

/// What a host has reported to this client.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HostFold {
    pub status: Option<Status>,
    pub name: Option<String>,
    pub instances: Option<Instances>,
    pub catalog: Catalog,
    pub home: Option<Home>,
    pub sign_ins: BTreeMap<InstanceId, SignInStep>,
    pub view: HostView,
}

impl HostFold {
    #[must_use]
    pub fn live(&self) -> bool {
        self.status
            .as_ref()
            .is_some_and(|status| status.state == LinkState::Live)
    }

    pub fn apply(&mut self, update: Update) {
        match update {
            Update::Status(status) => self.status = Some(status),
            Update::Reset(_) => self.view = HostView::empty(),
            Update::Welcome(welcome) => {
                self.name = Some(welcome.host);
                self.instances = Some(welcome.instances);
                self.catalog = welcome.catalog;
                self.home = welcome.home;
                self.sign_ins.clear();
            }
            Update::Feed(Feed::Event(event)) => {
                self.view = mem::take(&mut self.view).apply(event);
            }
            Update::Feed(Feed::Home(home)) => self.home = Some(home),
            Update::Feed(Feed::SignIn(sign_in)) => self.sign_in(sign_in),
            Update::Answer(CommandResult::Instances(instances)) => {
                self.instances = Some(instances);
            }
            Update::Answer(_) => {}
        }
    }

    fn sign_in(&mut self, SignIn { instance, step }: SignIn) {
        match &step {
            SignInStep::SignedIn { instances } => {
                self.instances = Some(instances.clone());
            }
            SignInStep::Cancelled => {
                self.sign_ins.remove(&instance);
                return;
            }
            _ => {}
        }
        self.sign_ins.insert(instance, step);
    }
}

#[cfg(test)]
mod tests {
    use super::HostFold;
    use crate::link::{LinkState, Status, Update};
    use alloc::collections::BTreeMap;
    use proto::{
        Availability, Catalog, ClientName, CommandResult, Epoch, Event,
        EventKind, Feed, Home, HomeSync, HostPath, HostSeq, InstanceId,
        InstanceStatus, Instances, Provider, SessionId, SignIn, SignInStep,
        Welcome,
    };

    fn instances(reason: &str) -> Instances {
        Instances(BTreeMap::from([(
            InstanceId::own(Provider::Claude),
            InstanceStatus {
                availability: Availability::Unavailable {
                    reason: String::from(reason),
                },
            },
        )]))
    }

    fn welcome(epoch: u64) -> Update {
        Update::Welcome(Welcome {
            host: String::from("desk"),
            epoch: Epoch(epoch),
            head_seq: HostSeq(0),
            instances: instances("welcome"),
            catalog: Catalog::default(),
            home: None,
        })
    }

    fn sign_in(step: SignInStep) -> Update {
        Update::Feed(Feed::SignIn(SignIn {
            instance: InstanceId::own(Provider::Claude),
            step,
        }))
    }

    #[test]
    fn the_welcome_feed_and_answers_update_the_advertised_facts() {
        let mut host = HostFold::default();
        host.apply(welcome(1));
        assert_eq!(host.name.as_deref(), Some("desk"));
        let home = Home {
            remote: String::from("file:///home.git"),
            synced: None,
            sync: HomeSync::Syncing,
            unpushed: 0,
            projects: Vec::new(),
        };
        host.apply(Update::Feed(Feed::Home(home.clone())));
        host.apply(sign_in(SignInStep::SignedIn {
            instances: instances("signed in"),
        }));
        assert_eq!(
            (&host.home, &host.instances),
            (&Some(home), &Some(instances("signed in")))
        );
        host.apply(sign_in(SignInStep::Cancelled));
        host.apply(Update::Answer(CommandResult::Instances(instances(
            "command",
        ))));
        assert_eq!(
            (&host.instances, host.sign_ins.is_empty()),
            (&Some(instances("command")), true)
        );
        host.apply(sign_in(SignInStep::Starting));
        host.apply(welcome(1));
        assert_eq!(
            (host.home, host.instances, host.sign_ins.is_empty()),
            (None, Some(instances("welcome")), true)
        );
    }

    #[test]
    fn reconnect_keeps_the_sessions_until_an_epoch_reset() {
        let mut host = HostFold::default();
        host.apply(welcome(1));
        host.apply(Update::Feed(Feed::Event(Event {
            seq: HostSeq(1),
            session: SessionId(1),
            kind: EventKind::SessionCreated {
                instance: InstanceId::own(Provider::Claude),
                provider: Provider::Claude,
                directory: HostPath(String::from("/project")),
                by: ClientName(String::from("desk")),
                model: None,
                effort: None,
            },
        })));
        host.apply(Update::Status(Status::default()));
        host.apply(welcome(1));
        host.apply(Update::Status(Status {
            state: LinkState::Live,
            error: None,
        }));
        assert_eq!((host.live(), host.view.sessions.len()), (true, 1));
        host.apply(Update::Reset(Epoch(2)));
        assert!(host.view.sessions.is_empty());
    }
}
