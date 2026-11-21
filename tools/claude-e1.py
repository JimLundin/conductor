#!/usr/bin/env python3
"""Run the Claude E1 sign-in check in a fresh local directory over pipes."""

import argparse
import getpass
import json
import os
from pathlib import Path
import re
import selectors
import stat
import subprocess
import sys
import time
from urllib.parse import urlsplit
import warnings


AUTHORIZE = {
    ("claude.com", "/cai/oauth/authorize"),
    ("claude.ai", "/oauth/authorize"),
    ("claude.ai", "/cai/oauth/authorize"),
}
URL = re.compile(rb"https://[^\s\x1b<>\"']+")
TOKEN = re.compile(rb"(?<![A-Za-z0-9_-])sk-ant-oat[0-9]+-[A-Za-z0-9_-]{20,}(?![A-Za-z0-9_-])")
LIMIT = 128 * 1024
TIMEOUT = 120


class Failed(Exception):
    pass


def authorization(output):
    for candidate in URL.findall(output):
        try:
            link = candidate.decode("ascii")
            url = urlsplit(link)
        except (UnicodeError, ValueError):
            continue
        if (
            all(33 <= byte <= 126 for byte in candidate)
            and (url.netloc, url.path) in AUTHORIZE
            and url.query
            and not url.fragment
        ):
            return link
    return None


def prepare(path):
    path = path.absolute()
    try:
        path.mkdir(mode=0o700)
    except FileExistsError:
        pass
    if path.is_symlink() or not path.is_dir():
        raise Failed("Use a real scratch directory.")
    info = path.stat()
    if info.st_uid != os.geteuid() or stat.S_IMODE(info.st_mode) != 0o700 or any(path.iterdir()):
        raise Failed("Use an empty scratch directory you own with mode 0700.")
    return path.resolve()


def environment(scratch):
    env = {name: os.environ[name] for name in ("PATH", "LANG", "LC_ALL") if name in os.environ}
    for option, name in {
        "HOME": "home",
        "CLAUDE_CONFIG_DIR": "claude",
        "XDG_CONFIG_HOME": "config",
        "XDG_CACHE_HOME": "cache",
        "XDG_DATA_HOME": "data",
        "XDG_STATE_HOME": "state",
        "TMPDIR": "tmp",
    }.items():
        directory = scratch / name
        directory.mkdir(mode=0o700)
        env[option] = str(directory)
    return env


def read_reply(child, terminal):
    output = bytearray()
    sent = False
    deadline = time.monotonic() + TIMEOUT
    with selectors.DefaultSelector() as selector:
        selector.register(child.stdout, selectors.EVENT_READ)
        while True:
            if not selector.select(max(0, deadline - time.monotonic())):
                raise Failed("Claude did not finish the pipe exchange in time.")
            chunk = os.read(child.stdout.fileno(), 4096)
            if not chunk:
                break
            output.extend(chunk)
            if len(output) > LIMIT:
                raise Failed("Claude produced too much output to inspect safely.")
            if not sent and (link := authorization(output[: output.rfind(b"\n") + 1])):
                print("Open this sign-in link on the other device:", link, sep="\n", file=terminal, flush=True)
                with warnings.catch_warnings():
                    warnings.simplefilter("error", getpass.GetPassWarning)
                    code = getpass.getpass("Paste the return code (hidden): ", stream=terminal).strip()
                if not code or len(code) > 4096 or any(ord(char) < 33 or ord(char) > 126 for char in code):
                    raise Failed("The return code was empty or malformed.")
                child.stdin.write(code.encode("ascii") + b"\n")
                child.stdin.flush()
                child.stdin.close()
                sent = True
                deadline = time.monotonic() + TIMEOUT
    if child.wait(timeout=max(0, deadline - time.monotonic())) != 0:
        raise Failed("Claude reported a sign-in failure.")
    if not sent:
        raise Failed("Claude did not print a recognized sign-in URL on its pipes.")
    return bytes(output)


def sign_in(command, scratch, env, terminal):
    child = subprocess.Popen(
        command,
        cwd=scratch,
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    try:
        return read_reply(child, terminal)
    finally:
        if child.poll() is None:
            child.kill()
            child.wait()
        child.stdin.close()
        child.stdout.close()


def save_token(path, token):
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(token + b"\n")
    except OSError:
        path.unlink(missing_ok=True)
        raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("claude", type=Path, help="supplied pinned Claude executable")
    parser.add_argument("scratch", type=Path, help="new or empty owner-only scratch directory")
    parser.add_argument("mode", choices=("setup-token", "login"))
    parser.add_argument("--save-token", type=Path, metavar="FILENAME")
    args = parser.parse_args()
    if args.save_token and (
        args.mode != "setup-token"
        or args.save_token.name != str(args.save_token)
        or args.save_token.name in ("", ".", "..")
    ):
        parser.error("--save-token takes a new filename inside the setup-token scratch directory")
    try:
        binary = args.claude.resolve(strict=True)
        if not binary.is_file() or not os.access(binary, os.X_OK):
            raise Failed("The supplied Claude executable is not executable.")
        with open("/dev/tty", "w", buffering=1) as terminal:
            scratch = prepare(args.scratch)
            env = environment(scratch)
            argv = ["setup-token"] if args.mode == "setup-token" else ["auth", "login", "--claudeai"]
            output = sign_in([binary, *argv], scratch, env, terminal)
            if args.mode == "setup-token":
                tokens = set(TOKEN.findall(output))
                if len(tokens) != 1:
                    raise Failed("Claude did not produce exactly one recognizable setup token.")
                if args.save_token:
                    save_token(scratch / args.save_token, tokens.pop())
                print("Success. Claude returned a setup token over pipes.", file=terminal)
                if args.save_token:
                    print("The token was saved to the requested private file.", file=terminal)
            else:
                status = subprocess.run(
                    [binary, "auth", "status"], cwd=scratch, env=env,
                    stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                    stderr=subprocess.DEVNULL, timeout=TIMEOUT, check=False,
                )
                if status.returncode or json.loads(status.stdout).get("loggedIn") is not True:
                    raise Failed("Auth status did not confirm the isolated login.")
                print("Success. Claude accepted the code over pipes and auth status confirmed the login.", file=terminal)
    except KeyboardInterrupt:
        print("Claude E1 cancelled.", file=sys.stderr)
        return 130
    except Failed as error:
        print(f"Claude E1 failed. {error}", file=sys.stderr)
        return 1
    except (OSError, ValueError, AttributeError, EOFError, getpass.GetPassWarning, subprocess.TimeoutExpired):
        print("Claude E1 failed. The terminal, private output, or Claude pipe exchange was unavailable.", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
