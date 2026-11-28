use super::*;

fn flow() -> Result<Flow, String> {
    home::flow::load(include_str!("../../../home/src/flow/test-flow.toml"))
}

#[test]
fn each_field_has_one_mapping() -> Result<(), String> {
    let options = "[[fields]]\nname='branch'\nsource={kind='metadata'}\n[[fields]]\nname='branch'\nsource={kind='metadata'}";
    assert!(load(options, &flow()?).is_err());
    Ok(())
}

#[test]
fn defaults_follow_flow_roles() -> Result<(), String> {
    let config = load("", &flow()?)?;
    assert_eq!(config.namespace, "work/");
    assert_eq!(
        config.defaults,
        Defaults {
            open: String::from("open"),
            completed: String::from("done"),
            not_planned: String::from("dropped"),
            closed: String::from("done"),
        }
    );
    assert_eq!(
        config
            .fields
            .iter()
            .find(|field| field.name == "created")
            .map(|field| &field.source),
        Some(&FieldSource::CreationTime)
    );
    assert_eq!(
        config
            .fields
            .iter()
            .find(|field| field.name == "labels")
            .map(|field| &field.source),
        Some(&FieldSource::OrdinaryLabels)
    );
    assert_eq!(
        config
            .fields
            .iter()
            .find(|field| field.name == "branch")
            .map(|field| &field.source),
        Some(&FieldSource::Metadata)
    );
    let field = config
        .fields
        .iter()
        .find(|field| field.name == "kind")
        .ok_or("missing kind mapping")?;
    let FieldSource::ManagedLabels { values } = &field.source else {
        return Err(String::from("kind needs labels"));
    };
    assert_eq!(
        values.get("task").map(String::as_str),
        Some("work/kind/task")
    );
    Ok(())
}

#[test]
fn fields_values_and_default_statuses_must_be_declared() -> Result<(), String> {
    let flow = flow()?;
    for options in [
        "[[fields]]\nname='unknown'\nsource={kind='metadata'}",
        "[[fields]]\nname='priority'\nsource={kind='managed_labels',values={someday='work/priority/someday'}}",
        "[[fields]]\nname='priority'\nsource={kind='managed_labels',values={urgent='work/priority/urgent'}}",
        "[defaults]\nopen='unknown'\ncompleted='done'\nnot_planned='dropped'\nclosed='done'",
        "[defaults]\nopen='done'\ncompleted='done'\nnot_planned='dropped'\nclosed='done'",
        "[[fields]]\nname='created'\nsource={kind='metadata'}",
        "[[fields]]\nname='status'\nsource={kind='metadata'}",
    ] {
        assert!(
            load(options, &flow).is_err(),
            "accepted invalid mapping: {options}"
        );
    }
    Ok(())
}

#[test]
fn label_names_cannot_be_shared() -> Result<(), String> {
    let flow = flow()?;
    for options in [
        "[[fields]]\nname='priority'\nsource={kind='managed_labels',values={low='work/priority/low',normal='WORK/PRIORITY/LOW',high='work/priority/high',urgent='work/priority/urgent'}}",
        "[[fields]]\nname='priority'\nsource={kind='managed_labels',values={low='work/kind/task',normal='work/priority/normal',high='work/priority/high',urgent='work/priority/urgent'}}",
        "[[fields]]\nname='priority'\nsource={kind='managed_labels',values={low='outside/low',normal='work/priority/normal',high='work/priority/high',urgent='work/priority/urgent'}}",
        "namespace=''",
    ] {
        assert!(
            load(options, &flow).is_err(),
            "accepted conflicting mapping: {options}"
        );
    }
    Ok(())
}

#[test]
fn a_namespace_renames_the_managed_labels() -> Result<(), String> {
    let config = load("namespace='team/'", &flow()?)?;
    let field = config
        .fields
        .iter()
        .find(|field| field.name == "priority")
        .ok_or("missing priority mapping")?;
    let FieldSource::ManagedLabels { values } = &field.source else {
        return Err(String::from("priority needs labels"));
    };
    assert_eq!(
        values.get("urgent").map(String::as_str),
        Some("team/priority/urgent")
    );
    Ok(())
}

#[test]
fn tracker_options_naming_relations_fail_as_an_unknown_field()
-> Result<(), String> {
    let refused =
        load("[[relations]]\nname='parent'\nsource='metadata'", &flow()?)
            .err()
            .ok_or("accepted a relations option")?;
    assert!(refused.contains("unknown field `relations`"), "{refused}");
    Ok(())
}
