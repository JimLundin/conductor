use std::path::Path;

use home::backend::{Backend, Selection};
use home::flow::Flow;
use home::flow::snapshot::Snapshot;
use project_runtime::Registry;

pub fn capabilities(root: &Path, name: &str) -> (String, u8) {
    let configuration = home_files::configuration(root);
    let mut registry = Registry::default();
    let result = local_workspace(&configuration, name, &mut registry)
        .and_then(|()| describe(&configuration, name, &registry));
    result.map_or_else(
        |problem| (format!("home: {problem}\n"), 1),
        |report| (report, 0),
    )
}

fn local_workspace(
    configuration: &Snapshot,
    name: &str,
    registry: &mut Registry,
) -> Result<(), String> {
    let Some(selection) = configuration
        .project(name)
        .and_then(|project| project.backends.version_control.as_ref())
    else {
        return Ok(());
    };
    let data = project_runtime::data_dir_or_default(None)
        .map_err(|error| error.to_string())?;
    vcs_git2::registration::register(
        registry,
        vcs_git2::registration::local(&data),
        selection,
        Path::new(&selection.resource),
    )
    .map_err(|reason| reason.label())
}

fn describe(
    configuration: &Snapshot,
    name: &str,
    registry: &Registry,
) -> Result<String, String> {
    let project = configuration.project(name).ok_or_else(|| {
        configuration
            .listing_problems()
            .first()
            .cloned()
            .unwrap_or_else(|| format!("unknown project {name:?}"))
    })?;
    project
        .backends
        .validate()
        .map_err(|refusal| refusal.label())?;
    [
        Backend::VersionControl,
        Backend::IssueTracker,
        Backend::RepositoryHosting,
    ]
    .into_iter()
    .filter_map(|backend| {
        project
            .backends
            .selected(backend)
            .map(|selection| (backend, selection))
    })
    .try_for_each(|(backend, selection)| {
        validate(backend, selection, configuration.flow())
    })?;
    Ok(home::backend::report(
        &project.name,
        &registry.capabilities(&project.backends),
    ))
}

fn validate(
    backend: Backend,
    selection: &Selection,
    flow: Result<&Flow, &str>,
) -> Result<(), String> {
    match (backend, selection.adapter.as_str()) {
        (Backend::IssueTracker, github::ADAPTER) => {
            github::configuration::load(&selection.options, flow?).map(drop)
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::{Registry, describe};

    fn configuration(options: &str) -> home::flow::snapshot::Snapshot {
        configuration_with_flow(
            options,
            home::flow::load(include_str!(
                "../../home/src/flow/test-flow.toml"
            )),
        )
    }

    fn configuration_with_flow(
        options: &str,
        flow: Result<home::flow::Flow, String>,
    ) -> home::flow::snapshot::Snapshot {
        let project = format!(
            "name = \"local\"\ntitle = \"Local\"\nroot = \"fc27e50efb661d7ef4a45d3edbe0234ff0a6fbd5\"\n{options}"
        );
        home::flow::snapshot::Snapshot::of(home::flow::snapshot::Inputs {
            flow,
            projects: home::project::listed(vec![(
                String::from("a1b2c3"),
                Ok(project),
            )]),
            issues: Vec::new(),
            unread: Vec::new(),
        })
    }

    #[test]
    fn configured_adapters_are_unavailable_until_registered()
    -> Result<(), String> {
        let loaded = configuration(
            "[backends.tracker]\nadapter = \"github\"\nresource = \"owner/issues\"\n",
        );
        let report = describe(&loaded, "local", &Registry::default())?;
        assert!(
            report.contains("read issues: adapter \"github\" is unavailable"),
            "{report}"
        );
        assert!(
            report.contains("capture snapshot: version control not configured"),
            "{report}"
        );
        Ok(())
    }

    #[test]
    fn invalid_provider_options_refuse_the_configuration_query() {
        let loaded = configuration(
            "[backends.tracker]\nadapter = \"github\"\nresource = \"owner/issues\"\noptions = 'namespace = \"\"'\n",
        );
        assert!(describe(&loaded, "local", &Registry::default()).is_err());
    }

    #[test]
    fn local_and_hosting_capabilities_do_not_require_issue_flow()
    -> Result<(), String> {
        let loaded = configuration_with_flow(
            "[backends.version_control]\nadapter = \"local-vcs\"\nresource = \"/local/project\"\n[backends.hosting]\nadapter = \"github\"\nresource = \"owner/builds\"\noptions = 'queue = \"configured\"'\n",
            Err(String::from("issue flow is unavailable")),
        );
        let report = describe(&loaded, "local", &Registry::default())?;
        assert!(
            report.contains(
                "capture snapshot: adapter \"local-vcs\" is unavailable"
            ),
            "{report}"
        );
        assert!(
            report.contains("read statuses: adapter \"github\" is unavailable"),
            "{report}"
        );
        assert!(
            report.contains("read issues: tracker not configured"),
            "{report}"
        );
        Ok(())
    }

    #[test]
    fn hosting_options_are_not_parsed_as_issue_mapping_options()
    -> Result<(), String> {
        let loaded = configuration(
            "[backends.hosting]\nadapter = \"github\"\nresource = \"owner/builds\"\noptions = 'queue = \"configured\"'\n",
        );
        let report = describe(&loaded, "local", &Registry::default())?;
        assert!(
            report.contains("read statuses: adapter \"github\" is unavailable"),
            "{report}"
        );
        Ok(())
    }
}
