use std::path::Path;

use github::configuration::selected;
use home::flow::snapshot::Snapshot;
use project_runtime::Registry;

pub fn capabilities(root: &Path, name: &str) -> (String, u8) {
    let configuration = home_files::configuration(root);
    describe(&configuration, name, &Registry::default()).map_or_else(
        |problem| (format!("home: {problem}\n"), 1),
        |report| (report, 0),
    )
}

fn describe(
    configuration: &Snapshot,
    name: &str,
    registry: &Registry,
) -> Result<String, String> {
    let project = selected(configuration, name)?;
    Ok(home::backend::report(
        &project.name,
        &registry.capabilities(&project.backends),
    ))
}

pub fn access(root: &Path, project: &str, data: Option<&str>) -> (String, u8) {
    access_report(root, project, data)
        .unwrap_or_else(|problem| (format!("home: {problem}\n"), 1))
}

fn access_report(
    root: &Path,
    project: &str,
    data: Option<&str>,
) -> Result<(String, u8), String> {
    let host_data = project_runtime::data_dir_or_default(
        data.map(std::path::PathBuf::from),
    )
    .map_err(|err| err.to_string())?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| err.to_string())?;
    runtime.block_on(github_api::project_access(root, project, &host_data))
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
