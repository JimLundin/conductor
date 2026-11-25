#!/usr/bin/env bash
set -euo pipefail

probe_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
probe_binary="$probe_dir/target/debug/transport-probe"
test -x "$probe_binary"
probe_root=$(cd -- "$probe_dir/../../.." && pwd)
mkdir -p "$probe_root/target"
fixture_dir=$(mktemp -d "$probe_root/target/tp.XXXXXXXX")
chmod 700 "$fixture_dir"
agent_pid=
sshd_pid=

cleanup() {
    if [[ -n "$sshd_pid" ]]; then
        kill "$sshd_pid" 2>/dev/null || true
        wait "$sshd_pid" 2>/dev/null || true
    fi
    if [[ -n "$agent_pid" ]]; then
        kill "$agent_pid" 2>/dev/null || true
        wait "$agent_pid" 2>/dev/null || true
    fi
    rm -rf -- "$fixture_dir"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

openssl req -x509 -newkey rsa:2048 -nodes -keyout "$fixture_dir/tls.key" -out "$fixture_dir/tls.crt" -days 1 -subj /CN=transport-fixture -addext subjectAltName=IP:127.0.0.1 2>/dev/null
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "$fixture_dir/app.key" 2>/dev/null
ssh-keygen -q -t ed25519 -N '' -f "$fixture_dir/agent.key"
ssh-keygen -q -t ed25519 -N '' -f "$fixture_dir/host"
cp "$fixture_dir/agent.key.pub" "$fixture_dir/authorized_keys"
python3 - "$fixture_dir" <<'PY'
import base64
import pathlib
import socket
import sys

root = pathlib.Path(sys.argv[1])
root.joinpath("host.pub.bin").write_bytes(base64.b64decode(root.joinpath("host.pub").read_text().split()[1]))
with socket.socket() as listener:
    listener.bind(("127.0.0.1", 0))
    root.joinpath("port").write_text(str(listener.getsockname()[1]))
PY
probe_port=$(cat "$fixture_dir/port")
probe_user=$(id -un)
cat > "$fixture_dir/sshd_config" <<EOF
ListenAddress 127.0.0.1
Port $probe_port
HostKey $fixture_dir/host
PidFile $fixture_dir/sshd.pid
AuthorizedKeysFile $fixture_dir/authorized_keys
StrictModes yes
PasswordAuthentication no
KbdInteractiveAuthentication no
PubkeyAuthentication yes
UsePAM no
AllowUsers $probe_user
AllowTcpForwarding no
AllowAgentForwarding no
X11Forwarding no
PermitTTY no
PermitUserRC no
ForceCommand $probe_binary ssh-wire
LogLevel ERROR
EOF
ssh-agent -D -a "$fixture_dir/a" > "$fixture_dir/agent.log" 2>&1 &
agent_pid=$!
export SSH_AUTH_SOCK="$fixture_dir/a"
python3 - "$SSH_AUTH_SOCK" <<'PY'
import pathlib
import sys
import time

for attempt in range(100):
    if pathlib.Path(sys.argv[1]).exists():
        break
    time.sleep(0.02)
else:
    raise SystemExit("fixture agent did not start")
PY
ssh-add "$fixture_dir/agent.key" 2>/dev/null
/usr/sbin/sshd -D -e -f "$fixture_dir/sshd_config" > "$fixture_dir/sshd.log" 2>&1 &
sshd_pid=$!
python3 - "$probe_port" <<'PY'
import socket
import sys
import time

for attempt in range(100):
    try:
        with socket.create_connection(("127.0.0.1", int(sys.argv[1])), timeout=0.1):
            break
    except OSError:
        time.sleep(0.02)
else:
    raise SystemExit("fixture sshd did not start")
PY
export TRANSPORT_PROBE_FIXTURE="$fixture_dir"
export TRANSPORT_PROBE_USER="$probe_user"
export TRANSPORT_PROBE_SSH_PORT="$probe_port"
export SSL_CERT_FILE="$fixture_dir/tls.crt"
"$probe_binary"
ssh-add -D >/dev/null 2>&1
"$probe_binary" ssh-missing-agent
SSH_AUTH_SOCK="$fixture_dir/missing.sock" "$probe_binary" ssh-missing-agent
