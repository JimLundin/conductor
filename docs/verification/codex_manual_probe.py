#!/usr/bin/env python3
"""Run disposable Codex E3 probes without printing provider frames."""

import argparse
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import threading
import time


class ProbeError(Exception):
    pass


class AppServer:
    def __init__(self, process):
        self.process = process
        self.lines = queue.Queue()
        self.next_id = 0
        threading.Thread(target=self.read_stdout, daemon=True).start()

    def read_stdout(self):
        for line in self.process.stdout:
            self.lines.put(line)
        self.lines.put(None)

    def send(self, method, params=None, request_id=None):
        frame = {"method": method}
        if params is not None:
            frame["params"] = params
        if request_id is not None:
            frame["id"] = request_id
        self.process.stdin.write(json.dumps(frame) + "\n")
        self.process.stdin.flush()

    def receive(self, deadline):
        line = self.lines.get(timeout=max(0, deadline - time.monotonic()))
        if line is None:
            raise ProbeError("Codex closed its output.")
        frame = json.loads(line)
        if "id" in frame and "method" in frame:
            raise ProbeError("Codex requested approval. The probe stopped.")
        return frame

    def request(self, method, params, observe=lambda _frame: None):
        self.next_id += 1
        self.send(method, params, self.next_id)
        deadline = time.monotonic() + 120
        while True:
            response = self.receive(deadline)
            if response.get("id") == self.next_id:
                return response
            observe(response)

    def call(self, method, params, observe=lambda _frame: None):
        response = self.request(method, params, observe)
        if "error" in response or "result" not in response:
            raise ProbeError(f"Codex did not accept {method}.")
        return response["result"]

    def turn(self, thread_id, prompt):
        messages = []
        outcomes = {}

        def observe(frame):
            params = frame.get("params", {})
            if params.get("threadId") != thread_id:
                return
            item = params.get("item", {})
            if frame.get("method") == "item/completed" and item.get("type") == "agentMessage":
                messages.append(item.get("text", ""))
            if frame.get("method") == "turn/completed":
                turn = params["turn"]
                outcomes[turn["id"]] = turn["status"]

        result = self.call(
            "turn/start",
            {"threadId": thread_id, "input": [{"type": "text", "text": prompt}]},
            observe,
        )
        turn_id = result["turn"]["id"]
        deadline = time.monotonic() + 180
        while turn_id not in outcomes:
            observe(self.receive(deadline))
        if outcomes[turn_id] != "completed":
            raise ProbeError("The test turn did not complete.")
        return messages


def refresh(server):
    response = server.request("account/read", {"refreshToken": True})
    account = response.get("result", {}).get("account") or {}
    if account.get("type") != "chatgpt":
        raise ProbeError("The refresh did not return an authenticated ChatGPT account.")
    print("ChatGPT account/read accepted with refreshToken=true")


def thread(server, worktree):
    return server.call("thread/start", {
        "cwd": str(worktree),
        "approvalPolicy": "on-request",
        "sandbox": "workspace-write",
        "historyMode": "paginated",
    })["thread"]["id"]

def run(server, args, worktree):
    server.call("initialize", {
        "clientInfo": {"name": "conductor-manual", "version": "1"},
        "capabilities": {"experimentalApi": True},
    })
    server.send("initialized")
    if args.action == "refresh":
        refresh(server)
        return
    identifier = thread(server, worktree)
    server.turn(identifier, "Reply with OK. Do not use tools.")
    print("Test turn completed")

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("codex", type=Path, help="Absolute path to the pinned executable")
    parser.add_argument("worktree", type=Path, help="Disposable checkout inside the run directory")
    parser.add_argument("action", choices=("refresh", "turn"))
    args = parser.parse_args()
    configured = os.environ.get("CODEX_HOME")
    if not configured:
        parser.error("Set CODEX_HOME to this run's disposable codex directory.")
    scratch_home = Path(configured).resolve()
    worktree = args.worktree.resolve()
    if (
        scratch_home == (Path.home() / ".codex").resolve()
        or not scratch_home.is_dir()
        or not scratch_home.parent.name.startswith("conductor-manual.")
        or not worktree.is_dir()
        or not worktree.is_relative_to(scratch_home.parent)
        or worktree.is_relative_to(scratch_home)
        or not args.codex.is_absolute()
    ):
        parser.error("Use an absolute pinned executable path and disposable run paths.")
    try:
        with subprocess.Popen(
            [str(args.codex), "app-server"],
            cwd=worktree,
            env={**os.environ, "CODEX_HOME": str(scratch_home)},
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
        ) as process:
            try:
                run(AppServer(process), args, worktree)
            finally:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
    except ProbeError as error:
        sys.exit(str(error))
    except (OSError, ValueError, KeyError, TypeError, queue.Empty):
        sys.exit("Codex probe failed or timed out. Provider output was withheld.")


if __name__ == "__main__":
    main()
