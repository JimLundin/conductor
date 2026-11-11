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
            "control.rs:7:5: expr.if_let: `if let` expression",
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
         crates/daemon/src/config.rs:3:5: attr.arg: clap only in a binary's main.rs and in examples/ (proposal 0019)\n\
         crates/daemon/src/config.rs:4:14: type.bool: clap only in a binary's main.rs and in examples/ (proposal 0019)\n"
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
