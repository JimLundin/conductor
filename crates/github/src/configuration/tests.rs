use super::*;

fn flow() -> Result<Flow, String> {
    home::flow::load(include_str!("../../../home/src/flow/test-flow.toml"))
}

#[test]
fn each_field_and_relation_has_one_mapping() -> Result<(), String> {
    let flow = flow()?;
    for options in [
        "[[fields]]\nname='branch'\nsource={kind='metadata'}\n[[fields]]\nname='branch'\nsource={kind='metadata'}",
        "[[relations]]\nname='parent'\nsource='parent'\n[[relations]]\nname='parent'\nsource='metadata'",
    ] {
        assert!(
            load(options, &flow).is_err(),
            "accepted duplicate mapping: {options}"
        );
    }
    Ok(())
}

#[test]
fn defaults_follow_flow_roles_and_each_relation_has_one_source()
-> Result<(), String> {
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
    assert_eq!(
        config.relations,
        Vec::from([
            RelationMapping {
                name: String::from("parent"),
                source: RelationSource::Parent
            },
            RelationMapping {
                name: String::from("blocked_by"),
                source: RelationSource::Dependency
            },
        ])
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
        "[[relations]]\nname='unknown'\nsource='metadata'",
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
fn label_names_and_native_relation_sources_cannot_be_shared()
-> Result<(), String> {
    let flow = flow()?;
    for options in [
        "[[fields]]\nname='priority'\nsource={kind='managed_labels',values={low='work/priority/low',normal='WORK/PRIORITY/LOW',high='work/priority/high',urgent='work/priority/urgent'}}",
        "[[fields]]\nname='priority'\nsource={kind='managed_labels',values={low='work/kind/task',normal='work/priority/normal',high='work/priority/high',urgent='work/priority/urgent'}}",
        "[[fields]]\nname='priority'\nsource={kind='managed_labels',values={low='outside/low',normal='work/priority/normal',high='work/priority/high',urgent='work/priority/urgent'}}",
        "[[relations]]\nname='blocked_by'\nsource='parent'",
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
fn configured_metadata_relations_preserve_the_domains_broader_graph()
-> Result<(), String> {
    let mut flow = flow()?;
    flow.relations.push(home::flow::Relation {
        name: String::from("related"),
        from: Vec::new(),
        to: Vec::new(),
        many: true,
        cycles: true,
        shape: home::flow::Shape::Dependency,
        reverse: String::from("related_from"),
    });
    let config = load(
        "namespace='team/'\n[[relations]]\nname='parent'\nsource='metadata'",
        &flow,
    )?;
    assert_eq!(
        config
            .relations
            .iter()
            .find(|relation| relation.name == "parent")
            .map(|relation| relation.source),
        Some(RelationSource::Metadata)
    );
    assert_eq!(
        config
            .relations
            .iter()
            .find(|relation| relation.name == "related")
            .map(|relation| relation.source),
        Some(RelationSource::Metadata)
    );
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
