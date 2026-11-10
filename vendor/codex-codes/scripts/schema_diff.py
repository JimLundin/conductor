"""Compare the vendored schema bundle with one exported by an installed codex.

Usage: python3 -I schema_diff.py VENDORED.json INSTALLED.json [Definition ...]

Export the installed bundle with
`codex app-server generate-json-schema --experimental --out DIR` and pass
DIR/codex_app_server_protocol.v2.schemas.json. With no definition names, every
definition present in both bundles is compared. Prints one line per property
or enum value that exists on only one side.
"""

import json
import sys


def members(definition):
    found = set()
    stack = [definition]
    while stack:
        node = stack.pop()
        if isinstance(node, dict):
            found.update(f"prop {k}" for k in node.get("properties", {}))
            found.update(f"enum {v}" for v in node.get("enum", []) if isinstance(v, str))
            for key in ("oneOf", "anyOf", "allOf"):
                stack.extend(node.get(key, []))
    return found


def main():
    vendored = json.load(open(sys.argv[1]))["definitions"]
    installed = json.load(open(sys.argv[2]))["definitions"]
    names = sys.argv[3:] or sorted(set(vendored) & set(installed))
    for name in names:
        if name not in vendored or name not in installed:
            side = "vendored" if name in vendored else "installed"
            print(f"{name}: only in {side}")
            continue
        v, i = members(vendored[name]), members(installed[name])
        for m in sorted(i - v):
            print(f"{name}: {m} only in installed")
        for m in sorted(v - i):
            print(f"{name}: {m} only in vendored")


main()
