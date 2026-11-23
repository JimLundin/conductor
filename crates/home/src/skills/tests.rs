use super::{Plan, plan};

fn names(list: &[&str]) -> Vec<String> {
    list.iter().copied().map(String::from).collect()
}

#[test]
fn the_home_s_skills_replace_its_earlier_copies_and_skip_others() {
    let offered = Vec::from([
        (String::from("tdd"), true),
        (String::from("build"), true),
        (String::from("notes"), false),
        (String::from("theirs"), true),
    ]);
    assert_eq!(
        plan(
            "/u/.claude/skills",
            Some("build\nold\n"),
            &names(&["build", "old", "theirs", ".home-skills"]),
            offered,
        ),
        Plan {
            remove: names(&["build", "old"]),
            copy: names(&["build", "tdd"]),
            listing: String::from("build\ntdd\n"),
            said: String::from(
                "skip theirs: /u/.claude/skills/theirs was not copied from the home\n\
                 /u/.claude/skills: 2 skills from the home\n"
            ),
        }
    );
}

#[test]
fn a_listed_name_that_leaves_the_folder_is_never_removed() {
    let made = plan("d", Some("../x\n\n.\na/b\nkept\n"), &[], Vec::new());
    assert_eq!(made.remove, names(&["kept"]));
    assert_eq!(made.said, "d: 0 skills from the home\n");
}
