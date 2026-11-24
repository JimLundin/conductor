use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use agents::{Auth, Models, Provider, ProviderInfo};

use super::{
    Availability, Catalog, Instance, InstanceId, InstanceStatus, Instances,
    ProviderNote, Roster,
};

#[test]
fn the_form_says_why_a_provider_is_unavailable_else_its_plugin_s_note() {
    let unavailable = Availability::Unavailable {
        reason: String::from("not on PATH"),
    };
    let available = Availability::Available {
        auth: Auth::SignedIn,
        models: Models::none(),
    };
    let info = |note: Option<&str>| ProviderInfo {
        label: String::from("Agent"),
        note: note.map(String::from),
        credentials: Vec::new(),
    };
    let noted = info(Some("writes unasked"));
    let quiet = info(None);
    assert_eq!(
        [
            (Some(&noted), Some(&unavailable)),
            (Some(&quiet), Some(&unavailable)),
            (Some(&noted), Some(&available)),
            (Some(&noted), None),
            (Some(&quiet), Some(&available)),
            (None, None),
        ]
        .map(|(info, availability)| Availability::note(info, availability)),
        [
            ProviderNote::Unavailable(String::from("not on PATH")),
            ProviderNote::Unavailable(String::from("not on PATH")),
            ProviderNote::Says(String::from("writes unasked")),
            ProviderNote::Says(String::from("writes unasked")),
            ProviderNote::Nothing,
            ProviderNote::Nothing,
        ]
    );
}

#[test]
fn a_provider_is_labelled_by_its_plugin_else_by_its_name() {
    let info = ProviderInfo {
        label: String::from("Claude Code"),
        note: None,
        credentials: Vec::new(),
    };
    let catalog =
        Catalog(BTreeMap::from([(Provider::new("claude"), info.clone())]));
    assert_eq!(catalog.of(&Provider::new("claude")), Some(&info));
    assert_eq!(catalog.of(&Provider::new("codex")), None);
    assert_eq!(
        [
            catalog.label(&Provider::new("claude")),
            catalog.label(&Provider::new("codex"))
        ],
        [String::from("Claude Code"), String::from("codex")]
    );
}

#[test]
fn an_instance_is_shown_and_found_by_its_provider_and_account() {
    let own = InstanceId::own(Provider::new("claude"));
    let named = InstanceId::named(Provider::new("codex"), "me@example.com");
    assert_eq!(
        [format!("{own}"), format!("{named}")],
        [String::from("claude"), String::from("codex me@example.com")]
    );
    let available = InstanceStatus {
        availability: Availability::Unavailable {
            reason: String::from("signed out"),
        },
    };
    let instances =
        Instances(BTreeMap::from([(own.clone(), available.clone())]));
    assert_eq!(instances.of(&own), Some(&available));
    assert_eq!(instances.availability(&own), Some(&available.availability));
    assert_eq!(instances.availability(&named), None);
    let roster = Roster(BTreeMap::from([(named.clone(), Instance::default())]));
    assert_eq!(roster.of(&named), Some(&Instance::default()));
    assert_eq!(roster.of(&own), None);
}

#[test]
fn stored_rosters_use_the_current_format()
-> Result<(), Box<dyn std::error::Error>> {
    let roster = Roster(BTreeMap::from([(
        InstanceId::own(Provider::new("codex")),
        Instance::default(),
    )]));
    let bytes = postcard::to_allocvec(&roster)?;
    assert_eq!(bytes, b"\x01\x05codex\x00\x00\x00\x00");
    assert_eq!(postcard::from_bytes::<Roster>(&bytes)?, roster);
    Ok(())
}
