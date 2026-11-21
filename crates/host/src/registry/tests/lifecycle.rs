use agents::{ConfigurationFailure, Retirement};

use super::*;

#[test]
fn busy_removal_blocks_work_and_escalates_on_the_supplied_clock() {
    let id = api();
    let mut registry = host_of(&[("fixture", id.clone())]);
    registry.admit(SessionId(1), id.clone(), 1);
    let effects = registry.remove_instance(id.clone(), "remove");
    let saved = saving(&mut registry, effects);
    let generation = saved.iter().find_map(|effect| match effect {
        Effect::Retire {
            id,
            stage: Retirement::Drain,
            ..
        } => Some(*id),
        _ => None,
    });
    assert!(generation.is_some());
    assert!(registry.for_order(SessionId(1), true).is_err());
    assert_eq!(registry.tick(4), []);
    assert!(matches!(
        registry.tick(5).as_slice(),
        [Effect::Retire {
            stage: Retirement::Interrupt,
            ..
        }]
    ));
    assert!(matches!(
        registry.tick(10).as_slice(),
        [Effect::Retire {
            stage: Retirement::Kill,
            ..
        }]
    ));
    assert_eq!(
        registry.retired(
            &id,
            generation.unwrap_or_default(),
            Retirement::Kill,
            false
        ),
        []
    );
    assert!(registry.retirements.contains_key(&id));
    assert_eq!(
        registry.retired(
            &id,
            generation.unwrap_or_default(),
            Retirement::Kill,
            true
        ),
        []
    );
    assert!(!registry.retirements.contains_key(&id));
}

#[test]
fn retirement_coalesces_replacement_and_rejects_a_stale_launch()
-> Result<(), &'static str> {
    let id = api();
    let mut registry = host_of(&[("fixture", id.clone())]);
    let effects = registry.remove_instance(id.clone(), "remove");
    let _ = saving(&mut registry, effects);
    let generation = registry.jobs;
    for value in ["first", "latest"] {
        let changed =
            registry.set_credential(Provider::Claude, gateway(value), "set");
        assert!(
            !saving(&mut registry, changed)
                .iter()
                .any(|effect| matches!(effect, Effect::Start(_)))
        );
    }
    let effects = registry.retired(&id, generation, Retirement::Kill, true);
    let Some(Effect::Start(starting)) = effects.first() else {
        return Err("replacement did not launch after retirement");
    };
    assert_eq!(starting.settings.credential, Some(gateway("latest")));
    let current = starting.id;
    assert!(matches!(
        registry
            .started(Started {
                id: generation,
                instance: id.clone(),
                launcher: ready("stale", OwnSignIn::Missing)
            })
            .as_slice(),
        [Effect::Retire {
            adapter: Some("stale"),
            stage: Retirement::Kill,
            ..
        }]
    ));
    assert!(
        matches!(registry.refreshes.get(&id), Some(super::super::Refresh::Starting { id: active, .. }) if *active == current)
    );
    Ok(())
}

#[test]
fn temporary_failures_back_off_and_refusals_wait_for_correction()
-> Result<(), &'static str> {
    let id = api();
    let mut registry = host_of(&[("fixture", id.clone())]);
    for attempt in 0..5 {
        let effects = registry.schedule(id.clone(), Vec::new());
        let Some(Effect::Refresh(job)) = effects.first() else {
            return Err("refresh was not scheduled");
        };
        let _ = registry.refreshed(Refreshed {
            id: job.id,
            instance: id.clone(),
            configured: Err(ConfigurationFailure::Temporary(text(
                "unreachable",
            ))),
        });
        let retry = registry.retries.get(&id);
        assert_eq!(
            retry.map(|scheduled| scheduled.at),
            Some(2_u64.pow(attempt))
        );
    }
    let effects = registry.schedule(id.clone(), Vec::new());
    let Some(Effect::Refresh(job)) = effects.first() else {
        return Err("refresh was not scheduled");
    };
    let _ = registry.refreshed(Refreshed {
        id: job.id,
        instance: id,
        configured: Err(ConfigurationFailure::Refused(text(
            "credential refused",
        ))),
    });
    assert_eq!(registry.tick(1000), []);
    assert_eq!(registry.retries, BTreeMap::new());
    Ok(())
}

#[test]
fn restart_blocks_old_shared_credentials_before_boot() {
    let id = api();
    let stored = Stored::Opened(Roster(BTreeMap::from([(
        id.clone(),
        Instance {
            credential: Some(gateway("old")),
            ..Instance::default()
        },
    )])));
    let mut registry = Test::new(&stored, catalog());
    let desired = Roster(BTreeMap::from([(
        id.clone(),
        Instance {
            credential: Some(gateway("new")),
            ..Instance::default()
        },
    )]));
    registry.desire(9, &desired, &BTreeSet::from([id.clone()]));
    assert!(
        !registry
            .boot()
            .iter()
            .any(|starting| starting.instance == id)
    );
    assert!(registry.launch_session(&id, true, None, None).is_err());
    assert_eq!(
        registry
            .application()
            .and_then(|application| application.adopted),
        None
    );
}

#[test]
fn desired_removal_retires_before_local_storage_recovers()
-> Result<(), &'static str> {
    let id = api();
    let mut registry = host_of(&[("fixture", id.clone())]);
    registry.admit(SessionId(1), id.clone(), 1);
    let effects =
        registry.desire(2, &Roster::default(), &BTreeSet::from([id.clone()]));
    let Some(Effect::Retire {
        id: generation,
        stage: Retirement::Drain,
        ..
    }) = effects.first()
    else {
        return Err("observed removal did not begin retirement");
    };
    assert!(registry.for_order(SessionId(1), true).is_err());
    let observed = *generation;
    let _ = registry.tick(10);
    assert_eq!(registry.retired(&id, observed, Retirement::Kill, true), []);
    assert!(registry.launch_session(&id, true, None, None).is_err());
    assert_eq!(
        registry.desire(2, &Roster::default(), &BTreeSet::from([id])),
        []
    );
    Ok(())
}
