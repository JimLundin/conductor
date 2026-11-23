#!/usr/bin/env bash
# Browser end to end: builds the web app, serves it under /conductor/ like
# GitHub Pages does, and drives headless Chromium and WebKit at an iPhone
# viewport against `conductord host` running the fake CLIs
# (crates/*-cli/tests/fake-*.sh) through n0's relays. Each scenario gets its
# own host: a Claude turn with approval in each engine (turn), three turns'
# diffs, one unchanged and one refused once, then Continue, in each engine
# (worktrees), two Codex
# sessions on one app-server (codex, Chromium only), a question and an
# approval for the session (questions, WebKit), a picked model, a killed
# CLI continued and the Thinking disclosure (continue), the agent's checklist
# and the effort changed between turns (checklist, WebKit), a
# sub-agent folded under the call that started it and opened (subagents,
# Chromium), the
# credentials forms and a second Claude instance added from the app
# (credentials, WebKit), Claude signed in from the app on the fake's sign-in
# (sign-in, WebKit), two hosts paired in one app with a session started on
# each (hosts, Chromium), a home's projects with a checkout found and one
# cloned from the app, an issue opened, moved on, checked and noted, and a
# session started in a project (projects,
# Chromium), and the screenshots of the
# states a real CLI shows only with careful timing (screens).
# Output: browser-run.txt and screens/ under target/e2e-browser, or under
# $E2E_RECORD. Failed pages go to target/e2e-failures.
set -euo pipefail
umask 077
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH" NO_COLOR=1
source tools/e2e-common.sh
recordings=$PWD/fixtures/e2e
claude=$PWD/crates/claude-cli/tests/fake-claude.sh
target=$(tools/target-dir)
lock_browser
record=${E2E_RECORD:-$target/e2e-browser}
mkdir -p "$record/screens"
export E2E_FAILURES=$target/e2e-failures
rm -rf "$E2E_FAILURES"
mkdir -p "$E2E_FAILURES"
install_browser_dependencies
init_browser_scratch
TMPDIR="$scratch" timeout 60s node tools/test-fleet-key.mjs | tee "$record/fleet-key.txt"

# app_tree: the git tree of the files the web build reads, as they are on
# disk: the workspace crates the app depends on, and the workspace's
# manifests, lock and toolchain. It is staged in an index of its own, so the
# checkout's index is untouched.
app_tree() {
  local crates
  mapfile -t crates < <(cargo metadata -q --format-version 1 | python3 -I -c '
import json, os, sys
meta = json.load(sys.stdin)
members = set(meta["workspace_members"])
packages = {p["id"]: p for p in meta["packages"]}
deps = {n["id"]: n["dependencies"] for n in meta["resolve"]["nodes"]}
todo = [i for i in members if packages[i]["name"] == "app"]
seen = set()
while todo:
    i = todo.pop()
    if i not in seen:
        seen.add(i)
        todo += [d for d in deps[i] if d in members]
for i in sorted(seen):
    print(os.path.relpath(os.path.dirname(packages[i]["manifest_path"]), meta["workspace_root"]))
')
  GIT_INDEX_FILE=$target/e2e-app.index git add -A -- Cargo.toml Cargo.lock rust-toolchain.toml .cargo "${crates[@]}" ':!crates/app/tests-e2e'
  GIT_INDEX_FILE=$target/e2e-app.index git write-tree
}

# The web build, reused while the tree it was built from, and dx, are the same.
web=$target/dx/app/release/web
built="$(app_tree) $(dx --version)"
if [ -d "$web/public" ] && [ "$(cat "$web/built-from" 2>/dev/null)" = "$built" ]; then
  echo "reusing the web build of $built" >&2
else
  rm -f "$web/built-from"
  (cd crates/app && dx build --platform web --release >/dev/null)
  echo "$built" >"$web/built-from"
fi
cargo build -q -p conductord --bin conductord --example worktrees_browser
start_browser_server

# repo <dir>: a Git repository with one commit, which a session needs to
# start (its worktree branches from it).
repo() {
  mkdir -p "$1"
  git -C "$1" init -q
  echo baseline >"$1/baseline.txt"
  git -C "$1" add baseline.txt
  git -C "$1" -c user.name=Conductor -c user.email=conductor@example.invalid commit -qm baseline -m 'Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>'
}
# session <dir> <recording> [filter...]: a repository holding the recording,
# passed through the filter, as the fake's stdout.jsonl.
session() {
  local dir=$1 recording=$2
  shift 2
  repo "$dir"
  "${@:-cat}" <"$recordings/$recording" >"$dir/stdout.jsonl"
}
# Claude's turn plays a line every 200 ms once the message arrives, so the
# reply streams.
paced() { sed '/"match": {"type": "user"}/a {"fake": "pace", "ms": 200}'; }
# Paced, and held after the reply's third delta for a `release` file that
# never comes.
held() { paced | awk '{print} /"text_delta"/ && ++n == 3 {print "{\"fake\": \"release\", \"file\": \"release\"}"}'; }

# start_host <name> [flags...]: starts a host on the fake CLIs with its own
# data dir and sets $join to its join link. Its log is $scratch/<name>.log.
# Codex is unavailable unless $codex names its CLI.
start_host() {
  local name=$1
  shift
  # The host lists Claude's models from the CLI's initialize answer, which
  # the fake plays in the default Claude instance's work dir.
  mkdir -p "$scratch/$name/work/claude"
  head -2 "$recordings/claude-killed.jsonl" >"$scratch/$name/work/claude/stdout.jsonl"
  "$target"/debug/conductord host --data-dir "$scratch/$name" \
    --web-url "http://localhost:$port/conductor/" \
    --cli claude="$claude" --cli codex="${codex:-$scratch/no-codex}" "$@" \
    >"$scratch/$name.log" 2>&1 9>&- &
  hosts+=($!)
  names+=("$name")
  wait_for_join "${hosts[-1]}" "$name" "$scratch/$name.log"
  join=$(awk '/^join:/ {print $2}' "$scratch/$name.log")
}

# host_log <name>: the host's log without the ticket, QR code and paths.
host_log() {
  if ! grep -q '^join:' "$scratch/$1.log"; then
    echo "host $1 did not announce pairing readiness"
    return
  fi
  python3 -I tools/e2e-host-log.py "$scratch/$1.log"
}

# expect <name> <count> <pattern> <what>: the host log has <count> lines matching.
expect() {
  local got
  got=$(host_log "$1" | grep -c -- "$3" || true)
  [ "$got" -eq "$2" ] || { echo "FAILED: host $1: $4 (expected $2, got $got)"; return 1; }
  echo "ok: host $1: $4 ($got)"
}

# expect_commands <host> <count> <operation> <outcome>
expect_commands() {
  local got
  got=$(host_log "$1" | count_browser_commands "$3" "$4")
  [ "$got" -eq "$2" ] || {
    echo "FAILED: host $1: $3 -> $4 (expected $2, got $got)"
    return 1
  }
  echo "ok: host $1: $3 -> $4 ($got)"
}

# run <engine> <scenario> <output> <session dir>
run() {
  ENGINE=$1 node crates/app/tests-e2e/browser.ts "$join" "$3" "$2" "$4"
}

# turn_run <engine> <scenario> <output>: a scenario's Claude turn, in a
# session directory named after the run.
turn_run() {
  session "$scratch/$2-$1" claude-approve.jsonl paced
  run "$1" "$2" "$3" "$scratch/$2-$1"
}

# turn_in <engine>: the Claude turn in one engine, on a host of its own.
turn_in() {
  start_host "turn-$1"
  echo
  echo "## Turn, $1"
  turn_run "$1" turn "$scratch/$1.png"
  echo
  echo "## Turn, $1, host log"
  host_log "turn-$1"
  echo
  expect "turn-$1" 1 'host: ready path=.* version=fake-claude own=Unmanaged models=4' 'the host found Claude'
  expect_commands "turn-$1" 1 start_session started
  expect_commands "turn-$1" 1 send_message accepted
  expect_commands "turn-$1" 1 resolve accepted
}

turn_chromium() { turn_in chromium; }
turn_webkit() { turn_in webkit; }

# worktrees_in <engine>: three turns seeded over the link (the
# worktrees_browser example), one unchanged, with the middle turn's first
# diff refused by a git that fails once, and Continue after it.
worktrees_in() {
  local name=worktrees-$1 dir=$scratch/worktrees-$1
  session "$dir" claude-worktrees.jsonl
  cp "$recordings/claude-worktrees-continued.jsonl" "$dir/stdout.2.jsonl"
  mkdir -p "$scratch/$name-bin" "$scratch/$name-fail"
  cat >"$scratch/$name-bin/git" <<'SH'
#!/usr/bin/env bash
if [[ " $* " == *" diff --no-ext-diff --no-textconv --no-renames --name-status "* ]] &&
   rmdir "$PREVIEW_FAILURE" 2>/dev/null; then
  echo 'fixture snapshot diff failure' >&2
  exit 1
fi
exec "$REAL_GIT" "$@"
SH
  chmod +x "$scratch/$name-bin/git"
  REAL_GIT=$(command -v git) PREVIEW_FAILURE=$scratch/$name-fail \
    PATH="$scratch/$name-bin:$PATH" start_host "$name"
  local ticket
  ticket=$(awk '/^ticket:/ {print $2}' "$scratch/$name.log")
  "$target"/debug/examples/worktrees_browser "$ticket" "$dir"
  echo
  echo "## Worktrees, $1"
  run "$1" worktrees "$scratch/$name.png" "$dir"
  echo
  echo "## Worktrees, $1, host log"
  host_log "$name"
}

worktrees_chromium() { worktrees_in chromium; }
worktrees_webkit() { worktrees_in webkit; }

codex_scenario() {
  # The app-server plays its recording in the Codex instance's work dir.
  mkdir -p "$scratch/codex/work/codex"
  cp "$recordings/codex-two-threads-listed.jsonl" "$scratch/codex/work/codex/stdout.jsonl"
  repo "$scratch/codex/first"
  repo "$scratch/codex/second"
  codex=$PWD/crates/codex-cli/tests/fake-codex.sh start_host codex
  echo
  echo "## Codex, two sessions on one app-server, chromium"
  run chromium codex "$scratch/codex.png" "$scratch/codex"
  echo
  echo "## Codex, host log"
  host_log codex
  echo
  expect codex 1 'host: ready path=.* version=fake-codex own=Unmanaged' 'the host found Codex'
}

questions() {
  session "$scratch/questions/question" claude-question.jsonl
  session "$scratch/questions/for-session" claude-for-session.jsonl
  start_host questions
  echo
  echo "## Questions and approval for the session, webkit"
  run webkit questions "$scratch/questions.png" "$scratch/questions"
  echo
  echo "## Questions, host log"
  host_log questions
  echo
  expect_commands questions 2 start_session started
  expect_commands questions 3 send_message accepted
  expect_commands questions 1 answer accepted
  expect_commands questions 1 resolve accepted
  grep -Eq '"updatedPermissions": ?\[.*"destination": ?"session"' "$scratch/questions/for-session/stdin.jsonl" ||
    { echo "FAILED: host questions: the fake CLI read no session-scoped approval"; return 1; }
  echo "ok: host questions: the fake CLI received a session-scoped approval"
}

continue_scenario() {
  session "$scratch/continue/killed" claude-killed.jsonl
  cp "$recordings/claude-resumed.jsonl" "$scratch/continue/killed/stdout.2.jsonl"
  session "$scratch/continue/reasoning" claude-reasoning.jsonl paced
  start_host continue-host
  echo
  echo "## Model and effort, continue, Thinking, chromium"
  run chromium continue "$scratch/continue.png" "$scratch/continue"
  echo
  echo "## Continue, host log"
  host_log continue-host
  echo
  expect_commands continue-host 1 continue accepted
  awk '
    $0 == "run" { model = effort = 0 }
    prev == "--model" && $0 == "sonnet" { model = 1 }
    prev == "--effort" && $0 == "high" { effort = 1 }
    model && effort { found = 1 }
    { prev = $0 }
    END { exit !found }
  ' "$scratch/continue/killed/argv.txt" ||
    { echo "FAILED: host continue-host: no CLI launch used Sonnet with high effort"; return 1; }
  echo "ok: host continue-host: the CLI received the picked model and effort"
}

checklist_scenario() {
  session "$scratch/checklist" claude-checklist-effort.jsonl
  start_host checklist
  echo
  echo "## Checklist and the effort changed between turns, webkit"
  run webkit checklist "$record/screens/phone-light-checklist.png" "$scratch/checklist"
  echo
  echo "## Checklist, host log"
  host_log checklist
  echo
  expect_commands checklist 1 set_effort accepted
  # The CLI gets the effort right before the next message, not mid-turn.
  order=$(grep -o '"subtype":"[a-z_]*"\|"text":"[a-z]*"' "$scratch/checklist/stdin.jsonl" | tr '\n' ' ')
  [ "$order" = '"subtype":"initialize" "text":"plan" "subtype":"apply_flag_settings" "text":"pong" ' ] ||
    { echo "FAILED: host checklist: what the CLI read ($order)"; return 1; }
  echo "ok: host checklist: the CLI read apply_flag_settings right before the next message"
}

subagents() {
  session "$scratch/subagents" claude-subagent.jsonl
  start_host subagents
  echo
  echo "## A sub-agent under its call, opened, chromium"
  run chromium subagents "$scratch/subagents.png" "$scratch/subagents"
  echo
  echo "## Sub-agents, host log"
  host_log subagents
  echo
  expect_commands subagents 1 start_session started
  expect_commands subagents 1 send_message accepted
}

seed_claude_account_models() {
  local host=$1 label=$2 account_dir
  account_dir=claude-$(printf '%s' "$label" | sha256sum | cut -c1-16)
  mkdir -p "$scratch/$host/work/$account_dir"
  head -2 "$recordings/claude-killed.jsonl" >"$scratch/$host/work/$account_dir/stdout.jsonl"
}

credentials() {
  seed_claude_account_models credentials https://gateway.conductor-test.invalid
  start_host credentials
  echo
  echo "## Credentials, webkit"
  run webkit credentials "$scratch/credentials.png" "$scratch"
  echo
  echo "## Credentials, host log"
  host_log credentials
  echo
  expect credentials 2 'added an instance instance=claude https://gateway.conductor-test.invalid' "the gateway's account was added, removed and added again"
  expect credentials 2 'instance{id=claude https://gateway.conductor-test.invalid}: host: ready' "each time the account's instance started its CLI"
  expect credentials 0 'conductor-test-browser-token' 'the token is in no log line'
}

sign_in() {
  seed_claude_account_models sign-in "Phone setup"
  codex=$PWD/crates/codex-cli/tests/fake-codex.sh start_host sign-in
  echo
  echo "## Sign-in from the app, webkit"
  run webkit sign-in "$scratch/sign-in.png" "$scratch"
  echo
  echo "## Sign-in, host log"
  host_log sign-in
  echo
  expect_commands sign-in 3 start_sign_in accepted
  expect_commands sign-in 1 start_credential_sign_in accepted
  expect_commands sign-in 3 submit_sign_in_code accepted
  expect sign-in 1 'sign-in instance=claude step=failed: Claude' 'the refused code failed the first sign-in'
  expect sign-in 2 'sign-in instance=claude step=signed in' 'the per-host and setup-token sign-ins both completed'
  expect sign-in 1 'added an instance instance=claude conductor-test@example.invalid' 'the account the CLI named became an instance'
  expect sign-in 0 'conductor-test-sign-in-code\|conductor-test-wrong-code\|sk-ant-oat01-conductor-synthetic' 'no code or setup token is in a log line'
  if grep -rlq 'conductor-test-sign-in-code\|conductor-test-wrong-code' "$scratch/sign-in"; then
    echo "FAILED: host sign-in: a code is in the data dir"
    return 1
  fi
  echo "ok: host sign-in: no code is in the data dir"
}

two_hosts() {
  session "$scratch/hosts/desk" claude-approve.jsonl
  session "$scratch/hosts/laptop" claude-approve.jsonl
  start_host desk --name desk
  local first=$join
  start_host laptop --name laptop
  echo
  echo "## Two hosts in one app, chromium"
  SECOND_JOIN=$join ENGINE=chromium node crates/app/tests-e2e/browser.ts "$first" "$scratch/hosts.png" hosts "$scratch/hosts"
  echo
  echo "## Two hosts, host logs"
  host_log desk
  host_log laptop
  echo
  expect_commands desk 1 start_session started
  expect_commands laptop 1 start_session started
}

# listed <home> <id> <remote> <source>: the project's project.toml in the
# home, named and titled as its id, with the source's root commit.
listed() {
  local root
  root=$(git -C "$4" rev-list --max-parents=0 HEAD)
  mkdir -p "$1/projects/$2"
  printf 'name = "%s"\ntitle = "%s"\nremote = "%s"\nroot = "%s"\nprofiles = ["rust"]\n' \
    "$2" "$2" "$3" "$root" >"$1/projects/$2/project.toml"
}

projects_scenario() {
  local at=$scratch/projects
  # Each project's one commit holds a file of its own, so their roots differ.
  for name in alpha beta; do
    mkdir -p "$at/sources/$name"
    git -C "$at/sources/$name" init -q
    echo "$name" >"$at/sources/$name/$name.txt"
    git -C "$at/sources/$name" add .
    git -C "$at/sources/$name" -c user.name=Conductor -c user.email=conductor@example.invalid commit -qm "$name" -m 'Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>'
    git clone -q --bare "$at/sources/$name" "$at/$name-remote.git"
  done
  # The person's own checkout of alpha, holding the session's recording.
  git clone -q "file://$at/alpha-remote.git" "$at/roots/alpha"
  cp "$recordings/claude-approve.jsonl" "$at/roots/alpha/stdout.jsonl"
  mkdir -p "$at/sources/home"
  git -C "$at/sources/home" init -q
  listed "$at/sources/home" alpha "file://$at/alpha-remote.git" "$at/sources/alpha"
  listed "$at/sources/home" beta "file://$at/beta-remote.git" "$at/sources/beta"
  # The tests' own flow, which the issues follow.
  mkdir -p "$at/sources/home/docs/agents"
  cp crates/home/src/flow/test-flow.toml "$at/sources/home/docs/agents/flow.toml"
  git -C "$at/sources/home" add .
  git -C "$at/sources/home" -c user.name=Conductor -c user.email=conductor@example.invalid commit -qm 'list alpha and beta' -m 'Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>'
  git clone -q --bare "$at/sources/home" "$at/home-remote.git"
  start_host projects-host --home "file://$at/home-remote.git" --projects "$at/roots"
  echo
  echo "## Projects, a clone from the app and a session in a project, chromium"
  run chromium projects "$scratch/projects.png" "$at"
  echo
  echo "## Projects, host log"
  host_log projects-host
  echo
  expect_commands projects-host 1 clone_project accepted
  expect_commands projects-host 2 start_session started
  expect_commands projects-host 1 start_session rejected
  [ -n "$(git -C "$at/roots/alpha" for-each-ref 'refs/heads/issue/')" ] ||
    { echo "FAILED: host projects-host: the checkout has no issue branch"; return 1; }
  echo "ok: host projects-host: the issue's branch is in the checkout"
  [ -e "$scratch/projects-host/projects/beta/.git" ] || { echo "FAILED: host projects-host: beta was not cloned into the data dir"; return 1; }
  echo "ok: host projects-host: beta was cloned into the data dir"
  [ -z "$(git -C "$at/roots/alpha" status --porcelain --untracked-files=no)" ] ||
    { echo "FAILED: host projects-host: the person's checkout changed"; return 1; }
  echo "ok: host projects-host: the person's checkout is untouched"
  local pushed
  pushed=$(git -C "$at/home-remote.git" log --format=%s)
  for subject in '^Open issue .*: Remove sessions$' '^Move issue .* to active$' '^Add an item to issue ' '^Note on issue '; do
    grep -q "$subject" <<<"$pushed" ||
      { echo "FAILED: host projects-host: the home's remote has no commit matching $subject"; return 1; }
  done
  echo "ok: host projects-host: each issue change reached the home's remote as its own commit"
}

screens() {
  # conductor runs, held mid-reply; scratch's CLI hangs up as the session
  # starts; notes is never sent a message.
  session "$scratch/screens/conductor" claude-approve.jsonl held
  session "$scratch/screens/scratch" claude-hang-up.jsonl
  session "$scratch/screens/notes" claude-approve.jsonl
  start_host screens
  echo
  echo "## Screens, chromium"
  run chromium screens "$record/screens" "$scratch/screens"
}

# failed_logs <status>: after a failure, each of the scenario's host logs,
# which the scenario shows only once its browser part passed.
failed_logs() {
  [ "$1" -eq 0 ] && return
  local name
  for name in "${names[@]}"; do
    echo
    echo "## $name, host log at the failure"
    host_log "$name"
  done
}

# scenario <name> <function>: runs the function in a shell of its own that
# stops at its first failure and stops its hosts, and records the outcome.
# The shell must not run as a condition, where errexit is off.
failed=()
scenario() {
  local status
  set +e
  (
    set -e
    hosts=()
    names=()
    trap 'failed_logs $?; stop_children "${hosts[@]}"' EXIT
    "$2"
  )
  status=$?
  set -e
  if [ "$status" -eq 0 ]; then
    echo "PASSED: $1"
  else
    echo "FAILED: $1 (exit $status)"
    failed+=("$1")
  fi
}

out=$record/browser-run.txt
{
  echo "# Web build in headless Chromium and WebKit (390x844) against conductord host on the fake CLIs, $(date -u +%FT%TZ)"
  echo
  wasm=$(ls -t "$target"/dx/app/release/web/public/assets/*.wasm | head -1)
  echo "wasm: $(stat -c %s "$wasm") bytes raw, $(gzip -9 -c "$wasm" | wc -c) bytes gzip -9"
  scenario "turn, chromium" turn_chromium
  scenario "turn, webkit" turn_webkit
  scenario "worktrees, chromium" worktrees_chromium
  scenario "worktrees, webkit" worktrees_webkit
  scenario "codex, chromium" codex_scenario
  scenario "questions, webkit" questions
  scenario "continue, chromium" continue_scenario
  scenario "checklist, webkit" checklist_scenario
  scenario "subagents, chromium" subagents
  scenario "credentials, webkit" credentials
  scenario "sign-in, webkit" sign_in
  scenario "hosts, chromium" two_hosts
  scenario "projects, chromium" projects_scenario
  scenario "screens, chromium" screens
  echo
  echo "## Summary"
  if [ "${#failed[@]}" -eq 0 ]; then
    echo "all scenarios passed"
  else
    echo "failed: ${failed[*]}"
  fi
} >"$out" 2>&1
python3 tools/scrub-ips.py "$out"
cat "$out"
[ "${#failed[@]}" -eq 0 ]
