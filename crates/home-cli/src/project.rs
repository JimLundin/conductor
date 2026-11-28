use std::path::Path;

use home::flow::snapshot::Snapshot;
use home_files::FolderStore;
use project_runtime::Registry;
use project_runtime::tracker::Trackers;

pub fn capabilities(root: &Path, name: &str) -> (String, u8) {
    run(async {
        let configuration = home_files::configuration(root, adapters::ISSUERS);
        let data = project_runtime::data_dir_or_default(None)
            .map_err(|error| error.to_string())?;
        let mut registry = Registry::default();
        local_workspace(&configuration, name, &mut registry, &data)?;
        describe(root, &configuration, name, &mut registry, &data)
            .map(|report| (report, 0))
    })
}

fn local_workspace(
    configuration: &Snapshot,
    name: &str,
    registry: &mut Registry,
    data: &Path,
) -> Result<(), String> {
    let Some(selection) = configuration
        .project(name)
        .and_then(|project| project.backends.version_control.as_ref())
    else {
        return Ok(());
    };
    vcs_git2::registration::register(
        registry,
        vcs_git2::registration::local(data),
        selection,
        Path::new(&selection.resource),
    )
    .map_err(|reason| reason.label())
}

fn describe(
    root: &Path,
    configuration: &Snapshot,
    name: &str,
    registry: &mut Registry,
    data: &Path,
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
    let mut trackers = Trackers::new(
        data.to_path_buf(),
        Box::new(FolderStore::new(root.to_path_buf())),
        crate::resolve::connect,
    );
    trackers.follow(configuration);
    let failure = trackers
        .register(project.id, registry)?
        .err()
        .map(|error| error.to_string());
    Ok(home::backend::report(
        &project.name,
        &registry.capabilities(&project.backends),
        failure.as_deref(),
    ))
}

pub fn access(root: &Path, project: &str, data: Option<&str>) -> (String, u8) {
    run(async {
        let host_data = project_runtime::data_dir_or_default(
            data.map(std::path::PathBuf::from),
        )
        .map_err(|err| err.to_string())?;
        github_api::project_access_with_fleet(
            root,
            project,
            &host_data,
            daemon::fleet::project_credential,
        )
        .await
    })
}

pub fn run(
    task: impl Future<Output = Result<(String, u8), String>>,
) -> (String, u8) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| err.to_string())
        .and_then(|runtime| runtime.block_on(task))
        .unwrap_or_else(|problem| (format!("home: {problem}\n"), 1))
}

pub fn answer(root: &Path, args: &[String]) -> Option<(String, u8)> {
    if let Some(project) = home::backend::requested(args) {
        return Some(capabilities(root, project));
    }
    home::backend::access_requested(args)
        .map(|(project, data)| access(root, project, data))
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
            issuers: adapters::ISSUERS,
        })
    }

    #[test]
    fn a_configured_tracker_reports_its_missing_credential()
    -> Result<(), String> {
        let loaded = configuration(
            "[backends.tracker]\nadapter = \"github\"\nresource = \"owner/issues\"\n",
        );
        let report = describe(
            std::path::Path::new("."),
            &loaded,
            "local",
            &mut Registry::default(),
            std::path::Path::new("."),
        )?;
        assert!(
            report.contains("read issues: host credential is missing"),
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
        assert!(
            describe(
                std::path::Path::new("."),
                &loaded,
                "local",
                &mut Registry::default(),
                std::path::Path::new(".")
            )
            .is_err()
        );
    }

    #[test]
    fn local_and_hosting_capabilities_do_not_require_issue_flow()
    -> Result<(), String> {
        let loaded = configuration_with_flow(
            "[backends.version_control]\nadapter = \"local-vcs\"\nresource = \"/local/project\"\n[backends.hosting]\nadapter = \"github\"\nresource = \"owner/builds\"\noptions = 'queue = \"configured\"'\n",
            Err(String::from("issue flow is unavailable")),
        );
        let report = describe(
            std::path::Path::new("."),
            &loaded,
            "local",
            &mut Registry::default(),
            std::path::Path::new("."),
        )?;
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
        let report = describe(
            std::path::Path::new("."),
            &loaded,
            "local",
            &mut Registry::default(),
            std::path::Path::new("."),
        )?;
        assert!(
            report.contains("read statuses: adapter \"github\" is unavailable"),
            "{report}"
        );
        Ok(())
    }
}
