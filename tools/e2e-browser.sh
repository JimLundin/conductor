#!/usr/bin/env bash
# Browser end to end: builds the web app, serves it under /conductor/ like
# GitHub Pages does, and drives headless Chromium and WebKit at an iPhone
# viewport against `conductord host` running the fake CLIs
# (crates/*-cli/tests/fake-*.sh) through n0's relays. Each scenario gets its
# own host: a Claude turn with approval in each engine (turn), two Codex
# sessions on one app-server (codex, Chromium only), a question and an
# approval for the session (questions, WebKit), a picked model, a killed
# CLI continued and the Thinking disclosure (continue), a checklist shown as the
# session's tasks and the effort changed between turns (tasks, WebKit), a
# sub-agent folded under the call that started it and opened (subagents,
# Chromium), the
# credentials forms and a second Claude instance added from the app
# (credentials, WebKit), Claude signed in from the app on the fake's sign-in
# (sign-in, WebKit), two hosts paired in one app with a session started on
# each (hosts, Chromium), a home's projects with a checkout found and one
# cloned from the app and a session started in a project (projects,
# Chromium), and the screenshots of the
# states a real CLI shows only with careful timing (screens).
# Output: docs/p1/browser-run.txt, docs/p1/screens/*-{running,closed,idle}.png
# and docs/p1/screens/phone-light-tasks.png, or the same under $E2E_RECORD
# (the gate's suite writes them to target/). Every scenario runs even after
# one fails; the script exits non-zero if any did. A failed scenario's record
# carries its host logs, and its page is shot to target/e2e-failures. Each
# browser wait is on what the page shows, bounded by E2E_PATIENCE ms per wait
# and E2E_DEADLINE ms per scenario (crates/app/tests-e2e/browser.mjs). The
# web build is reused while the files it is built from are unchanged.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH" NO_COLOR=1
recordings=$PWD/fixtures/e2e
record=${E2E_RECORD:-docs/p1}
mkdir -p "$record/screens"
claude=$PWD/crates/claude-cli/tests/fake-claude.sh
target=$(tools/target-dir)
# A failed scenario's page, as it was when it failed.
export E2E_FAILURES=$target/e2e-failures
rm -rf "$E2E_FAILURES"
mkdir -p "$E2E_FAILURES"

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
site=$(mktemp -d)
ln -s "$target/dx/app/release/web/public" "$site/conductor"
(cd crates/app/tests-e2e && [ -d node_modules ] || npm install --silent)
cargo build -q -p conductord

# Fixed session paths, which the screenshots show, so a rerun that changes
# nothing redraws nothing. A second run, from another worktree too, waits
# for the first rather than share them; nothing started in the background
# holds the lock. Checkouts from before the lock used /tmp/conductor-e2e
# and port 8080 without one, so this run keeps clear of both.
scratch=/tmp/conductor-browser-e2e
exec 9>"$scratch.lock"
flock -n 9 || { echo "waiting for the browser run that holds $scratch.lock" >&2; flock 9; }
rm -rf "$scratch"
mkdir -p "$scratch"
server=
trap 'kill "$server" 2>/dev/null || true; rm -rf "$site" "$scratch"' EXIT

# The site on a port of its own, or $PORT, once it listens.
python3 -u -m http.server "${PORT:-0}" --bind 127.0.0.1 --directory "$site" >"$scratch/site.log" 2>&1 9>&- &
server=$!
until port=$(grep -om1 'port [0-9]*' "$scratch/site.log"); do
  kill -0 "$server" 2>/dev/null || { cat "$scratch/site.log"; exit 1; }
  sleep 0.1
done
port=${port#port }

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
  until grep -q '^join:' "$scratch/$name.log"; do
    kill -0 "${hosts[-1]}" 2>/dev/null || { cat "$scratch/$name.log"; return 1; }
    sleep 0.5
  done
  join=$(awk '/^join:/ {print $2}' "$scratch/$name.log")
}

# host_log <name>: the host's log without the ticket, QR code and paths.
host_log() {
  sed -E 's/^[0-9TZ:.-]+ +//' "$scratch/$1.log" |
    grep -v '^\(ticket\|join\|data dir\|qr image\|host id\|relay\):' | grep -v '[█▀▄]'
}

# expect <name> <count> <pattern> <what>: the host log has <count> lines matching.
expect() {
  local got
  got=$(host_log "$1" | grep -c -- "$3" || true)
  [ "$got" -eq "$2" ] || { echo "FAILED: host $1: $4 (expected $2, got $got)"; return 1; }
  echo "ok: host $1: $4 ($got)"
}

# run <engine> <scenario> <output> <session dir>
run() {
  ENGINE=$1 node crates/app/tests-e2e/browser.mjs "$join" "$3" "$2" "$4"
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
  expect "turn-$1" 3 'serve: command client="iPhone"' 'the start, the message and the approval reached the host'
}

turn_chromium() { turn_in chromium; }
turn_webkit() { turn_in webkit; }

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
  expect questions 7 'serve: command client="iPhone"' 'both starts, three messages, the answer and the approval reached the host'
  expect questions 1 'body: Answer {' 'one answer reached the host'
  expect questions 1 'decision: ApproveForSession' 'one approval for the session reached the host'
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
  expect continue-host 1 'body: Continue {' 'one Continue reached the host'
  expect continue-host 1 'model: Some(ModelId("sonnet")), effort: Some(High)' 'the picked model and effort reached the host'
}

tasks() {
  session "$scratch/tasks" claude-checklist-effort.jsonl
  start_host tasks
  echo
  echo "## Tasks and the effort changed between turns, webkit"
  run webkit tasks "$record/screens/phone-light-tasks.png" "$scratch/tasks"
  echo
  echo "## Tasks, host log"
  host_log tasks
  echo
  expect tasks 1 'body: SetEffort {' 'one effort change reached the host'
  # The CLI gets the effort right before the next message, not mid-turn.
  order=$(grep -o '"subtype":"[a-z_]*"\|"text":"[a-z]*"' "$scratch/tasks/stdin.jsonl" | tr '\n' ' ')
  [ "$order" = '"subtype":"initialize" "text":"plan" "subtype":"apply_flag_settings" "text":"pong" ' ] ||
    { echo "FAILED: host tasks: what the CLI read ($order)"; return 1; }
  echo "ok: host tasks: the CLI read apply_flag_settings right before the next message"
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
  expect subagents 2 'serve: command client="iPhone"' 'the start and the message reached the host'
}

credentials() {
  # The Claude instance the scenario adds lists its models in its own work
  # dir.
  mkdir -p "$scratch/credentials/work/claude-api"
  head -2 "$recordings/claude-killed.jsonl" >"$scratch/credentials/work/claude-api/stdout.jsonl"
  start_host credentials
  echo
  echo "## Credentials, webkit"
  run webkit credentials "$scratch/credentials.png" "$scratch"
  echo
  echo "## Credentials, host log"
  host_log credentials
  echo
  expect credentials 3 'stored a credential change' "the gateway was stored and cleared, then the added instance's stored"
  expect credentials 1 'added an instance instance="claude-api"' 'a second Claude instance was added'
  expect credentials 1 'instance{id=claude-api}: host: ready' 'the added instance started its CLI'
  expect credentials 0 'conductor-test-browser-token' 'the token is in no log line'
}

sign_in() {
  start_host sign-in
  echo
  echo "## Sign-in from the app, webkit"
  run webkit sign-in "$scratch/sign-in.png" "$scratch"
  echo
  echo "## Sign-in, host log"
  host_log sign-in
  echo
  expect sign-in 2 'body: StartSignIn {' 'both sign-ins reached the host'
  expect sign-in 2 'code: SignInCode(..)' 'both codes reached the host, logged without their text'
  expect sign-in 1 'sign-in instance="claude" step=failed: Claude' 'the refused code failed the first sign-in'
  expect sign-in 1 'sign-in instance="claude" step=signed in' 'the second signed the instance in'
  expect sign-in 0 'conductor-test-sign-in-code\|conductor-test-wrong-code' 'no code is in a log line'
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
  SECOND_JOIN=$join ENGINE=chromium node crates/app/tests-e2e/browser.mjs "$first" "$scratch/hosts.png" hosts "$scratch/hosts"
  echo
  echo "## Two hosts, host logs"
  host_log desk
  host_log laptop
  echo
  expect desk 1 'serve: command client="iPhone"' "desk's session started on desk"
  expect laptop 1 'serve: command client="iPhone"' "laptop's session started on laptop"
}

# listed <home> <id> <remote> <source>: the project's project.toml in the
# home, with the source's root commit and branch, usually at <id>.
listed() {
  local root branch
  root=$(git -C "$4" rev-list --max-parents=0 HEAD)
  branch=$(git -C "$4" symbolic-ref --short HEAD)
  mkdir -p "$1/projects/$2"
  printf 'name = "%s"\nremote = "%s"\nroot = "%s"\nbranch = "%s"\nprofiles = ["rust"]\npath = "%s"\n' \
    "$2" "$3" "$root" "$branch" "$2" >"$1/projects/$2/project.toml"
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
  expect projects-host 1 'body: CloneProject { project: ProjectId("beta") }' 'the clone reached the host'
  expect projects-host 1 'at: Project(ProjectId("alpha"))' 'the session started in alpha'
  [ -e "$scratch/projects-host/projects/beta/.git" ] || { echo "FAILED: host projects-host: beta was not cloned into the data dir"; return 1; }
  echo "ok: host projects-host: beta was cloned into the data dir"
  [ -z "$(git -C "$at/roots/alpha" status --porcelain --untracked-files=no)" ] ||
    { echo "FAILED: host projects-host: the person's checkout changed"; return 1; }
  echo "ok: host projects-host: the person's checkout is untouched"
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
    trap 'failed_logs $?; kill "${hosts[@]}" 2>/dev/null || true' EXIT
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
  scenario "codex, chromium" codex_scenario
  scenario "questions, webkit" questions
  scenario "continue, chromium" continue_scenario
  scenario "tasks, webkit" tasks
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
