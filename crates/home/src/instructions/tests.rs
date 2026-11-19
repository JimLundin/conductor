use proto::{Checkout, Cloning, HostPath, Origin, Project, ProjectId};

use super::{Own, Sources, compose, project_of};

fn project(checkouts: &[&str]) -> Project {
    Project {
        id: ProjectId(String::from("01J")),
        name: String::from("conductor"),
        title: String::from("Conductor"),
        remote: None,
        profiles: Vec::from([String::from("rust"), String::from("typescript")]),
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
fn a_session_takes_the_shared_rules_its_profiles_and_the_project_rules_its_cli_skips()
 {
    let project = project(&["/p/conductor"]);
    let sources = Sources {
        home: String::from("/data/home-repo/"),
        shared: Some(String::from("Be brief.\n")),
        own: Some(Own {
            project: &project,
            rules: Some(String::from("Gate with tools/check.sh.")),
            cli_reads_rules: false,
        }),
    };
    assert_eq!(
        compose(&sources),
        Some(String::from(
            "# Instructions from the home\n\
             \n\
             The owner's home repository is cloned at `/data/home-repo`. \
             Paths in these rules that name the home's files, such as \
             `standards/`, are in it.\n\
             \n\
             ## Rules for every session\n\
             \n\
             Be brief.\n\
             \n\
             ## Standards of Conductor\n\
             \n\
             - `rust`: its checks and configs are in \
             `/data/home-repo/standards/rust/`. Run \
             `/data/home-repo/standards/rust/check.sh <tier>` in the \
             session's directory.\n\
             - `typescript`: its checks and configs are in \
             `/data/home-repo/standards/typescript/`. Run \
             `/data/home-repo/standards/typescript/check.sh <tier>` in the \
             session's directory.\n\
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
    };
    assert_eq!(
        compose(&skipped),
        Some(String::from(
            "# Instructions from the home\n\
             \n\
             The owner's home repository is cloned at `/h`. Paths in these \
             rules that name the home's files, such as `standards/`, are in \
             it.\n\
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
    };
    assert_eq!(compose(&none), None);
}
