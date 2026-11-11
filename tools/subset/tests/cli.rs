//! Drives the `subset` binary the way the pair loop does and asserts its
//! exact output.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn subset(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_subset"))
        .args(args)
        .output()
        .expect("run subset");
    (
        out.status.code().expect("exit code"),
        String::from_utf8(out.stdout).expect("utf-8 stdout"),
        String::from_utf8(out.stderr).expect("utf-8 stderr"),
    )
}

fn check_fixtures() -> (i32, String) {
    let config = fixtures().join("subset.toml");
    let (code, stdout, _) = subset(&["check", "--config", config.to_str().unwrap()]);
    (code, stdout)
}

fn check_lines(file: &str) -> Vec<String> {
    let prefix = format!("{file}:");
    check_fixtures()
        .1
        .lines()
        .filter(|line| line.starts_with(&prefix))
        .map(str::to_string)
        .collect()
}

/// A scratch directory holding `subset.toml` and the given files.
fn scratch(name: &str, config: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("subset-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("subset.toml"), config).unwrap();
    for (path, text) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir.join("subset.toml")
}

/// `subset check` output for `files` laid out as in the repository, under
/// the repository's own `subset.toml`.
fn check_repo_config(name: &str, files: &[(&str, &str)]) -> String {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config = std::fs::read_to_string(repo.join("subset.toml")).unwrap();
    let config = scratch(name, &config, files);
    let (_, stdout, stderr) = subset(&["check", "--config", config.to_str().unwrap()]);
    assert_eq!(stderr, "");
    stdout
}

#[test]
fn the_fixture_check_fails_with_exit_code_1_and_reports_only_fixture_files() {
    let (code, stdout) = check_fixtures();
    assert_eq!(code, 1);
    let mut files: Vec<&str> = stdout
        .lines()
        .map(|line| line.split(':').next().unwrap())
        .collect();
    files.dedup();
    assert_eq!(
        files,
        [
            "attrs.rs",
            "bindings.rs",
            "closures.rs",
            "control.rs",
            "effects.rs",
            "items.rs",
            "macros.rs",
            "operators.rs",
            "tests/in_tests_dir.rs",
            "types.rs",
            "values.rs",
        ]
    );
}

#[test]
fn the_core_fixture_passes() {
    assert_eq!(check_lines("allowed.rs"), Vec::<String>::new());
}

#[test]
fn the_core_fixture_uses_every_core_construct() {
    let config = fixtures().join("core.toml");
    let (code, stdout, _) = subset(&["report", "--config", config.to_str().unwrap(), "--json"]);
    assert_eq!(code, 0);
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let mut ids: Vec<&str> = report["ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    ids.sort();
    assert_eq!(
        ids,
        [
            "attr.cfg_test",
            "attr.derive",
            "attr.doc",
            "expr.block",
            "expr.call",
            "expr.closure.adapter",
            "expr.field",
            "expr.lit",
            "expr.match",
            "expr.method_call",
            "expr.paren",
            "expr.path",
            "expr.reference",
            "expr.struct",
            "expr.try",
            "item.const",
            "item.enum",
            "item.fn",
            "item.impl.inherent",
            "item.mod",
            "item.struct.named",
            "item.struct.newtype",
            "item.use",
            "macro.allowed",
            "pat.ident",
            "pat.lit",
            "pat.or",
            "pat.path",
            "pat.struct",
            "pat.tuple_struct",
            "pat.wild",
            "stmt.let",
            "type.path",
            "type.reference",
        ]
    );
}

#[test]
fn derives_outside_the_list_and_other_attributes() {
    assert_eq!(
        check_lines("attrs.rs"),
        [
            "attrs.rs:6:1: attr.derive.serde::Serialize: derive of a name not in `derives`",
            "attrs.rs:6:1: attr.derive.Default: derive of a name not in `derives`",
            "attrs.rs:11:1: attr.allow: attribute not in the subset",
            "attrs.rs:12:1: attr.cfg: attribute not in the subset",
        ]
    );
}

#[test]
fn mutable_and_refutable_bindings() {
    assert_eq!(
        check_lines("bindings.rs"),
        [
            "bindings.rs:1:29: pat.ident.mut: `mut` binding outside `let`",
            "bindings.rs:2:5: stmt.let.mut: `let` with a `mut` binding",
            "bindings.rs:3:5: stmt.let.mut: `let` with a `mut` binding",
            "bindings.rs:3:9: pat.tuple: tuple pattern",
            "bindings.rs:3:22: expr.tuple: tuple expression",
            "bindings.rs:4:5: stmt.let.else: `let ... else`",
            "bindings.rs:5:9: expr.return: `return`",
            "bindings.rs:7:5: stmt.let.uninit: `let` without an initializer",
            "bindings.rs:9:14: pat.ident.ref: `ref` binding",
            "bindings.rs:10:15: pat.ident.at: `name @ pattern` binding",
        ]
    );
}

#[test]
fn closures_outside_method_arguments() {
    assert_eq!(
        check_lines("closures.rs"),
        [
            "closures.rs:2:18: expr.closure: closure that is not a direct argument of a method call",
            "closures.rs:3:24: expr.closure: closure that is not a direct argument of a method call",
        ]
    );
}

#[test]
fn control_flow_other_than_match() {
    assert_eq!(
        check_lines("control.rs"),
        [
            "control.rs:2:5: expr.if: `if` expression",
            "control.rs:3:9: expr.return: `return`",
            "control.rs:4:12: expr.if: `if` expression",
            "control.rs:5:9: expr.return: `return`",
            "control.rs:7:5: expr.if_let.single: `if let` with no `else`",
            "control.rs:8:9: expr.return: `return`",
            "control.rs:10:5: expr.while: `while` loop",
            "control.rs:11:9: expr.break: `break`",
            "control.rs:13:5: expr.while_let: `while let` loop",
            "control.rs:14:9: expr.continue: `continue`",
            "control.rs:16:5: expr.for: `for` loop",
            "control.rs:17:5: expr.loop: `loop`",
            "control.rs:19:17: expr.match.guard: `if` guard on a match arm",
        ]
    );
}

#[test]
fn if_let_only_with_a_single_branch() {
    assert_eq!(
        rule_case(
            "if-let",
            "crates/daemon/src/a.rs",
            "fn cancel(open: Option<Approval>) {\n    if let Some(approval) = open {\n        approval.cancel();\n    }\n}\n",
            "fn label(name: Option<&str>, kind: Option<Kind>) -> String {\n    if let Some(name) = name {\n        name.to_string()\n    } else {\n        String::new()\n    }\n}\n\nfn note(name: Option<&str>, kind: Option<Kind>) {\n    if let Some(name) = name {\n        log(name);\n    } else if let Some(kind) = kind {\n        log(kind.label());\n    }\n    if let Some(name) = name && let Some(kind) = kind {\n        log(name);\n    }\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:2:5: expr.if_let: `if let` with an `else`, or an `if` with a let-chain\n\
         crates/daemon/src/a_fail.rs:10:5: expr.if_let: `if let` with an `else`, or an `if` with a let-chain\n\
         crates/daemon/src/a_fail.rs:15:5: expr.if_let: `if let` with an `else`, or an `if` with a let-chain\n\
         crates/daemon/src/a_fail.rs:15:8: expr.let: `let` inside a condition chain\n\
         crates/daemon/src/a_fail.rs:15:30: expr.binary.and: a boolean expression (`matches!`, a comparison, `&&`, `||`, `!`) only as the body of a closure passed straight to a method in `predicate_adapters` (proposals 0028, 0037)\n\
         crates/daemon/src/a_fail.rs:15:33: expr.let: `let` inside a condition chain\n"
    );
}

#[test]
fn async_and_unsafe() {
    assert_eq!(
        check_lines("effects.rs"),
        [
            "effects.rs:2:17: expr.async: async block",
            "effects.rs:3:24: expr.await: `.await`",
            "effects.rs:4:5: expr.unsafe: unsafe block",
        ]
    );
}

#[test]
fn items_outside_the_core() {
    assert_eq!(
        check_lines("items.rs"),
        [
            "items.rs:1:1: item.struct.tuple: tuple struct with zero or several unnamed fields",
            "items.rs:2:1: item.struct.unit: unit struct",
            "items.rs:3:1: vis.pub: `pub` visibility",
            "items.rs:4:5: vis.restricted: `pub(crate)`, `pub(super)` or `pub(in path)` visibility",
            "items.rs:7:7: item.enum.discriminant: explicit enum discriminant",
            "items.rs:9:1: item.trait: trait definition",
            "items.rs:10:5: item.trait.fn: fn in a trait",
            "items.rs:12:1: item.impl.trait: trait impl",
            "items.rs:18:14: item.fn.receiver.mut: `mut self` or `&mut self` receiver",
            "items.rs:20:1: item.static: static item",
            "items.rs:21:1: item.type: type alias",
            "items.rs:22:1: item.macro_rules: macro_rules! definition",
            "items.rs:25:14: item.use.glob: glob import",
            "items.rs:26:22: item.use.rename: import renamed with `as`",
            "items.rs:27:1: item.fn.const: const fn",
            "items.rs:30:1: item.fn.async: async fn",
            "items.rs:31:1: item.fn.unsafe: unsafe fn",
        ]
    );
}

#[test]
fn macros_outside_the_lists_and_the_test_profile() {
    assert_eq!(
        check_lines("macros.rs"),
        [
            "macros.rs:3:5: macro.info: macro not listed in `macros`",
            "macros.rs:4:5: macro.println: macro not listed in `macros`",
            "macros.rs:5:5: macro.assert_eq: macro not listed in `macros`",
            "macros.rs:13:9: macro.assert: macro not listed in `macros`",
        ]
    );
}

#[test]
fn operators() {
    assert_eq!(
        check_lines("operators.rs"),
        [
            "operators.rs:2:17: expr.binary.add: `+` operator",
            "operators.rs:3:18: expr.binary.and: `&&` operator",
            "operators.rs:4:18: expr.binary.eq: `==` operator",
            "operators.rs:5:15: expr.unary.not: `!` operator",
            "operators.rs:6:15: expr.unary.neg: unary `-`",
            "operators.rs:7:17: expr.unary.deref: `*` dereference",
            "operators.rs:8:18: expr.cast: `as` cast",
            "operators.rs:9:18: expr.index: indexing",
            "operators.rs:10:17: expr.range: range expression",
            "operators.rs:11:7: expr.assign: assignment",
            "operators.rs:12:7: expr.binary.add_assign: compound assignment is not allowed; write `x = x.checked_add(1)?` (proposal 0008)",
        ]
    );
}

#[test]
fn test_macros_in_a_tests_directory() {
    assert_eq!(
        check_lines("tests/in_tests_dir.rs"),
        ["tests/in_tests_dir.rs:3:5: macro.assert_ne: macro not listed in `macros`",]
    );
}

#[test]
fn types_and_generics() {
    assert_eq!(
        check_lines("types.rs"),
        [
            "types.rs:1:10: generics.lifetime: lifetime parameter",
            "types.rs:1:14: generics.type_param: generic type parameter",
            "types.rs:1:23: type.bool: `bool` type",
            "types.rs:1:33: type.impl_trait.arg: `impl Trait` outside a return type",
            "types.rs:1:53: type.reference.mut: `&mut T`",
            "types.rs:1:67: type.lifetime: lifetime in a type or bound",
            "types.rs:1:78: type.impl_trait.ret: `impl Trait` in a return type",
            "types.rs:2:1: generics.where: where clause",
            "types.rs:9:14: type.dyn: trait object",
            "types.rs:10:11: type.tuple: tuple type",
            "types.rs:11:11: type.array: array type",
            "types.rs:12:13: type.lifetime: lifetime in a type or bound",
            "types.rs:12:21: type.slice: slice type",
            "types.rs:13:14: type.unit: `()` type",
        ]
    );
}

#[test]
fn booleans_tuples_and_patterns() {
    assert_eq!(
        check_lines("values.rs"),
        [
            "values.rs:2:15: expr.lit.bool: `true` / `false` literal",
            "values.rs:3:16: expr.tuple: tuple expression",
            "values.rs:4:16: expr.unit: `()` value",
            "values.rs:5:16: expr.array: array expression",
            "values.rs:6:14: pat.type: type ascription in `let` or a closure parameter",
            "values.rs:8:14: pat.lit.bool: `true` / `false` pattern",
            "values.rs:8:29: expr.lit.bool: `true` / `false` literal",
            "values.rs:9:14: pat.lit.bool: `true` / `false` pattern",
            "values.rs:9:28: expr.lit.bool: `true` / `false` literal",
            "values.rs:13:17: type.tuple: tuple type",
            "values.rs:13:36: type.slice: slice type",
            "values.rs:15:9: pat.tuple: tuple pattern",
            "values.rs:15:13: pat.rest: `..` pattern",
            "values.rs:16:9: pat.tuple: tuple pattern",
            "values.rs:16:11: pat.range: range pattern",
        ]
    );
}

#[test]
fn a_clean_tree_exits_0_with_no_output() {
    let config = scratch(
        "clean",
        "version = 0\ninclude = [\"*.rs\"]\nallowed = [\"item.fn\"]\n",
        &[("lib.rs", "fn f() {}\n")],
    );
    assert_eq!(
        subset(&["check", "--config", config.to_str().unwrap()]),
        (0, String::new(), String::new())
    );
}

#[test]
fn a_parse_error_exits_2_with_its_location() {
    let config = scratch(
        "parse",
        "version = 0\ninclude = [\"*.rs\"]\nallowed = []\n",
        &[("lib.rs", "fn f( {}\n")],
    );
    let (code, stdout, stderr) = subset(&["check", "--config", config.to_str().unwrap()]);
    assert_eq!((code, stdout), (2, String::new()));
    assert_eq!(
        stderr.lines().next().unwrap(),
        "subset: lib.rs:1:5: parse error: cannot parse string into token stream"
    );
}

#[test]
fn an_unknown_config_key_exits_2() {
    let config = scratch(
        "config",
        "version = 0\ninclude = []\nallowed = []\nallow = []\n",
        &[],
    );
    let (code, stdout, _) = subset(&["check", "--config", config.to_str().unwrap()]);
    assert_eq!((code, stdout), (2, String::new()));
}

#[test]
fn report_counts_each_id_with_its_files_and_examples_most_frequent_first() {
    let config = scratch(
        "report",
        "version = 0\ninclude = [\"*.rs\"]\nallowed = [\"item.fn\", \"expr.path\"]\n",
        &[
            ("a.rs", "fn f() { if x {} if y {} }\n"),
            ("b.rs", "fn g() { if z {} loop {} }\n"),
        ],
    );
    let (code, stdout, _) = subset(&["report", "--config", config.to_str().unwrap()]);
    assert_eq!(code, 0);
    assert_eq!(
        stdout,
        "expr.if 3 in 2 files: a.rs:1:10 a.rs:1:18 b.rs:1:10\n\
         expr.loop 1 in 1 file: b.rs:1:18\n"
    );
}

#[test]
fn measure_counts_tokens_and_code_lines_without_comments() {
    let dir = fixtures().join("measure");
    let (code, stdout, _) = subset(&["measure", dir.to_str().unwrap(), "--json"]);
    assert_eq!(code, 0);
    let prefix = format!("{}/", dir.to_str().unwrap());
    assert_eq!(
        stdout.replace(&prefix, ""),
        r#"{
  "files": [
    {
      "lines": 5,
      "path": "one.rs",
      "tokens": 21
    },
    {
      "lines": 1,
      "path": "two.rs",
      "tokens": 4
    }
  ],
  "total": {
    "lines": 6,
    "tokens": 25
  }
}
"#
    );
}

#[test]
fn measure_prints_one_line_per_file_and_a_total() {
    let file = fixtures().join("measure/two.rs");
    let (code, stdout, _) = subset(&["measure", file.to_str().unwrap()]);
    assert_eq!(code, 0);
    assert_eq!(
        stdout.replace(file.to_str().unwrap(), "two.rs"),
        "two.rs: 4 tokens, 1 lines\ntotal: 4 tokens, 1 lines\n"
    );
}

#[test]
fn the_readme_lists_every_id_the_tool_emits() {
    let (code, table, _) = subset(&["ids"]);
    assert_eq!(code, 0);
    let readme = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"))
        .expect("README.md");
    assert!(
        readme.contains(&table),
        "README.md is out of date; paste the output of `subset ids` into it"
    );
}

#[test]
fn dioxus_is_allowed_only_in_the_app_crate() {
    let ui = "use dioxus::prelude::*;\n\nfn view() -> Element {\n    rsx! { div {} }\n}\n";
    assert_eq!(
        check_repo_config(
            "scope-rsx",
            &[
                ("crates/app/src/view.rs", ui),
                ("crates/client/src/view.rs", ui)
            ]
        ),
        "crates/client/src/view.rs:1:22: item.use.glob: a glob import only for `dioxus::prelude::*` in crates/app (proposal 0018)\n\
         crates/client/src/view.rs:4:5: macro.rsx: Dioxus only in crates/app (proposal 0018)\n"
    );
}

#[test]
fn dyn_is_allowed_only_in_the_client_platform_module() {
    let callback = "fn listen(on_open: Closure<dyn FnMut(Event)>) -> Listener {\n    Listener::new(on_open)\n}\n";
    assert_eq!(
        check_repo_config(
            "scope-dyn",
            &[
                ("crates/client/src/platform/rtc.rs", callback),
                ("crates/client/src/link.rs", callback),
            ]
        ),
        "crates/client/src/link.rs:1:28: type.dyn: `dyn` only in crates/client/src/platform/ (proposal 0017)\n"
    );
}

#[test]
fn clap_is_allowed_only_in_a_binary_main_and_in_examples() {
    let cli = "#[derive(Parser)]\nstruct Args {\n    #[arg(long)]\n    verbose: bool,\n}\n";
    assert_eq!(
        check_repo_config(
            "scope-clap",
            &[
                ("crates/daemon/src/main.rs", cli),
                ("crates/client/examples/headless.rs", cli),
                ("crates/daemon/src/config.rs", cli),
            ]
        ),
        "crates/daemon/src/config.rs:1:1: attr.derive.Parser: clap only in a binary's main.rs and in examples/ (proposal 0019)\n\
         crates/daemon/src/config.rs:3:5: attr.arg: clap only in a binary's main.rs and in examples/ (proposal 0019)\n"
    );
}

#[test]
fn serde_derives_are_allowed_only_on_wire_types() {
    let wire = "#[derive(Serialize, Deserialize)]\n#[serde(transparent)]\nstruct Seq(u64);\n";
    assert_eq!(
        check_repo_config(
            "scope-serde",
            &[
                ("crates/proto/src/lib.rs", wire),
                ("crates/agents/src/claude/protocol.rs", wire),
                ("crates/agents/src/claude/session.rs", wire),
            ]
        ),
        "crates/agents/src/claude/session.rs:1:1: attr.derive.Serialize: serde derives only on wire types in crates/proto and the agents' provider protocol modules (proposal 0012)\n\
         crates/agents/src/claude/session.rs:1:1: attr.derive.Deserialize: serde derives only on wire types in crates/proto and the agents' provider protocol modules (proposal 0012)\n\
         crates/agents/src/claude/session.rs:2:1: attr.serde: serde derives only on wire types in crates/proto and the agents' provider protocol modules (proposal 0012)\n"
    );
}

#[test]
fn tokio_main_is_allowed_only_in_a_native_binary() {
    let main = "#[tokio::main]\nasync fn main() {}\n";
    assert_eq!(
        check_repo_config(
            "scope-tokio-main",
            &[
                ("crates/daemon/src/main.rs", main),
                ("crates/agents/src/bin/s1.rs", main),
                ("crates/daemon/src/lib.rs", main),
            ]
        ),
        "crates/daemon/src/lib.rs:1:1: attr.tokio::main: `#[tokio::main]` only in a native binary (proposal 0006)\n"
    );
}

#[test]
fn interior_mutability_is_flagged_in_types_and_expressions() {
    let shared = "fn shared(count: Rc<RefCell<u64>>, flag: Arc<AtomicBool>) -> Mutex<u64> {\n    Mutex::new(RefCell::new(0).into_inner())\n}\n";
    assert_eq!(
        check_repo_config("interior", &[("crates/daemon/src/shared.rs", shared)]),
        "crates/daemon/src/shared.rs:1:21: type.interior_mut.RefCell: interior mutability stays out of the subset (proposal 0008)\n\
         crates/daemon/src/shared.rs:1:46: type.interior_mut.AtomicBool: interior mutability stays out of the subset (proposal 0008)\n\
         crates/daemon/src/shared.rs:1:62: type.interior_mut.Mutex: interior mutability stays out of the subset (proposal 0008)\n\
         crates/daemon/src/shared.rs:2:5: expr.interior_mut.Mutex: interior mutability stays out of the subset (proposal 0008)\n\
         crates/daemon/src/shared.rs:2:16: expr.interior_mut.RefCell: interior mutability stays out of the subset (proposal 0008)\n"
    );
}

#[test]
fn tokio_test_fns_may_use_the_test_macros() {
    let test = "#[tokio::test]\nasync fn adds() {\n    assert_eq!(1, 1);\n}\n";
    assert_eq!(
        check_repo_config("tokio-test", &[("crates/daemon/src/lib.rs", test)]),
        ""
    );
}

/// Checks one rule: `pass` must be clean, and `fail` must give `expected`.
fn rule_case(name: &str, path: &str, pass: &str, fail: &str) -> String {
    let fail_path = path.replace(".rs", "_fail.rs");
    check_repo_config(name, &[(path, pass), (&fail_path, fail)])
}

#[test]
fn only_pub_crate_restricts_visibility() {
    assert_eq!(
        rule_case(
            "vis-crate",
            "crates/daemon/src/a.rs",
            "pub(crate) fn shared() {}\n",
            "pub(super) fn shared() {}\n",
        ),
        "crates/daemon/src/a_fail.rs:1:1: vis.restricted: only `pub(crate)`, never `pub(super)` or `pub(in …)` (proposal 0003)\n"
    );
}

#[test]
fn a_pub_crate_struct_has_no_pub_fields() {
    assert_eq!(
        rule_case(
            "pub-field",
            "crates/daemon/src/a.rs",
            "pub(crate) struct Seen {\n    pub(crate) seq: u64,\n}\n",
            "pub(crate) struct Seen {\n    pub seq: u64,\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:2:5: vis.pub: a field of a `pub(crate)` struct is `pub(crate)`, never `pub` (proposal 0003)\n"
    );
}

#[test]
fn struct_update_takes_a_path_or_a_call() {
    assert_eq!(
        rule_case(
            "struct-update",
            "crates/daemon/src/a.rs",
            "fn rename(host: Host, name: String) -> Host {\n    Host { name, ..host }\n}\n\nfn options(model: String) -> Options {\n    Options { model, ..Options::new() }\n}\n\nfn live(outbox: &Outbox) -> Status {\n    Status { state: LinkState::Live, ..outbox.current() }\n}\n",
            "fn rename(host: Host, name: String) -> Host {\n    Host { name, ..{ host } }\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:2:5: expr.struct.update: the base of `..base` is a path, a constructor call or a method call (proposals 0004, 0036)\n"
    );
}

#[test]
fn unit_type_only_in_library_type_arguments_and_main() {
    assert_eq!(
        rule_case(
            "unit-type",
            "crates/app/src/a.rs",
            "fn ready(done: oneshot::Sender<()>) -> Result<(), Lost> {\n    done.send(()).map_err(Lost::from_unit)\n}\n\nfn close_button(on_close: EventHandler<()>) -> Element {\n    Element::button(on_close)\n}\n",
            "fn wait(done: ()) -> Signal {\n    Signal::new(done)\n}\n",
        ),
        "crates/app/src/a_fail.rs:1:15: type.unit: `()` as a type only in a type argument of a type we don't define, as in `Result<(), E>`, or as `fn main`'s return type (proposals 0005, 0023)\n"
    );
}

#[test]
fn unit_in_our_result_alias_is_still_a_result() {
    assert_eq!(
        check_repo_config(
            "unit-result-alias",
            &[(
                "crates/agents/src/claude/session.rs",
                "pub type Result<T, E = Error> = std::result::Result<T, E>;\n\nfn stop() -> Result<()> {\n    Ok(())\n}\n"
            )]
        ),
        "crates/agents/src/claude/session.rs:1:5: item.type: type alias\n\
         crates/agents/src/claude/session.rs:1:17: generics.type_param: a type parameter only on a fn, with exactly one bound, `Serialize` or `DeserializeOwned`, and no `where` (proposal 0013), or as the unbounded `T` of an `ask` helper (proposal 0026)\n\
         crates/agents/src/claude/session.rs:1:20: generics.type_param: a type parameter only on a fn, with exactly one bound, `Serialize` or `DeserializeOwned`, and no `where` (proposal 0013), or as the unbounded `T` of an `ask` helper (proposal 0026)\n"
    );
}

#[test]
fn a_future_output_of_unit_breaks_no_unit_rule() {
    assert_eq!(
        check_repo_config(
            "unit-future",
            &[(
                "crates/client/src/upgrade.rs",
                "enum Upgrade {\n    Waiting(Pin<Box<dyn Future<Output = ()>>>),\n}\n"
            )]
        ),
        "crates/client/src/upgrade.rs:2:21: type.dyn: `dyn` only in crates/client/src/platform/ (proposal 0017)\n\
         crates/client/src/upgrade.rs:2:32: generics.assoc_binding: associated item binding `Trait<Item = T>`\n"
    );
}

#[test]
fn unit_value_only_for_library_calls() {
    assert_eq!(
        rule_case(
            "unit-value",
            "crates/daemon/src/a.rs",
            "fn ready(tx: oneshot::Sender<()>) -> Result<(), Lost> {\n    tx.send(()).map_err(Lost::from_unit)?;\n    Ok(())\n}\n",
            "enum Done {\n    Now(Signal),\n}\n\nfn finish() -> Done {\n    Done::Now(())\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:6:15: expr.unit: `()` as a value only as an argument or field value of a fn, method or struct we don't define, as in `Ok(())` (proposals 0005, 0023)\n"
    );
}

#[test]
fn async_blocks_only_spawned_or_as_stream_bodies() {
    assert_eq!(
        rule_case(
            "async-block",
            "crates/daemon/src/a.rs",
            "fn start(rx: Inbox) {\n    let _ = tokio::spawn(async move { work(rx).await });\n}\n\nfn events(rx: Inbox) -> Events {\n    stream::unfold(rx, |rx| async move { next(rx).await })\n}\n",
            "async fn attempt() -> Result<u8, Lost> {\n    let result = async { step().await }.await;\n    result\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:2:18: expr.async: an async block only as a spawn argument or the whole body of an `unfold`, `then` or `use_future` closure (proposal 0006)\n"
    );
}

#[test]
fn tokio_main_marks_fn_main() {
    assert_eq!(
        check_repo_config(
            "tokio-main-host",
            &[
                (
                    "crates/daemon/src/main.rs",
                    "#[tokio::main]\nasync fn main() {}\n"
                ),
                (
                    "crates/agents/src/bin/s1.rs",
                    "#[tokio::main]\nasync fn start() {}\n"
                ),
            ]
        ),
        "crates/agents/src/bin/s1.rs:1:1: attr.tokio::main: `#[tokio::main]` only on `fn main` (proposal 0006)\n"
    );
}

const ACTOR: &str = "impl Log {
    async fn run(mut self, mut inbox: Inbox) -> Result<Infallible, Stopped> {
        loop {
            let msg = inbox.recv().await.ok_or(Stopped {})?;
            self.on(msg);
        }
    }

    fn on(&mut self, msg: Msg) {
        self.head = msg.seq;
    }
}

async fn drive(inbox: Inbox) -> Outcome {
    let mut state = Turns::new();
    loop {
        match inbox.recv().await {
            Some(Msg::Done(outcome)) => break outcome,
            Some(msg) => state = state.on(msg),
            None => break Outcome::Lost,
        }
    }
}

impl Turns {
    fn count(&mut self) {
        self.seen = self.seen.saturating_add(1);
    }
}
";

#[test]
fn a_loop_is_the_whole_body_of_an_actor_fn() {
    assert_eq!(
        rule_case(
            "loop-position",
            "crates/daemon/src/a.rs",
            ACTOR,
            "async fn run(inbox: Inbox) {\n    let host = Host::new();\n    loop {\n        host.on(inbox.recv().await);\n    }\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:3:5: expr.loop: loop must be the whole body of an actor run fn, after at most one `let mut` state (proposal 0007)\n"
    );
}

#[test]
fn a_loop_turn_takes_one_input_and_handles_it_once() {
    assert_eq!(
        rule_case(
            "loop-turn",
            "crates/daemon/src/a.rs",
            ACTOR,
            "async fn run(inbox: Inbox) -> Result<Infallible, Stopped> {\n    loop {\n        let msg = inbox.recv().await.ok_or(Stopped {})?;\n        log(&msg);\n        handle(msg);\n    }\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:2:5: expr.loop: a loop turn takes one input (`select!`, a `match` of `select!`s, or one awaited call) and handles it with one call or `match` (proposal 0007)\n"
    );
}

#[test]
fn break_carries_a_value_out_of_an_actor_loop() {
    assert_eq!(
        rule_case(
            "break",
            "crates/daemon/src/a.rs",
            ACTOR,
            "async fn drive(inbox: Inbox) -> Outcome {\n    loop {\n        match inbox.recv().await {\n            Some(msg) => handle(msg),\n            None => break,\n        }\n    }\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:5:21: expr.break: `break` only with a value, out of an actor loop (proposal 0007)\n"
    );
}

#[test]
fn mut_self_only_on_an_actor_loop_fn() {
    assert_eq!(
        rule_case(
            "mut-self",
            "crates/daemon/src/a.rs",
            ACTOR,
            "impl Log {\n    fn with_head(mut self, head: HostSeq) -> Log {\n        self.head = head;\n        self\n    }\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:2:18: item.fn.receiver.mut: `mut self` only on a fn whose body is an actor loop (proposal 0008)\n"
    );
}

#[test]
fn ref_mut_self_only_on_loop_state_methods() {
    assert_eq!(
        rule_case(
            "ref-mut-self",
            "crates/daemon/src/a.rs",
            ACTOR,
            "impl Host {\n    fn rename(&mut self, name: String) {\n        self.name = name;\n    }\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:2:16: item.fn.receiver.mut: `&mut self` only on methods of an actor's loop state, of a type of ours named in one of its field types or in a variant of an enum it owns (proposal 0008), or of a type in `endpoints` (proposal 0035)\n"
    );
}

const OWNED: &str = "struct Reader {
    frames: FrameBuffer,
    turns: HashMap<ThreadId, Turn>,
    phase: Phase,
}

enum Phase {
    Idle,
    Live(Live),
}

struct Live {
    conn: Conn,
}

impl Live {
    fn arm(&mut self) {
        self.conn = Conn {};
    }
}

struct FrameBuffer {
    pending: Vec<u8>,
}

struct Turn {
    outcome: Option<Outcome>,
}

impl Reader {
    async fn run(mut self, mut inbox: Inbox) -> Result<Infallible, Stopped> {
        loop {
            let msg = inbox.recv().await.ok_or(Stopped {})?;
            self.on(msg);
        }
    }

    fn on(&mut self, msg: Msg) {
        self.frames.push(msg.bytes);
    }
}

impl FrameBuffer {
    fn push(&mut self, bytes: Vec<u8>) {
        self.pending.extend(bytes);
    }
}

impl Turn {
    fn finish(&mut self, outcome: Outcome) {
        self.outcome = Some(outcome);
    }
}
";

#[test]
fn ref_mut_self_reaches_a_value_that_loop_state_owns() {
    assert_eq!(
        rule_case(
            "ref-mut-owned",
            "crates/daemon/src/a.rs",
            OWNED,
            "struct Session {\n    turn: Turn,\n}\n\nimpl Session {\n    fn finish(&mut self, outcome: Outcome) {\n        self.turn.finish(outcome);\n    }\n}\n\nfn apply(turn: &mut Turn, outcome: Outcome) {\n    turn.finish(outcome);\n}\n\nstruct Conn {}\n\nimpl Conn {\n    fn close(&mut self) {}\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:6:16: item.fn.receiver.mut: `&mut self` only on methods of an actor's loop state, of a type of ours named in one of its field types or in a variant of an enum it owns (proposal 0008), or of a type in `endpoints` (proposal 0035)\n\
         crates/daemon/src/a_fail.rs:11:17: type.reference.mut: no `&mut` of our own types other than `&mut self` (proposals 0008, 0023)\n\
         crates/daemon/src/a_fail.rs:18:15: item.fn.receiver.mut: `&mut self` only on methods of an actor's loop state, of a type of ours named in one of its field types or in a variant of an enum it owns (proposal 0008), or of a type in `endpoints` (proposal 0035)\n"
    );
}

#[test]
fn ref_mut_self_on_a_listed_endpoint_anywhere() {
    assert_eq!(
        rule_case(
            "ref-mut-endpoint",
            "crates/client/src/a.rs",
            "pub(crate) enum SendHalf {\n    Iroh(SendStream),\n}\n\nimpl SendHalf {\n    pub(crate) async fn write(&mut self, frame: &[u8]) -> Result<(), WireError> {\n        match self {\n            Self::Iroh(send) => send.write_all(frame).await?,\n        }\n        Ok(())\n    }\n}\n\nasync fn rpc(transport: Transport, frame: Vec<u8>) -> Result<(), WireError> {\n    let mut send = transport.open().await?;\n    send.write(&frame).await?;\n    Ok(())\n}\n",
            "struct Transcript {\n    items: Vec<Item>,\n}\n\nimpl Transcript {\n    fn push(&mut self, item: Item) {\n        self.items.push(item);\n    }\n}\n",
        ),
        "crates/client/src/a_fail.rs:6:14: item.fn.receiver.mut: `&mut self` only on methods of an actor's loop state, of a type of ours named in one of its field types or in a variant of an enum it owns (proposal 0008), or of a type in `endpoints` (proposal 0035)\n"
    );
}

#[test]
fn let_mut_is_loop_state_or_lent_as_mut() {
    assert_eq!(
        rule_case(
            "let-mut",
            "crates/daemon/src/a.rs",
            "async fn body(mut reader: Reader) -> Result<Vec<u8>, Lost> {\n    let mut buf = Vec::new();\n    reader.read_to_end(&mut buf).await.map_err(Lost::Read)?;\n    Ok(buf)\n}\n",
            "fn count(n: u64) -> Option<u64> {\n    let mut total = n;\n    total = total.checked_add(1)?;\n    Some(total)\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:2:5: stmt.let.mut: `let mut` only for a loop's state just before the loop, or for a value never reassigned and only lent as `&mut` (proposal 0008)\n\
         crates/daemon/src/a_fail.rs:3:11: expr.assign: assignment only to `self.field` in a `&mut self` method, or to the loop's state binding (proposal 0008)\n"
    );
}

#[test]
fn a_mut_binding_is_never_reassigned() {
    assert_eq!(
        rule_case(
            "pat-mut",
            "crates/daemon/src/a.rs",
            "fn drain(mut rx: Inbox) {\n    let _ = rx.try_recv();\n}\n",
            "fn reset(mut n: u64) -> u64 {\n    n = 0;\n    n\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:1:10: pat.ident.mut: a `mut` binding is never reassigned, only lent as `&mut` (proposal 0008)\n\
         crates/daemon/src/a_fail.rs:2:7: expr.assign: assignment only to `self.field` in a `&mut self` method, or to the loop's state binding (proposal 0008)\n"
    );
}

#[test]
fn assignment_only_to_self_fields_and_loop_state() {
    assert_eq!(
        rule_case(
            "assign",
            "crates/daemon/src/a.rs",
            ACTOR,
            "impl Log {\n    fn head(&self, seen: Seen) {\n        seen.seq = self.head;\n    }\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:3:18: expr.assign: assignment only to `self.field` in a `&mut self` method, or to the loop's state binding (proposal 0008)\n"
    );
}

#[test]
fn no_mut_reference_to_our_own_types() {
    assert_eq!(
        rule_case(
            "ref-mut-ours",
            "crates/daemon/src/a.rs",
            "struct Log {\n    head: HostSeq,\n}\n\nasync fn fill(reader: Reader, buf: &mut Vec<u8>) -> Result<usize, Lost> {\n    reader.read(buf).await.map_err(Lost::Read)\n}\n",
            "fn bump(log: &mut Log) {\n    drop(log)\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:1:15: type.reference.mut: no `&mut` of our own types other than `&mut self` (proposals 0008, 0023)\n"
    );
}

#[test]
fn discard_with_let_underscore() {
    assert_eq!(
        rule_case(
            "discard",
            "crates/daemon/src/a.rs",
            "fn answer(reply: oneshot::Sender<HostSeq>, head: HostSeq, guard: Lease) {\n    let _ = reply.send(head);\n    drop(guard);\n}\n",
            "fn answer(reply: oneshot::Sender<HostSeq>, head: HostSeq, lost: oneshot::Sender<HostSeq>) {\n    drop(reply.send(head));\n    lost.send(head).ok();\n    std::mem::drop(tokio::spawn(work()));\n}\n\nfn sent(reply: Reply) -> Result<(), Lost> {\n    reply.send().map(drop)\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:2:5: expr.call: `drop(..)` only of a named value that owns a resource released early; discard a value with `let _ = value;` (proposal 0025)\n\
         crates/daemon/src/a_fail.rs:3:21: expr.method_call: `.ok()` is not a discard; write `let _ = value;` (proposal 0025)\n\
         crates/daemon/src/a_fail.rs:4:5: expr.call: `drop(..)` only of a named value that owns a resource released early; discard a value with `let _ = value;` (proposal 0025)\n\
         crates/daemon/src/a_fail.rs:8:22: expr.path: `drop` passed as a function, as in `.map(drop)`, is a discard; write `let _ = value;` (proposal 0025)\n"
    );
}

#[test]
fn boolean_expressions_only_in_predicate_closures() {
    assert_eq!(
        rule_case(
            "predicate",
            "crates/daemon/src/a.rs",
            "fn finished(events: &[Event]) -> usize {\n    events.iter().filter(|e| matches!(e.kind, Kind::TurnFinished)).count()\n}\n\nfn seen(events: &[Event], seq: HostSeq) -> Option<&Event> {\n    events.iter().find(|e| e.seq == seq)\n}\n\nfn first_live(events: &[Event], seq: HostSeq) -> Option<usize> {\n    events.iter().position(|e| e.seq >= seq && !matches!(e.kind, Kind::Replayed))\n}\n",
            "fn same(a: HostSeq, b: HostSeq) -> Same {\n    let same = a == b;\n    Same::from(same)\n}\n\nfn done(next: Option<Event>) {\n    if let Some(event) = next && matches!(event.kind, Kind::TurnFinished) {\n        finish(event);\n    }\n}\n\nfn label(next: Option<Event>) -> Label {\n    match next {\n        Some(e) if matches!(e.kind, Kind::TurnFinished) => Label::Done,\n        _ => Label::Busy,\n    }\n}\n\nfn all(events: &[Event]) -> usize {\n    events.iter().filter(|_| true).count()\n}\n\nfn counted(events: &[Event]) -> usize {\n    events.iter().filter_map(|e| match e.kind {\n        Kind::TurnFinished => Some(()),\n        _ => None,\n    }).count()\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:2:18: expr.binary.eq: a boolean expression (`matches!`, a comparison, `&&`, `||`, `!`) only as the body of a closure passed straight to a method in `predicate_adapters` (proposals 0028, 0037)\n\
         crates/daemon/src/a_fail.rs:7:5: expr.if_let: `if let` with an `else`, or an `if` with a let-chain\n\
         crates/daemon/src/a_fail.rs:7:8: expr.let: `let` inside a condition chain\n\
         crates/daemon/src/a_fail.rs:7:31: expr.binary.and: a boolean expression (`matches!`, a comparison, `&&`, `||`, `!`) only as the body of a closure passed straight to a method in `predicate_adapters` (proposals 0028, 0037)\n\
         crates/daemon/src/a_fail.rs:7:34: macro.matches: a boolean expression (`matches!`, a comparison, `&&`, `||`, `!`) only as the body of a closure passed straight to a method in `predicate_adapters` (proposals 0028, 0037)\n\
         crates/daemon/src/a_fail.rs:14:17: expr.match.guard: `if` guard on a match arm\n\
         crates/daemon/src/a_fail.rs:14:20: macro.matches: a boolean expression (`matches!`, a comparison, `&&`, `||`, `!`) only as the body of a closure passed straight to a method in `predicate_adapters` (proposals 0028, 0037)\n\
         crates/daemon/src/a_fail.rs:20:30: expr.lit.bool: `true` / `false` only as an argument or field value of a fn, method or struct we don't define (proposal 0023)\n\
         crates/daemon/src/a_fail.rs:27:8: expr.method_call: count matching items with `filter(|x| matches!(x, ..)).count()`, not `filter_map(..).count()` (proposal 0028)\n"
    );
}

#[test]
fn a_closure_goes_only_to_a_listed_method() {
    assert_eq!(
        rule_case(
            "closure-method",
            "crates/daemon/src/a.rs",
            "fn last_seen(events: &[Event], seq: HostSeq) -> Option<&Event> {\n    events.iter().rfind(|e| e.seq == seq)\n}\n\nasync fn closed(mut state: watch::Receiver<ChannelState>) -> Result<(), Lost> {\n    state.wait_for(|s| s == &ChannelState::Closed).await.map_err(Lost::from_watch)?;\n    Ok(())\n}\n\nfn label(name: Option<Name>) -> Option<Label> {\n    name.map(|n| n.label())\n}\n",
            "async fn arm(link: Link, state: LinkState) {\n    link.update(|s| s.state = state).await;\n}\n\nasync fn connecting(mut state: watch::Receiver<ChannelState>) {\n    let _ = state.wait_until(|s| s != &ChannelState::Connecting).await;\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:2:17: expr.closure.adapter: a closure passed to a method only to a method in `closure_methods` or `predicate_adapters`: the iterator, `Option` and `Result` methods and the library methods the config lists (proposals 0006, 0026, 0028, 0037)\n\
         crates/daemon/src/a_fail.rs:2:29: expr.assign: assignment only to `self.field` in a `&mut self` method, or to the loop's state binding (proposal 0008)\n\
         crates/daemon/src/a_fail.rs:6:30: expr.closure.adapter: a closure passed to a method only to a method in `closure_methods` or `predicate_adapters`: the iterator, `Option` and `Result` methods and the library methods the config lists (proposals 0006, 0026, 0028, 0037)\n\
         crates/daemon/src/a_fail.rs:6:36: expr.binary.ne: a boolean expression (`matches!`, a comparison, `&&`, `||`, `!`) only as the body of a closure passed straight to a method in `predicate_adapters` (proposals 0028, 0037)\n"
    );
}

#[test]
fn trait_impls_only_for_library_traits() {
    assert_eq!(
        rule_case(
            "impl-trait",
            "crates/daemon/src/a.rs",
            "impl Drop for Guard {\n    fn drop(&mut self) {\n        let _ = self.child.start_kill();\n    }\n}\n",
            "impl Display for Guard {\n    fn fmt(&self, f: &mut Formatter) -> fmt::Result {\n        f.write_str(\"guard\")\n    }\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:1:1: item.impl.trait: trait impls only for `Drop`, `Debug`, `Serialize`, `Deserialize` and `ProtocolHandler` (proposal 0009)\n"
    );
}

#[test]
fn lifetimes_only_in_library_trait_impls() {
    assert_eq!(
        rule_case(
            "lifetimes",
            "crates/proto/src/a.rs",
            "impl<'de> Deserialize<'de> for Seq {\n    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Seq, D::Error> {\n        u64::deserialize(d).map(Seq)\n    }\n}\n",
            "fn first<'a>(name: &'a str) -> &'a str {\n    name\n}\n",
        ),
        "crates/proto/src/a_fail.rs:1:10: generics.lifetime: lifetime parameters only in a library trait impl (proposal 0009)\n\
         crates/proto/src/a_fail.rs:1:21: type.lifetime: lifetimes only in a library trait impl, where its signature demands them (proposal 0009)\n\
         crates/proto/src/a_fail.rs:1:33: type.lifetime: lifetimes only in a library trait impl, where its signature demands them (proposal 0009)\n"
    );
}

#[test]
fn thiserror_only_on_error_types() {
    assert_eq!(
        rule_case(
            "thiserror-host",
            "crates/daemon/src/a.rs",
            "#[derive(Debug, thiserror::Error)]\nenum Lost {\n    #[error(\"the peer closed\")]\n    Closed,\n}\n\nfn wait() -> Result<u8, Lost> {\n    Err(Lost::Closed)\n}\n",
            "#[derive(Debug, thiserror::Error)]\nenum Outcome {\n    #[error(\"done\")]\n    Done,\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:1:1: attr.derive: `thiserror::Error` only on an enum or named struct that is an error type, named `…Error` or used as a `Result`'s error (proposal 0010)\n"
    );
}

#[test]
fn ord_only_on_a_newtype_compared_as_a_whole() {
    assert_eq!(
        rule_case(
            "ord",
            "crates/daemon/src/a.rs",
            "#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]\npub struct Seq(u64);\n\nfn fresh(seen: Seq, head: Seq) -> Freshness {\n    match seen.cmp(&head) {\n        Ordering::Less => Freshness::Behind,\n        Ordering::Equal | Ordering::Greater => Freshness::Current,\n    }\n}\n",
            "#[derive(PartialEq, Eq, PartialOrd, Ord)]\nenum Phase {\n    Early,\n    Late,\n}\n\n#[derive(PartialEq, Eq, PartialOrd, Ord)]\npub struct Mark(u64);\n\nimpl Mark {\n    fn after(self, other: Mark) -> Ordering {\n        self.0.cmp(&other.0)\n    }\n}\n\nfn order(a: Mark, b: &Mark, c: Count) -> Ordering {\n    a.0.cmp(&b.0).then(c.0.cmp(&a.0))\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:1:1: attr.derive: `PartialOrd` and `Ord` only on a newtype struct whose order means something (proposal 0024)\n\
         crates/daemon/src/a_fail.rs:12:16: expr.method_call: compare an ordered newtype as a whole, `a.cmp(&b)`, never through `.0` (proposal 0024)\n\
         crates/daemon/src/a_fail.rs:17:9: expr.method_call: compare an ordered newtype as a whole, `a.cmp(&b)`, never through `.0` (proposal 0024)\n"
    );
}

#[test]
fn each_error_variant_has_one_message() {
    assert_eq!(
        rule_case(
            "error-message",
            "crates/daemon/src/a.rs",
            "#[derive(Debug, thiserror::Error)]\nenum LoadError {\n    #[error(\"reading {0}\")]\n    Read(String),\n}\n",
            "#[derive(Debug, thiserror::Error)]\nenum LoadError {\n    #[error(transparent)]\n    Io(std::io::Error),\n    Parse(String),\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:3:5: attr.error: each error variant carries one `#[error(\"…\")]` message, never `transparent` (proposal 0010)\n\
         crates/daemon/src/a_fail.rs:5:5: attr.error: each error variant carries one `#[error(\"…\")]` message, never `transparent` (proposal 0010)\n"
    );
}

#[test]
fn from_only_on_the_one_variant_holding_a_source_type() {
    assert_eq!(
        rule_case(
            "from-once",
            "crates/daemon/src/a.rs",
            "#[derive(Debug, thiserror::Error)]\nenum LoadError {\n    #[error(\"reading: {0}\")]\n    Read(#[from] std::io::Error),\n    #[error(\"parsing: {0}\")]\n    Parse(#[from] toml::de::Error),\n    #[error(\"decoding: {0}\")]\n    Decode(#[from] DecodeError),\n}\n\n#[derive(Debug, thiserror::Error)]\nenum DecodeError {\n    #[error(\"empty line\")]\n    Empty,\n}\n",
            "#[derive(Debug, thiserror::Error)]\nenum LoadError {\n    #[error(\"reading: {0}\")]\n    Read(#[from] std::io::Error),\n    #[error(\"writing: {0}\")]\n    Write(#[from] std::io::Error),\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:4:10: attr.from: `#[from]` only on the one variant of an enum holding a given error type (proposal 0011)\n\
         crates/daemon/src/a_fail.rs:6:11: attr.from: `#[from]` only on the one variant of an enum holding a given error type (proposal 0011)\n"
    );
}

#[test]
fn serde_attributes_take_only_the_listed_keys() {
    assert_eq!(
        rule_case(
            "serde-keys",
            "crates/proto/src/a.rs",
            "#[derive(Serialize, Deserialize)]\n#[serde(tag = \"type\", rename_all = \"snake_case\")]\nenum Frame {\n    Hello { host: String },\n}\n",
            "#[derive(Serialize, Deserialize)]\nstruct Hello {\n    #[serde(default)]\n    host: String,\n}\n",
        ),
        "crates/proto/src/a_fail.rs:3:5: attr.serde: `#[serde]` only with `transparent`, `tag`, `content`, `rename_all` and `rename` (proposal 0012)\n"
    );
}

#[test]
fn type_params_only_on_codec_fns() {
    assert_eq!(
        rule_case(
            "type-param",
            "crates/proto/src/a.rs",
            "pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, Error> {\n    serde_json::to_vec(value)\n}\n",
            "pub struct Tagged<T> {\n    value: T,\n}\n\nfn copy<T: Clone>(value: &T) -> T {\n    value.clone()\n}\n",
        ),
        "crates/proto/src/a_fail.rs:1:19: generics.type_param: a type parameter only on a fn, with exactly one bound, `Serialize` or `DeserializeOwned`, and no `where` (proposal 0013), or as the unbounded `T` of an `ask` helper (proposal 0026)\n\
         crates/proto/src/a_fail.rs:5:9: generics.type_param: a type parameter only on a fn, with exactly one bound, `Serialize` or `DeserializeOwned`, and no `where` (proposal 0013), or as the unbounded `T` of an `ask` helper (proposal 0026)\n"
    );
}

const ASK: &str = "impl Handle {
    pub async fn head(&self) -> Result<HostSeq, Stopped> {
        self.ask(|reply| Msg::Head { reply }).await
    }

    async fn ask<T>(&self, build: impl FnOnce(oneshot::Sender<T>) -> Msg) -> Result<T, Stopped> {
        let (reply, answer) = oneshot::channel();
        self.commands.send(build(reply)).await.map_err(|_| Stopped {})?;
        answer.await.map_err(|_| Stopped {})
    }
}

async fn stop(commands: &mpsc::Sender<Msg>) -> Result<Summary, Stopped> {
    ask(commands, |reply| Msg::Stop { reply }).await
}

async fn ask<T>(
    commands: &mpsc::Sender<Msg>,
    build: impl FnOnce(oneshot::Sender<T>) -> Msg,
) -> Result<T, Stopped> {
    let (reply, answer) = oneshot::channel();
    commands.send(build(reply)).await.map_err(|_| Stopped {})?;
    answer.await.map_err(|_| Stopped {})
}
";

#[test]
fn impl_trait_arguments_only_for_the_ask_helper() {
    assert_eq!(
        rule_case(
            "ask",
            "crates/daemon/src/a.rs",
            ASK,
            "impl Handle {\n    pub async fn ask<T>(&self, build: impl FnOnce(oneshot::Sender<T>) -> Msg) -> Result<T, Stopped> {\n        self.send(build).await\n    }\n}\n\nasync fn request<T: Clone>(build: impl FnOnce(Sender<T>) -> Msg) -> T {\n    send(build).await\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:2:22: generics.type_param: a type parameter only on a fn, with exactly one bound, `Serialize` or `DeserializeOwned`, and no `where` (proposal 0013), or as the unbounded `T` of an `ask` helper (proposal 0026)\n\
         crates/daemon/src/a_fail.rs:2:39: type.impl_trait.arg: `impl Trait` as an argument only as `build: impl FnOnce(oneshot::Sender<T>) -> Msg`, the last parameter of a private `ask<T>` helper (proposal 0026)\n\
         crates/daemon/src/a_fail.rs:7:18: generics.type_param: a type parameter only on a fn, with exactly one bound, `Serialize` or `DeserializeOwned`, and no `where` (proposal 0013), or as the unbounded `T` of an `ask` helper (proposal 0026)\n\
         crates/daemon/src/a_fail.rs:7:35: type.impl_trait.arg: `impl Trait` as an argument only as `build: impl FnOnce(oneshot::Sender<T>) -> Msg`, the last parameter of a private `ask<T>` helper (proposal 0026)\n"
    );
}

#[test]
fn slices_only_as_parameters_and_consts() {
    assert_eq!(
        rule_case(
            "slice",
            "crates/daemon/src/a.rs",
            "const ALPN: &[u8] = b\"conductor/1\";\n\nfn total(sizes: &[u64]) -> Option<u64> {\n    sizes.iter().copied().try_fold(0, |sum, size| sum.checked_add(size))\n}\n",
            "struct Packet {\n    bytes: Box<[u8]>,\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:2:16: type.slice: `&[T]` only as a parameter type or a `const` type; a `const` list is `const NAME: &[T] = &[..]` (proposal 0014)\n"
    );
}

#[test]
fn array_types_only_for_bytes() {
    assert_eq!(
        rule_case(
            "array-type",
            "crates/daemon/src/a.rs",
            "struct Key([u8; 32]);\n\nconst MAGIC: [u8; 4] = [0x63, 0x6f, 0x6e, 0x64];\n",
            "struct Pair([u64; 2]);\n\nconst BACKOFF: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(5)];\n",
        ),
        "crates/daemon/src/a_fail.rs:1:13: type.array: an array type only for bytes whose size is part of a format or API (`[u8; N]`); a `const` list is `&[T]` (proposals 0014, 0015)\n\
         crates/daemon/src/a_fail.rs:3:16: type.array: an array type only for bytes whose size is part of a format or API (`[u8; N]`); a `const` list is `&[T]` (proposals 0014, 0015)\n"
    );
}

#[test]
fn array_literals_only_as_call_arguments_and_consts() {
    assert_eq!(
        rule_case(
            "array-value",
            "crates/daemon/src/a.rs",
            "fn git(cmd: Command) -> Command {\n    cmd.args([\"init\", \"-q\"])\n}\n\nfn hosts(a: Host, b: Host) -> Vec<Host> {\n    Vec::from([a, b])\n}\n",
            "fn pair(a: Host, b: Host) -> Vec<Host> {\n    let hosts = [a, b];\n    Vec::from(hosts)\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:2:17: expr.array: an array literal only passed straight to a call, as in `f([a, b])` or `f(&[a, b])`, or as a `const` value (proposal 0015)\n"
    );
}

#[test]
fn an_array_literal_behind_a_reference_keeps_its_place() {
    assert_eq!(
        rule_case(
            "array-ref",
            "crates/daemon/src/a.rs",
            "const STUN_SERVERS: &[&str] = &[\"stun:a:3478\", \"stun:b:3478\"];\n\nfn replay(name: &str) -> Recording {\n    recording(name, &[Frame::Hello, Frame::Bye])\n}\n",
            "fn pair(a: Host, b: Host) -> Pair {\n    let hosts = &[a, b];\n    Pair::new(hosts)\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:2:18: expr.array: an array literal only passed straight to a call, as in `f([a, b])` or `f(&[a, b])`, or as a `const` value (proposal 0015)\n"
    );
}

#[test]
fn array_repeats_only_of_a_literal() {
    assert_eq!(
        rule_case(
            "repeat",
            "crates/daemon/src/a.rs",
            "fn buffer() -> [u8; 16] {\n    [0u8; 16]\n}\n",
            "fn buffer(fill: u8) -> [u8; 16] {\n    [fill; 16]\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:2:5: expr.repeat: an array repeat only of a literal, as in `[0u8; N]` (proposal 0015)\n"
    );
}

const CFG_PAIR_SRC: &str = "#[cfg(unix)]\nfn socket() -> Socket {\n    Socket::Unix\n}\n\n#[cfg(not(unix))]\nfn socket() -> Socket {\n    Socket::Tcp\n}\n";

#[test]
fn cfg_takes_only_the_platform_predicates() {
    assert_eq!(
        rule_case(
            "cfg-predicate",
            "crates/daemon/src/a.rs",
            CFG_PAIR_SRC,
            "#[cfg(feature = \"tls\")]\nmod tls;\n",
        ),
        "crates/daemon/src/a_fail.rs:1:1: attr.cfg: `cfg` only with `target_family = \"wasm\"`, `unix` or their `not` (proposal 0016)\n"
    );
}

#[test]
fn cfg_sits_on_items_or_atop_a_test_file() {
    assert_eq!(
        check_repo_config(
            "cfg-host",
            &[
                (
                    "crates/agents/tests/replay.rs",
                    "#![cfg(unix)]\n\nfn replay() {}\n"
                ),
                (
                    "crates/agents/src/wire.rs",
                    "fn send(frame: Frame) -> Result<(), Lost> {\n    #[cfg(unix)]\n    let socket = frame.socket();\n    Ok(())\n}\n"
                ),
            ]
        ),
        "crates/agents/src/wire.rs:2:5: attr.cfg: `cfg` only on a mod, use, fn or impl item, or as `#![cfg]` atop a test file (proposal 0016)\n"
    );
}

#[test]
fn a_cfg_item_has_its_counterpart_in_the_same_file() {
    assert_eq!(
        rule_case(
            "cfg-pair",
            "crates/daemon/src/a.rs",
            CFG_PAIR_SRC,
            "#[cfg(unix)]\nfn socket() -> Socket {\n    Socket::Unix\n}\n\n#[cfg(not(unix))]\nfn listen() -> Socket {\n    Socket::Tcp\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:1:1: attr.cfg: a `cfg` fn, impl or use needs its counterpart for the other platform in the same file (proposal 0016)\n\
         crates/daemon/src/a_fail.rs:6:1: attr.cfg: a `cfg` fn, impl or use needs its counterpart for the other platform in the same file (proposal 0016)\n"
    );
}

#[test]
fn dyn_only_as_a_wasm_closure() {
    assert_eq!(
        rule_case(
            "dyn",
            "crates/client/src/platform/a.rs",
            "struct Listeners {\n    on_open: Closure<dyn FnMut(Event)>,\n}\n",
            "struct Listeners {\n    on_open: Box<dyn Fn(Event)>,\n}\n",
        ),
        "crates/client/src/platform/a_fail.rs:2:18: type.dyn: `dyn` only as `Closure<dyn FnMut(..)>` (proposal 0017)\n"
    );
}

#[test]
fn closures_outside_method_calls_only_for_callbacks_streams_and_hooks() {
    assert_eq!(
        rule_case(
            "closure",
            "crates/client/src/platform/a.rs",
            "fn on_open(tx: Sender) -> Callback {\n    Closure::new(move |event| {\n        let _ = tx.send(Msg::Open(event));\n    })\n}\n",
            "fn on_open(tx: Sender) -> Callback {\n    Callback::wrap(move |event| {\n        let _ = tx.send(Msg::Open(event));\n    })\n}\n",
        ),
        "crates/client/src/platform/a_fail.rs:2:20: expr.closure: a closure outside a method call only as the argument of `Closure::new`, `unfold`, `ask` or a Dioxus hook (proposals 0006, 0017, 0018, 0026)\n"
    );
}

#[test]
fn glob_imports_only_for_the_dioxus_prelude() {
    assert_eq!(
        rule_case(
            "glob",
            "crates/app/src/a.rs",
            "use dioxus::prelude::*;\n",
            "use std::collections::*;\n",
        ),
        "crates/app/src/a_fail.rs:1:23: item.use.glob: a glob import only for `dioxus::prelude::*` (proposal 0018)\n"
    );
}

#[test]
fn allow_only_for_useless_format_at_the_app_root() {
    assert_eq!(
        check_repo_config(
            "allow",
            &[(
                "crates/app/src/main.rs",
                "#![allow(clippy::useless_format)]\n#![allow(dead_code)]\n\nfn main() {}\n"
            )]
        ),
        "crates/app/src/main.rs:2:1: attr.allow: only `#![allow(clippy::useless_format)]` at the crate root (proposal 0018)\n"
    );
}

#[test]
fn bool_only_in_library_type_arguments_and_clap_flags() {
    assert_eq!(
        check_repo_config(
            "clap-bool",
            &[
                (
                    "crates/daemon/src/main.rs",
                    "#[derive(Parser)]\nstruct Args {\n    #[arg(long)]\n    verbose: bool,\n}\n"
                ),
                (
                    "crates/daemon/src/live.rs",
                    "fn watch_live(live: watch::Receiver<bool>) -> Watcher {\n    Watcher::new(live)\n}\n"
                ),
                (
                    "crates/daemon/src/session.rs",
                    "struct Session {\n    live: bool,\n}\n\nfn is_live(session: &Session) -> bool {\n    session.live\n}\n"
                ),
                (
                    "crates/client/examples/headless.rs",
                    "fn verbosity(verbose: bool) -> Verbosity {\n    Verbosity::from_flag(verbose)\n}\n"
                ),
            ]
        ),
        "crates/client/examples/headless.rs:1:23: type.bool: `bool` only in a type argument of a type we don't define, or as a flag field of a clap `Parser` struct (proposals 0019, 0023)\n\
         crates/daemon/src/session.rs:2:11: type.bool: `bool` only in a type argument of a type we don't define, or as a flag field of a clap `Parser` struct (proposals 0019, 0023)\n\
         crates/daemon/src/session.rs:5:34: type.bool: `bool` only in a type argument of a type we don't define, or as a flag field of a clap `Parser` struct (proposals 0019, 0023)\n"
    );
}

#[test]
fn named_choices_are_a_value_enum() {
    assert_eq!(
        check_repo_config(
            "value-parser",
            &[
                (
                    "crates/daemon/src/main.rs",
                    "#[derive(Parser)]\nstruct Args {\n    #[arg(long, value_enum)]\n    provider: Provider,\n}\n"
                ),
                (
                    "crates/client/examples/headless.rs",
                    "#[derive(Parser)]\nstruct Args {\n    #[arg(long, value_parser = [\"claude\", \"codex\"])]\n    provider: String,\n}\n"
                ),
            ]
        ),
        "crates/client/examples/headless.rs:3:5: attr.arg: a choice between named values is a `ValueEnum`, not `value_parser = [...]` (proposal 0019)\n"
    );
}

#[test]
fn tokio_test_takes_no_arguments_and_marks_an_async_fn() {
    assert_eq!(
        rule_case(
            "tokio-test-args",
            "crates/daemon/tests/a.rs",
            "#[tokio::test]\nasync fn replays() {\n    assert_eq!(replay().await, Some(3));\n}\n",
            "#[tokio::test(flavor = \"multi_thread\")]\nasync fn replays() {\n    assert_eq!(replay().await, Some(3));\n}\n",
        ),
        "crates/daemon/tests/a_fail.rs:1:1: attr.tokio::test: `#[tokio::test]` takes no arguments and marks an `async fn` (proposal 0020)\n"
    );
}

#[test]
fn bool_literals_only_for_library_apis() {
    assert_eq!(
        rule_case(
            "bool-lit",
            "crates/daemon/src/a.rs",
            "fn open(path: &Path) -> Result<File, Lost> {\n    OpenOptions::new().create(true).open(path).map_err(Lost::Open)\n}\n\nfn init() -> Capabilities {\n    Capabilities { experimental_api: Some(true) }\n}\n\nfn idle() -> Watch {\n    Watch::new(watch::channel(false))\n}\n",
            "enum Live {\n    On,\n    Off,\n}\n\nstruct Flags {\n    live: Live,\n}\n\nimpl Flags {\n    fn on(&self) -> Live {\n        self.live(true)\n    }\n}\n\nfn off() -> Live {\n    Live::from_flag(false)\n}\n",
        ),
        "crates/daemon/src/a_fail.rs:12:19: expr.lit.bool: `true` / `false` only as an argument or field value of a fn, method or struct we don't define (proposal 0023)\n\
         crates/daemon/src/a_fail.rs:17:21: expr.lit.bool: `true` / `false` only as an argument or field value of a fn, method or struct we don't define (proposal 0023)\n"
    );
}

#[test]
fn env_only_for_cargo_build_constants() {
    assert_eq!(
        rule_case(
            "env",
            "crates/daemon/src/a.rs",
            "const VERSION: &str = env!(\"CARGO_PKG_VERSION\");\n",
            "const HOME: &str = env!(\"HOME\");\n",
        ),
        "crates/daemon/src/a_fail.rs:1:20: macro.env: `env!` only for `CARGO_PKG_VERSION`, `CARGO_PKG_NAME` and `CARGO_MANIFEST_DIR` (proposal 0022)\n"
    );
}

#[test]
fn the_readme_lists_every_shape_rule() {
    let (code, table, _) = subset(&["rules"]);
    assert_eq!(code, 0);
    let readme = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"))
        .expect("README.md");
    assert!(
        readme.contains(&table),
        "README.md is out of date; paste the output of `subset rules` into it"
    );
}

/// Each rust block of `docs/subset/examples.md`, at the path its first-line
/// comment names or at `crates/examples/src/<n>.rs`.
fn example_files() -> Vec<(String, String)> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/subset/examples.md");
    let text = std::fs::read_to_string(path).expect("examples.md");
    text.split("```rust\n")
        .skip(1)
        .enumerate()
        .map(|(n, rest)| {
            let code = rest.split("```").next().unwrap().to_string();
            let named = code
                .lines()
                .next()
                .and_then(|line| line.strip_prefix("// crates/"))
                .map(|path| format!("crates/{path}"));
            (
                named.unwrap_or(format!("crates/examples/src/e{n}.rs")),
                code,
            )
        })
        .collect()
}

#[test]
fn every_example_passes_the_subset() {
    let files = example_files();
    assert_eq!(files.len(), 24);
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, code)| (path.as_str(), code.as_str()))
        .collect();
    assert_eq!(check_repo_config("examples", &files), "");
}

/// A config for the score tests: the given allowed ids, the owner's starting
/// tier weights, and `ids` as the `[weights.ids]` table.
fn score_config(allowed: &[&str], ids: &[(&str, &str)], extra: &str) -> String {
    let allowed: Vec<String> = allowed.iter().map(|id| format!("\"{id}\"")).collect();
    let ids: String = ids
        .iter()
        .map(|(id, tier)| format!("\"{id}\" = \"{tier}\"\n"))
        .collect();
    format!(
        "version = 0\ninclude = [\"**/*.rs\"]\nallowed = [{}]\n\n[weights]\n{extra}\n\
         [weights.tiers]\nencouraged = 0.5\nneutral = 1.0\ntolerated = 2.0\ncostly = 3.0\n\
         outside = 5.0\nboundary = 1.0\n\n[weights.ids]\n{ids}",
        allowed.join(", ")
    )
}

fn score_output(config: &Path, by: &str) -> String {
    let (code, stdout, stderr) =
        subset(&["score", "--config", config.to_str().unwrap(), "--by", by]);
    assert_eq!((code, stderr.as_str()), (0, ""));
    stdout
}

#[test]
fn score_charges_each_token_the_weight_of_its_innermost_construct() {
    // `flip`: 40 tokens. The enum's 6 cost 0.5 each (3.0). The fn's own 7
    // (`fn flip ( : - > {`) cost 1.0, `l` 0.5 and the two `Light` types 1.0
    // each (9.5). The match's own 7 (`match {` and each arm's `=>` and the
    // `,`) cost 0.5 (3.5), the scrutinee `l` 1.0, the two arm patterns' 8
    // tokens 0.5 (4.0) and the two arm values' 8 tokens 1.0 (8.0): 29.0.
    // `first`: 22 tokens. The fn's own 5 cost 5.0, `v` 0.5 and
    // `Option<Light>` 4.0. The `if let` takes its whole head `if let Some(x)
    // = v` (7 tokens), its block's braces and the `;` inside: 9 tokens at
    // 2.0 (18.0). The call `flip(x)` is 3 neutral tokens (3.0): 30.5.
    let src = "// Comments cost nothing.\n\
               enum Light { On, Off }\n\
               fn flip(l: Light) -> Light {\n    match l { Light::On => Light::Off, Light::Off => Light::On }\n}\n\
               fn first(v: Option<Light>) {\n    if let Some(x) = v { flip(x); }\n}\n";
    let config = score_config(
        &[
            "item.fn",
            "item.enum",
            "expr.match",
            "expr.path",
            "expr.call",
            "pat.path",
            "pat.ident",
            "pat.tuple_struct",
            "expr.if_let.single",
            "type.path",
        ],
        &[
            ("item.enum", "encouraged"),
            ("expr.match", "encouraged"),
            ("pat.path", "encouraged"),
            ("pat.ident", "encouraged"),
            ("pat.tuple_struct", "encouraged"),
            ("expr.if_let.single", "tolerated"),
        ],
        "",
    );
    let config = scratch(
        "score-attribution",
        &config,
        &[("lights/Cargo.toml", ""), ("lights/src/lib.rs", src)],
    );
    assert_eq!(
        score_output(&config, "fn"),
        "     score   tokens   lines per token  boundary  share fn\n\
         \x20     30.5       22       3     1.386       0.0   0.0% lights/src/lib.rs:6 first\n\
         \x20     26.0       34       3     0.765       0.0   0.0% lights/src/lib.rs:3 flip\n\
         \x20     59.5       62       7     0.960       0.0   0.0% total\n"
    );
    assert_eq!(
        score_output(&config, "crate"),
        "     score   tokens   lines per token  boundary  share crate\n\
         \x20     59.5       62       7     0.960       0.0   0.0% lights\n\
         \x20     59.5       62       7     0.960       0.0   0.0% total\n"
    );
    let (code, stdout, _) = subset(&[
        "score",
        "--config",
        config.to_str().unwrap(),
        "--by",
        "fn",
        "--json",
    ]);
    assert_eq!(code, 0);
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        (
            &json["rows"][1]["name"],
            &json["rows"][1]["tokens"],
            &json["rows"][1]["lines"]
        ),
        (&"lights/src/lib.rs:3 flip".into(), &34.into(), &3.into())
    );
    assert_eq!(json["total"]["lines"], 7);
}

#[test]
fn the_same_construct_costs_the_boundary_weight_where_a_library_fixes_its_shape() {
    let src = "#[derive(Serialize)]\nstruct Total {\n    n: u8,\n}\n\n\
               impl Drop for Total {\n    fn drop(&mut self) {}\n}\n\n\
               fn list() -> Output {\n    let mut cmd = Command::new(\"ls\");\n    cmd.output()\n}\n\n\
               fn watch() -> Pair {\n    watch::channel(false)\n}\n\n\
               fn drive(rx: Receiver) -> Total {\n    let mut state = Total { n: 0 };\n    loop {\n        state = rx.recv();\n    }\n}\n";
    let config = score_config(
        &[
            "item.fn",
            "item.struct.named",
            "item.impl.trait",
            "item.fn.receiver.mut",
            "attr.derive.Serialize",
            "stmt.let.mut",
            "pat.ident",
            "expr.call",
            "expr.path",
            "expr.lit",
            "expr.lit.bool",
            "expr.method_call",
            "expr.struct",
            "expr.loop",
            "expr.assign",
            "type.path",
        ],
        &[
            ("item.struct.named", "encouraged"),
            ("type.field", "encouraged"),
            ("pat.ident", "encouraged"),
            ("item.impl.trait", "tolerated"),
            ("expr.lit.bool", "tolerated"),
            ("type.primitive", "tolerated"),
            ("item.fn.receiver.mut", "costly"),
            ("stmt.let.mut", "costly"),
            ("expr.assign", "costly"),
            ("expr.loop", "costly"),
        ],
        "",
    );
    let config = scratch("score-boundary", &config, &[("lib.rs", src)]);
    // `let mut cmd` exists to lend `cmd` to a library method, so its `let =
    // ;` cost the boundary weight (3.0). The actor's `let mut state` keeps
    // the costly tier (9.0). So do `impl Drop` (4 tokens), its `&mut self`
    // (3), the serde derive (5) and the `false` passed to `watch::channel`.
    assert_eq!(
        score_output(&config, "construct"),
        "     score   tokens   lines per token  boundary  share  count tier       construct\n\
         \x20     23.0       23       7     1.000       0.0   0.0%      4 neutral    item.fn\n\
         \x20     15.0        3       3     5.000       0.0   0.0%      1 outside    expr.loop\n\
         \x20     11.0       11       4     1.000       0.0   0.0%      5 neutral    expr.path\n\
         \x20      9.0        3       1     3.000       0.0   0.0%      1 costly     stmt.let.mut\n\
         \x20      6.0        6       2     1.000       0.0   0.0%      2 neutral    expr.method_call\n\
         \x20      5.0        5       1     1.000       5.0 100.0%      1 boundary   attr.derive.Serialize\n\
         \x20      5.0        5       4     1.000       0.0   0.0%      5 neutral    type.path\n\
         \x20      4.0        4       1     1.000       0.0   0.0%      1 neutral    expr.struct\n\
         \x20      4.0        4       2     1.000       4.0 100.0%      1 boundary   item.impl.trait\n\
         \x20      3.0        1       1     3.000       0.0   0.0%      1 costly     expr.assign\n\
         \x20      3.0        3       1     1.000       3.0 100.0%      1 boundary   item.fn.receiver.mut\n\
         \x20      3.0        6       3     0.500       0.0   0.0%      1 encouraged item.struct.named\n\
         \x20      3.0        3       1     1.000       3.0 100.0%      1 boundary   stmt.let.mut\n\
         \x20      2.5        5       3     0.500       0.0   0.0%      3 encouraged pat.ident\n\
         \x20      2.0        2       2     1.000       0.0   0.0%      2 neutral    expr.call\n\
         \x20      2.0        2       2     1.000       0.0   0.0%      2 neutral    expr.lit\n\
         \x20      2.0        1       1     2.000       0.0   0.0%      1 tolerated  type.primitive\n\
         \x20      1.0        1       1     1.000       1.0 100.0%      1 boundary   expr.lit.bool\n\
         \x20    103.5       88      20     1.176      16.0  15.5%                   total\n"
    );
}

#[test]
fn typing_charges_primitives_in_signatures_and_fields_and_std_containers_and_untyped_data() {
    let derive = "#[derive(Serialize)]\nstruct Msg {\n    body: Vec<u8>,\n}\n";
    let config = score_config(
        &[
            "item.fn",
            "item.struct.named",
            "item.struct.newtype",
            "pat.ident",
            "type.path",
            "attr.derive.Serialize",
        ],
        &[
            ("item.struct.named", "encouraged"),
            ("item.struct.newtype", "encouraged"),
            ("pat.ident", "encouraged"),
            ("type.field", "encouraged"),
            ("type.primitive", "tolerated"),
            ("type.untyped", "costly"),
        ],
        "external_protocols = [\"wire/**\"]\nuntyped = [\"serde_json::Value\"]\n",
    );
    let config = scratch(
        "score-typing",
        &config,
        &[
            ("primitive.rs", "fn resume(session: String) {}\n"),
            (
                "typed.rs",
                "struct SessionId(String);\n\nfn resume(session: SessionId) {}\n",
            ),
            ("untyped.rs", "struct Event {\n    body: Value,\n}\n"),
            (
                "contained.rs",
                "struct Peer {\n    name: Option<String>,\n    tx: Sender<String>,\n}\n",
            ),
            ("wire/lib.rs", derive),
            ("proto/lib.rs", derive),
        ],
    );
    // contained.rs: `struct Peer {`, both fields' `name : ,` and the
    // containers' `Option < >` and `Sender < >` cost 0.5 each (7.5). The
    // `String` in std's `Option` is a primitive (2.0); the one in a library's
    // `Sender` is a type argument the library fixes (boundary, 1.0).
    // primitive.rs: `fn resume ( : {` 5.0, `session` 0.5, `String` 2.0.
    // typed.rs: the newtype's `struct SessionId ( ;` and its `String` cost
    // 0.5 each (2.5), and the fn 6.5 with `SessionId` at 1.0.
    // untyped.rs: `struct Event { body : ,` 3.0 and `Value` 3.0.
    // wire/lib.rs mirrors an external protocol: the derive (5 tokens), the
    // field's `body :` and its `Vec<u8>` all cost the boundary weight, and
    // `struct Msg { ,` 0.5 each. proto/lib.rs is our own format: the same
    // field costs `body :` 0.5 each and `Vec<u8>` 2.0 a token.
    assert_eq!(
        score_output(&config, "file"),
        "     score   tokens   lines per token  boundary  share file\n\
         \x20     10.5       17       4     0.618       1.0   9.5% contained.rs\n\
         \x20      7.5        7       1     1.071       0.0   0.0% primitive.rs\n\
         \x20     16.0       15       4     1.067       5.0  31.2% proto/lib.rs\n\
         \x20      9.0       12       2     0.750       0.0   0.0% typed.rs\n\
         \x20      6.0        7       3     0.857       0.0   0.0% untyped.rs\n\
         \x20     13.0       15       4     0.867      11.0  84.6% wire/lib.rs\n\
         \x20     62.0       73      18     0.849      17.0  27.4% total\n"
    );
}

#[test]
fn the_readme_lists_every_score_only_id() {
    let (code, table, _) = subset(&["score-ids"]);
    assert_eq!(code, 0);
    let readme = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"))
        .expect("README.md");
    assert!(
        readme.contains(&table),
        "README.md is out of date; paste the output of `subset score-ids` into it"
    );
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?}");
}

#[test]
fn report_html_writes_one_static_page_with_history() {
    let config = score_config(
        &[
            "item.fn",
            "item.enum",
            "expr.match",
            "expr.path",
            "pat.path",
            "pat.ident",
            "type.path",
        ],
        &[
            ("item.enum", "encouraged"),
            ("expr.match", "encouraged"),
            ("pat.path", "encouraged"),
            ("pat.ident", "encouraged"),
        ],
        "",
    )
    .replace("**/*.rs", "lights/**/*.rs");
    let config = scratch(
        "report-html",
        &config,
        &[
            ("lights/Cargo.toml", ""),
            ("lights/src/lib.rs", "enum Light { On, Off }\n"),
            ("other/main.rs", "fn main() { loop {} }\n"),
        ],
    );
    let root = config.parent().unwrap();
    git(root, &["init", "-q"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "one"]);
    std::fs::write(
        root.join("lights/src/lib.rs"),
        "enum Light { On, Off }\n\nfn flip(l: Light) -> Light {\n    match l { Light::On => Light::Off, Light::Off => Light::On }\n}\n",
    )
    .unwrap();
    git(root, &["commit", "-q", "-am", "two"]);
    let out = root.join("site");
    let (code, stdout, stderr) = subset(&[
        "report-html",
        "--config",
        config.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--compare",
        root.join("other").to_str().unwrap(),
        "--history",
        "5",
    ]);
    assert_eq!((code, stdout.as_str(), stderr.as_str()), (0, "", ""));
    let html = std::fs::read_to_string(out.join("index.html")).unwrap();
    assert!(!html.contains("<script"), "the page has no script");
    // Tiles: `flip` and `Light` score 29.0 over 40 tokens on 4 lines, as in
    // the score test, so the score is about 29.0 / (40 / 4) = 2.9 lines.
    // History: the first commit's enum alone scored 3.0.
    for expected in [
        "<div class=\"value\">29</div>",
        "<div class=\"value\">≈ 3 lines</div>",
        "<div class=\"value\">40</div>",
        "4 lines, comments excluded",
        "40 tokens · 4 lines, score ≈ 3 lines",
        "<td>29</td><td>≈ 3 lines</td><td>40</td><td>4</td>",
        "<div class=\"value\">0.725</div>",
        "<span class=\"badge\">Comparison</span>",
        "Score per crate at each of the last 2 commits",
        "<td>29</td></tr>",
        "<td>3</td></tr>",
        "<code>flip</code>",
    ] {
        assert!(html.contains(expected), "missing {expected}");
    }
}
