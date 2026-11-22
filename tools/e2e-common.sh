#!/usr/bin/env bash

lock_browser() {
  mkdir -p "$target"
  exec 9>"$target/e2e-browser.lock"
  flock 9
}

install_browser_dependencies() (
  set -e
  cd crates/app/tests-e2e
  fingerprint=$(sha256sum package.json package-lock.json)
  stamp=node_modules/.conductor-lock
  if [ "$(cat "$stamp" 2>/dev/null)" != "$fingerprint" ]; then
    rm -f -- "$stamp"
    npm ci || return 1
    printf '%s\n' "$fingerprint" >"$stamp"
  fi
)

stop_children() {
  local pid
  for pid in "$@"; do
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
}

cleanup_browser() {
  if [ -n "$server" ]; then
    stop_children "$server"
  fi
  rm -rf -- "$scratch"
}

init_browser_scratch() {
  scratch=$(mktemp -d "$target/e2e.XXXXXX")
  scratch=$(cd "$scratch" && pwd -P)
  server=
  trap cleanup_browser EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
}

start_browser_server() {
  local web=$target/dx/app/release/web/public
  local end=$((SECONDS + 10))
  mkdir "$scratch/site"
  ln -s "$web" "$scratch/site/conductor"
  python3 -I tools/e2e-http.py "${PORT:-0}" "$scratch/site" >"$scratch/site.port" 2>"$scratch/site.log" 9>&- &
  server=$!
  until [ -s "$scratch/site.port" ] && read -r port <"$scratch/site.port"; do
    if ! kill -0 "$server" 2>/dev/null; then
      cat "$scratch/site.log" >&2
      return 1
    fi
    if [ "$SECONDS" -ge "$end" ]; then
      echo "browser HTTP server did not announce a listening port" >&2
      return 1
    fi
    sleep 0.1
  done
}

wait_for_join() {
  local host=$1 name=$2 log=$3 status
  until grep -q '^join:' "$log"; do
    if ! kill -0 "$host" 2>/dev/null; then
      status=0
      wait "$host" || status=$?
      echo "host $name exited before pairing readiness (exit $status)" >&2
      return 1
    fi
    sleep 0.5
  done
}

count_browser_commands() {
  awk -v operation="$1" -v outcome="$2" '
    $2 == "daemon::serve:" && $3 == "command" && $5 ~ /^command=[0-9]+$/ {
      gsub(/"/, "", $6)
      if ($4 == "received" && $6 == "operation=" operation) received[$5] = 1
      if ($4 == "finished" && $6 == "outcome=" outcome) finished[$5] = 1
    }
    END {
      matching = 0
      for (id in received) if (id in finished) matching++
      print matching
    }
  '
}
