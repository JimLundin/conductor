"""Interrupt only the provider for this live check's continuation session."""

import os
import signal
import sys
from pathlib import Path


def matches(process, parent, cli_root, workspace):
    try:
        stat = (process / "stat").read_bytes().rsplit(b") ", 1)[1].split()
        exe = (process / "exe").readlink()
        cwd = (process / "cwd").readlink()
    except (FileNotFoundError, ProcessLookupError, PermissionError):
        return False
    return int(stat[1]) == parent and exe.is_relative_to(cli_root) and cwd == workspace


def interrupt(proc, host, cli_root, workspace):
    matches_run = [
        path for path in proc.iterdir()
        if path.name.isdecimal() and matches(path, host, cli_root, workspace)
    ]
    if len(matches_run) != 1:
        raise RuntimeError(f"expected one continuation provider, found {len(matches_run)}")
    process = matches_run[0]
    pidfd = os.pidfd_open(int(process.name))
    try:
        if not matches(process, host, cli_root, workspace):
            raise RuntimeError("continuation provider changed before interruption")
        signal.pidfd_send_signal(pidfd, signal.SIGKILL)
    finally:
        os.close(pidfd)


def main():
    host, data_dir, provider, workspace = sys.argv[1:]
    if not workspace:
        sys.exit("continuation did not report its workspace")
    try:
        interrupt(
            Path("/proc"),
            int(host),
            (Path(data_dir) / "cli" / provider).resolve(strict=True),
            Path(workspace).resolve(strict=True),
        )
    except (OSError, RuntimeError) as error:
        sys.exit(f"live host crash check: {error}")


if __name__ == "__main__":
    main()
