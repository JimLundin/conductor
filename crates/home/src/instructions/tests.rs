use proto::{Checkout, Cloning, HostPath, Origin, Project, ProjectId};

use super::{Own, Sources, compose, project_of};

fn project(checkouts: &[&str]) -> Project {
    Project {
        id: ProjectId::new(1),
        name: String::from("conductor"),
        title: String::from("Conductor"),
        remote: None,
        profiles: Vec::from([String::from("rust"), String::from("typescript")]),
        provides: Vec::new(),
        checkouts: checkouts
            .iter()
            .map(|path| Checkout {
                path: HostPath(String::from(*path)),
                origin: Origin::Found,
            })
            .collect(),
        cloning: Cloning::Idle,
        issues: Vec::new(),
    }
}

fn standards(checkouts: &[&str]) -> Project {
    let mut standards = project(checkouts);
    standards.name = String::from("standards");
    standards.title = String::from("Standards");
    standards.profiles.clear();
    standards.provides = Vec::from([String::from("rust")]);
    standards
}

#[test]
fn a_session_is_in_the_project_whose_checkout_holds_its_directory() {
    let projects = [project(&["/p/conductor", "/q/conductor/"])];
    let found = |directory: &str| {
        project_of(&projects, directory)
            .map(|(_, checkout)| checkout.path.0.clone())
    };
    assert_eq!(found("/p/conductor"), Some(String::from("/p/conductor")));
    assert_eq!(
        found("/p/conductor/crates"),
        Some(String::from("/p/conductor"))
    );
    assert_eq!(
        found("/q/conductor/docs"),
        Some(String::from("/q/conductor/"))
    );
    assert_eq!(found("/p/conductor2"), None);
    assert_eq!(found("/p"), None);
}

#[test]
fn a_nested_checkout_takes_precedence_over_its_enclosing_checkout() {
    let projects = [project(&["/outer/"]), project(&["/outer/inner"])];
    assert_eq!(
        project_of(&projects, "/outer/inner/src")
            .map(|(_, checkout)| checkout.path.0.as_str()),
        Some("/outer/inner")
    );
}

#[test]
fn a_session_takes_the_shared_rules_its_profiles_and_the_project_rules_its_cli_skips()
 {
    let project = project(&["/p/conductor"]);
    let projects = [
        project.clone(),
        standards(&["/p/standards/", "/q/standards"]),
    ];
    let sources = Sources {
        home: String::from("/data/home-repo/"),
        shared: Some(String::from("Be brief.\n")),
        own: Some(Own {
            project: &project,
            rules: Some(String::from("Gate with tools/check.sh.")),
            cli_reads_rules: false,
        }),
        projects: &projects,
    };
    assert_eq!(
        compose(&sources),
        Some(String::from(
            "# Instructions from the home\n\
             \n\
             The owner's home repository is cloned at `/data/home-repo`. \
             Paths in these rules that name the home's files, such as \
             `docs/agents/`, are in it.\n\
             \n\
             ## Rules for every session\n\
             \n\
             Be brief.\n\
             \n\
             ## Standards of Conductor\n\
             \n\
             - `rust`: its checks and configs are in \
             `/p/standards/rust/`. Run \
             `/p/standards/rust/check.sh <tier>` in the \
             session's directory.\n\
             - `typescript`: no project checked out here provides it.\n\
             \n\
             ## Rules of Conductor (its `AGENTS.md`)\n\
             \n\
             Gate with tools/check.sh.\n"
        ))
    );
}

#[test]
fn rules_the_cli_reads_by_itself_are_left_out_and_no_sources_make_none() {
    let mut project = project(&["/p/conductor"]);
    project.profiles.clear();
    let skipped = Sources {
        home: String::from("/h"),
        shared: None,
        own: Some(Own {
            project: &project,
            rules: Some(String::from("Gate with tools/check.sh.")),
            cli_reads_rules: true,
        }),
        projects: &[],
    };
    assert_eq!(
        compose(&skipped),
        Some(String::from(
            "# Instructions from the home\n\
             \n\
             The owner's home repository is cloned at `/h`. Paths in these \
             rules that name the home's files, such as `docs/agents/`, are \
             in it.\n\
             \n\
             ## Standards of Conductor\n\
             \n\
             It names no profile.\n"
        ))
    );
    let none = Sources {
        home: String::from("/h"),
        shared: Some(String::from(" \n")),
        own: None,
        projects: &[],
    };
    assert_eq!(compose(&none), None);
}
