#!/usr/bin/env python3
"""Writes the architecture page: one static HTML file, inline SVG, no script.

Reads docs/status.toml (what is built, being built, planned), cargo metadata
(crates and their dependencies) and the subset tool (size and violations per
file). Runs offline.

usage: python3 tools/archmap/archmap.py --out DIR [--subset $(tools/target-dir)/debug/subset]
"""

import argparse
import datetime
import json
import re
import shutil
from fnmatch import fnmatch
import subprocess
import tomllib
from dataclasses import dataclass, field
from html import escape
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CARGO = shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo")
REPO = "https://github.com/JimLundin/conductor"
STATUSES = ("done", "in_progress", "planned", "delete")
LABEL = {"done": "Built", "in_progress": "Being built", "planned": "Planned", "delete": "To be deleted"}


@dataclass
class File:
    tokens: int
    lines: int
    violations: int = 0


@dataclass
class Crate:
    name: str
    dir: str
    purpose: str
    in_subset: bool
    deps: list = field(default_factory=list)  # (name, kind): kind is "normal", "dev" or a unit id
    files: dict = field(default_factory=dict)  # path relative to the crate dir -> File

    @property
    def tokens(self):
        return sum(f.tokens for f in self.files.values())

    @property
    def lines(self):
        return sum(f.lines for f in self.files.values())

    @property
    def violations(self):
        return sum(f.violations for f in self.files.values())


# ---------------------------------------------------------------- inputs


def load_status(path):
    status = tomllib.loads(path.read_text())
    units = {}
    for phase in status["phase"]:
        for unit in phase["unit"]:
            if unit["status"] not in STATUSES[:3]:
                raise SystemExit(f"{path}: unit {unit['id']}: unknown status {unit['status']!r}")
            if (unit["status"] == "in_progress") != ("branch" in unit):
                raise SystemExit(f"{path}: unit {unit['id']}: a branch exactly when in_progress")
            units[unit["id"]] = unit
    for entry in status.get("module", []) + status.get("planned_dep", []):
        if entry["unit"] not in units:
            raise SystemExit(f"{path}: {entry}: no unit {entry['unit']!r}")
    return status, units


def run(*cmd):
    done = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True)
    if done.returncode not in (0, 1) or not done.stdout:
        raise SystemExit(f"{' '.join(cmd)} failed:\n{done.stderr}")
    return done.stdout


def load_crates(status, subset):
    meta = json.loads(run(CARGO, "metadata", "--format-version", "1", "--no-deps", "--offline"))
    include = tomllib.loads((ROOT / "subset.toml").read_text())["include"]
    crates = {}
    for package in meta["packages"]:
        dir = str(Path(package["manifest_path"]).parent.relative_to(ROOT))
        crates[package["name"]] = Crate(
            name=package["name"],
            dir=dir,
            purpose=status["purpose"].get(package["name"], package.get("description") or ""),
            in_subset=any(fnmatch(f"{dir}/src/lib.rs", glob) for glob in include),
        )
    for package in meta["packages"]:
        for dep in package["dependencies"]:
            if dep["name"] in crates:
                crates[package["name"]].deps.append((dep["name"], dep["kind"] or "normal"))
    for dep in status.get("planned_dep", []):
        crates[dep["from"]].deps.append((dep["to"], dep["unit"]))

    by_dir = sorted(crates.values(), key=lambda c: -len(c.dir))
    def owner(path):
        return next(c for c in by_dir if path.startswith(c.dir + "/"))

    score = json.loads(run(subset, "score", *sorted({c.dir for c in crates.values()}), "--json", "--by", "file"))
    for row in score["rows"]:
        crate = owner(row["name"])
        crate.files[row["name"][len(crate.dir) + 1:]] = File(row["tokens"], row["lines"])
    for line in run(subset, "check").splitlines():
        if m := re.match(r"(\S+\.rs):\d+:\d+: ", line):
            crate = owner(m[1])
            crate.files[m[1][len(crate.dir) + 1:]].violations += 1
    return crates


# ---------------------------------------------------------------- shared marks


def mark(status, size=14):
    """The status glyph: a notehead, filled when built, half when being built,
    hollow when planned, struck when it will be deleted."""
    return (f'<svg class="mark m-{status}" width="{size}" height="{size}" viewBox="-8 -8 16 16" aria-hidden="true">'
            f'<use href="#nh-{status}" x="-8" y="-8" width="16" height="16"/></svg>')


DEFS = """<svg width="0" height="0" style="position:absolute" aria-hidden="true"><defs>
<clipPath id="nh-left"><rect x="-8" y="-8" width="8" height="16"/></clipPath>
<symbol id="nh-done" viewBox="-8 -8 16 16" overflow="visible"><ellipse rx="6" ry="4.3" transform="rotate(-25)" fill="currentColor"/></symbol>
<symbol id="nh-in_progress" viewBox="-8 -8 16 16" overflow="visible"><ellipse rx="6" ry="4.3" transform="rotate(-25)" fill="currentColor" clip-path="url(#nh-left)"/><ellipse rx="5.3" ry="3.6" transform="rotate(-25)" fill="none" stroke="currentColor" stroke-width="1.5"/></symbol>
<symbol id="nh-planned" viewBox="-8 -8 16 16" overflow="visible"><ellipse rx="5.3" ry="3.6" transform="rotate(-25)" fill="none" stroke="currentColor" stroke-width="1.5" stroke-dasharray="2.4 1.6"/></symbol>
<symbol id="nh-delete" viewBox="-8 -8 16 16" overflow="visible"><ellipse rx="5.3" ry="3.6" transform="rotate(-25)" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M-7 5 7 -5" stroke="currentColor" stroke-width="1.8"/></symbol>
<pattern id="hatch" width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(45)"><rect width="6" height="6" class="hatch-bg"/><line x1="0" y1="0" x2="0" y2="6" class="hatch-line" stroke-width="2"/></pattern>
<marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 1 9 5 0 9z" fill="context-stroke"/></marker>
</defs></svg>"""


def num(n):
    return f"{n:,}"


def size(tokens, lines):
    return f"{num(tokens)}\u00a0tokens, {num(lines)}\u00a0lines"


def wrap(text, chars):
    lines, line = [], ""
    for word in text.split(" "):
        if line and len(line) + 1 + len(word) > chars:
            lines.append(line)
            line = word
        else:
            line = f"{line} {word}".strip()
    return lines + [line] if line else lines


def svg_text(x, y, text, cls="", anchor="start"):
    return f'<text x="{x:.1f}" y="{y:.1f}" class="{cls}" text-anchor="{anchor}">{escape(text)}</text>'


def svg_box(x, y, w, h, status, rows, title, rx=6):
    """A status box: the notehead, then rows of (text, class) at 16 px."""
    out = [f'<g class="box s-{status}"><title>{escape(title)}</title>',
           f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{rx}"/>',
           f'<use href="#nh-{status}" x="{x + 6}" y="{y + 6}" width="16" height="16" class="m-{status}"/>']
    ty = y + 19
    for i, (text, cls) in enumerate(rows):
        out.append(svg_text(x + (26 if i == 0 else 10), ty, text, cls))
        ty += 16
    return "".join(out) + "</g>"


def clip(box, tx, ty):
    """Where the line from a box's centre towards (tx, ty) leaves the box."""
    x, y, w, h = box
    cx, cy = x + w / 2, y + h / 2
    dx, dy = tx - cx, ty - cy
    scale = min(w / 2 / abs(dx) if dx else 1e9, h / 2 / abs(dy) if dy else 1e9)
    return cx + dx * scale, cy + dy * scale


def svg_edge(a, b, status, label, title, both=False, lx=0, ly=0):
    acx, acy = a[0] + a[2] / 2, a[1] + a[3] / 2
    bcx, bcy = b[0] + b[2] / 2, b[1] + b[3] / 2
    x1, y1 = clip(a, bcx, bcy)
    x2, y2 = clip(b, acx, acy)
    start = ' marker-start="url(#arrow)"' if both else ""
    out = (f'<g class="edge e-{status}"><title>{escape(title)}</title>'
           f'<line x1="{x1:.1f}" y1="{y1:.1f}" x2="{x2:.1f}" y2="{y2:.1f}" marker-end="url(#arrow)"{start}/>')
    if label:
        mx, my = (x1 + x2) / 2 + lx, (y1 + y2) / 2 + ly
        out += "".join(svg_text(mx, my + i * 14, part, "elabel", "middle") for i, part in enumerate(label.split("\n")))
    return out + "</g>"


def two_svgs(wide, narrow, label):
    """The same view laid out twice; CSS shows the one that fits."""
    return "".join(f'<svg class="{cls}" viewBox="0 0 {w} {h}" role="img" aria-label="{escape(label)}">{body}</svg>'
                   for cls, (w, h, body) in (("wide", wide), ("narrow", narrow)))


# ---------------------------------------------------------------- views


def roadmap(status, units):
    out = []
    for phase in status["phase"]:
        cards = []
        for unit in phase["unit"]:
            branch = f'<code class="branch">{escape(unit["branch"])}</code>' if "branch" in unit else ""
            touches = ", ".join(unit["touches"]) or "docs and tools only"
            cards.append(
                f'<li class="card s-{unit["status"]}" title="{escape(unit["purpose"])}&#10;Touches: {escape(touches)}">'
                f'<div class="card-head">{mark(unit["status"])}<strong>{escape(unit["name"])}</strong></div>'
                f'{branch}<p class="done-when">Done when: {escape(unit["done_when"])}</p></li>')
        counts = {s: sum(u["status"] == s for u in phase["unit"]) for s in STATUSES[:3]}
        tally = ", ".join(f"{n} {LABEL[s].lower()}" for s, n in counts.items() if n)
        out.append(f'<section class="phase"><h3>{escape(phase["name"])} <span class="tally">{tally}</span></h3>'
                   f'<ol class="cards">{"".join(cards)}</ol></section>')
    return "".join(out)


def subset_text(crate):
    if not crate.in_subset:
        return "outside the subset"
    if crate.violations == 0:
        return "in the subset: 0\u00a0violations\u00a0✓"
    return f"subset: {num(crate.violations)}\u00a0violations\u00a0▲"


def working_on(crate, units):
    return [u for u in units.values() if u["status"] == "in_progress"
            and any(t.split("/")[0] == crate.name for t in u["touches"])]


def crate_status(crate, units):
    return "in_progress" if working_on(crate, units) else "done"


def short(unit):
    return unit["name"].split(":")[0]


def crate_order(crates):
    return sorted(crates.values(), key=lambda c: (["crates", "vendor", "tools"].index(c.dir.split("/")[0]), c.name))


def crate_graph(crates, units, width, cols):
    """Layered: a crate sits one row above everything it depends on. Within a
    row, crates sit under the crates that use them; an arrow that skips a row
    passes through the nearest gap in it."""
    depth = {}
    def level(name):
        if name not in depth:
            depth[name] = 1 + max((level(d) for d, _ in crates[name].deps), default=-1)
        return depth[name]
    order = crate_order(crates)
    users = {c.name: [u.name for u in order if any(d == c.name for d, _ in u.deps)] for c in order}
    slot, rows = {}, []
    for d in range(max(level(c.name) for c in order), -1, -1):
        layer = [c for c in order if depth[c.name] == d]
        def pull(c):
            above = [slot[u] for u in users[c.name] if u in slot]
            return sum(above) / len(above) if above else 1e9
        layer.sort(key=pull)
        for part in (layer[i:i + cols] for i in range(0, len(layer), cols)):
            for i, c in enumerate(part):
                slot[c.name] = i
            rows.append(part)
    gap, pad = 28, 8
    w = (width - 2 * pad - (cols - 1) * gap) / cols
    chars = int((w - 20) / 6.5)
    boxes, out, gaps, y = {}, [], [], pad
    for row in rows:
        contents = []
        for c in row:
            lines = [(c.name, "name")] + [(t, "small") for t in wrap(c.purpose, chars)[:3]]
            lines += [(t, "small") for t in wrap(size(c.tokens, c.lines), chars)]
            lines += [(t, "small v" if c.violations else "small") for t in wrap(subset_text(c), chars)]
            for u in working_on(c, units):
                lines += [(t, "small now") for t in wrap(f"now: {short(u)}, {u['branch']}", chars)]
            contents.append(lines)
        h = 14 + 16 * max(len(l) for l in contents)
        x = pad + (width - 2 * pad - (len(row) * w + (len(row) - 1) * gap)) / 2
        free = [pad, x - 10]
        for c, lines in zip(row, contents):
            boxes[c.name] = (x, y, w, h, lines)
            free += [x + w + 10, x + w + gap - 10]
            x += w + gap
        free[-1] = width - pad
        gaps.append((y + h / 2, h, [(free[i], free[i + 1]) for i in range(0, len(free), 2) if free[i + 1] >= free[i]]))
        y += h + 56
    for c in order:
        for dep, kind in c.deps:
            ax, ay, aw, ah, _ = boxes[c.name]
            bx, by, bw, bh, _ = boxes[dep]
            st = "done" if kind in ("normal", "dev") else units[kind]["status"]
            x1, y1 = ax + aw / 2 + (bx + bw / 2 - ax - aw / 2) * 0.15, ay + ah
            x2, y2 = bx + bw / 2 + (ax + aw / 2 - bx - bw / 2) * 0.15, by - 2
            if abs(ay - by) < 1:
                x1, y1, x2, y2 = (ax + aw, ay + ah / 2, bx, by + bh / 2) if bx > ax else (ax, ay + ah / 2, bx + bw, by + bh / 2)
                path = f"M{x1:.1f} {y1:.1f} L{x2:.1f} {y2:.1f}"
            else:
                points = [(x1, y1)]
                for mid, h, free in gaps:
                    if y1 < mid < y2:
                        want = x1 + (x2 - x1) * (mid - y1) / (y2 - y1)
                        gx = min((min(max(want, lo), hi) for lo, hi in free), key=lambda g: abs(g - want))
                        points += [(gx, mid - h / 2 - 6), (gx, mid + h / 2 + 6)]
                points.append((x2, y2))
                path = f"M{x1:.1f} {y1:.1f}"
                for (px, py), (qx, qy) in zip(points, points[1:]):
                    if px == qx:
                        path += f" L{qx:.1f} {qy:.1f}"
                    else:
                        path += f" C{px:.1f} {py + 26:.1f} {qx:.1f} {qy - 26:.1f} {qx:.1f} {qy:.1f}"
            what = {"normal": "depends on", "dev": "tests depend on"}.get(kind) or f"will depend on, from {units[kind]['name']},"
            cls = "dev" if kind == "dev" else st
            out.append(f'<g class="edge e-{cls}"><title>{c.name} {what} {dep}</title>'
                       f'<path d="{path}" fill="none" marker-end="url(#arrow)"/></g>')
    for c in order:
        x, by, bw, bh, lines = boxes[c.name]
        title = f"{c.name} ({c.dir}): {c.purpose}"
        out.append(svg_box(x, by, bw, bh, crate_status(c, units), lines, title))
    return width, y - 56 + pad, "".join(out)


def module_states(crate, status, units):
    """Every module today and after the units in status.toml: path -> (today, target, entry)."""
    entries = {e["path"].split("/", 1)[1]: e for e in status.get("module", []) if e["path"].split("/")[0] == crate.name}
    paths = sorted(set(crate.files) | set(entries), key=lambda p: (not p.startswith("src/"), p))
    states = {}
    for path in paths:
        e = entries.get(path)
        unit = units[e["unit"]] if e else None
        exists = path in crate.files
        if not e:
            states[path] = ("done", "done", None)
        elif e.get("delete"):
            states[path] = ("delete" if unit["status"] != "done" else None, None, e)
        else:
            states[path] = ("done" if exists else None, unit["status"], e)
    return states


def group_paths(crate, paths):
    """src/ files one by one; anything else by top directory when it holds several."""
    groups = {}
    for p in paths:
        key = p if p.startswith("src/") else p.split("/")[0] + "/"
        groups.setdefault(key, []).append(p)
    return {k if len(v) > 1 else v[0]: v for k, v in groups.items()}


def module_box(crate, name, paths, state, entry, units, target):
    files = [crate.files[p] for p in paths if p in crate.files]
    label = name.removeprefix("src/")
    lines = [f'<div class="mod-head">{mark(state, 12)}<code>{escape(label)}</code></div>']
    if len(paths) > 1:
        lines.append(f'<div class="meta">{len(paths)} files</div>')
    if files and not (target and state != "done"):
        tokens, nlines, v = sum(f.tokens for f in files), sum(f.lines for f in files), sum(f.violations for f in files)
        lines.append(f'<div class="meta">{size(tokens, nlines)}</div>')
        if crate.in_subset:
            lines.append(f'<div class="meta {"v" if v else "ok"}">{f"{v} violations ▲" if v else "0 violations ✓"}</div>')
    title = f"{crate.dir}/{name}: {LABEL[state]}"
    if entry:
        unit = units[entry["unit"]]
        verb = "deletes it" if entry.get("delete") else ("rewrites it" if paths[0] in crate.files and state != "delete" else "builds it")
        lines.append(f'<div class="note">{escape(entry["note"])}</div>'
                     f'<div class="meta unit">{escape(short(unit))} {verb}</div>')
        title += f". {entry['note']} {unit['name']}, {LABEL[unit['status']].lower()}."
    return f'<li class="mod s-{state}" title="{escape(title)}">{"".join(lines)}</li>'


def inside(crates, status, units):
    out = []
    for crate in crate_order(crates):
        states = module_states(crate, status, units)
        changing = any(e for _, _, e in states.values())
        def column(which, heading):
            present = [p for p, s in states.items() if s[which] is not None]
            boxes = []
            for name, paths in group_paths(crate, present).items():
                st = states[paths[0]]
                boxes.append(module_box(crate, name, paths, st[which] if len(paths) == 1 else "done", st[2] if len(paths) == 1 else None, units, which == 1))
            head = f"<h4>{heading}</h4>" if heading else ""
            return f'<div class="col">{head}<ul class="mods">{"".join(boxes)}</ul></div>'
        body = (column(0, "Today, on main") + column(1, "After its P1 units")) if changing else column(0, "")
        out.append(f'<details class="crate" {"open" if changing else ""}><summary><code>{escape(crate.name)}</code> '
                   f'<span class="meta">{escape(crate.dir)}, {size(crate.tokens, crate.lines)}</span></summary>'
                   f'<div class="cols{" two" if changing else ""}">{body}</div></details>')
    return "".join(out)


# The system: (id, label, detail, unit that makes it real, wide box, narrow box).
SYSTEM = [
    ("pages", "GitHub Pages", "Serves the web app's files. No backend.", "s3", (20, 20, 250, 60), (8, 8, 165, 60)),
    ("relay", "n0 relays", "Introduce peers; carry browser traffic.", "s3", (395, 20, 250, 60), (185, 8, 165, 70)),
    ("stun", "STUN servers", "Tell the host its public address.", "s3", (770, 20, 250, 60), (185, 490, 165, 60)),
    ("phone", "iPhone web app", "Dioxus on wasm, from the Home Screen.", "s3", (20, 170, 250, 66), (8, 130, 165, 66)),
    ("desktop", "Desktop app", "Dioxus desktop, native iroh.", "s3", (20, 330, 250, 60), (8, 490, 165, 60)),
    ("fleet", "Fleet root key", "Signs device certificates; pairs once for every host.", "fleet", (20, 430, 250, 60), (8, 580, 165, 76)),
    ("host", "conductord host", "Session actors, event log, iroh and WebRTC.", "s3", (650, 150, 370, 190), (8, 250, 342, 180)),
    ("claude", "Claude", "CLI over stream-json, one per session", "p1-4", (662, 230, 112, 94), (16, 322, 106, 92)),
    ("codex", "Codex", "app-server CLI, one per host", "p1-5", (781, 230, 112, 94), (126, 322, 106, 92)),
    ("sim", "Simulated", "agent in host.rs, the S3 stand-in", "p1-3", (900, 230, 112, 94), (236, 322, 106, 92)),
    ("hosts", "More hosts", "Windows, WSL, macOS in one view", "fleet", (650, 400, 175, 60), (185, 580, 165, 60)),
    ("push", "Web Push", "Approval alerts via Apple", "push", (845, 400, 175, 60), (185, 666, 165, 60)),
]
# (from, to, label, unit, both directions, label offset wide, label offset narrow)
SYSTEM_EDGES = [
    ("pages", "phone", "app files, once", "s3", False, (0, 0), (0, 0)),
    ("phone", "relay", "① iroh over relay", "s3", True, (-20, -10), (62, 8)),
    ("relay", "host", "relayed QUIC", "s3", True, (60, -16), (48, 0)),
    ("phone", "host", "② WebRTC data channel", "s3", True, (0, -8), (-10, 4)),
    ("desktop", "host", "iroh QUIC, direct", "s3", True, (0, 18), (-46, 0)),
    ("host", "stun", "binding request", "s3", False, (-56, 6), (52, 0)),
    ("desktop", "hosts", "fleet view", "fleet", True, (0, 14), (0, -2)),
]


def system_view(units):
    def status_of(unit, sim=False):
        st = units[unit]["status"]
        return ("delete" if st != "done" else None) if sim else st
    def layout(which, width, height):
        boxes = {n[0]: n[4 + which] for n in SYSTEM}
        out = []
        for a, b, label, unit, both, *offsets in SYSTEM_EDGES:
            lx, ly = offsets[which]
            st = status_of(unit)
            out.append(svg_edge(boxes[a], boxes[b], st, label, f"{label}: {LABEL[st].lower()}", both, lx, ly))
        for id, name, detail, unit, *_ in SYSTEM:
            st = status_of(unit, id == "sim")
            if st is None:
                continue
            x, y, w, h = boxes[id]
            chars = int((w - 20) / 6.6)
            rows = [(name, "name")] + [(t, "small") for t in wrap(detail, chars)[:1 if id == "host" else 4]]
            title = f"{name}: {detail} {LABEL[st]} ({units[unit]['name']})."
            out.append(svg_box(x, y, w, h, st, rows, title))
        return width, height, "".join(out)
    return two_svgs(layout(0, 1040, 500), layout(1, 358, 740), "The system: clients, relays and hosts")


LANES = ["Phone app", "Relay or WebRTC", "Host: serve", "Session", "Claude or Codex CLI"]
NARROW_LANES = ["Phone", "Relay/RTC", "serve", "Session", "CLI"]
# (from lane, to lane, message, unit that makes it real; None works today)
TURN = [
    (0, 1, "SendMessage { session, text }", None),
    (1, 2, "frame on the relayed stream, or the data channel", None),
    (2, 3, "registry finds the session; send(text)", "p1-3"),
    (3, 4, "user message on the CLI's stdin", "p1-4"),
    (4, 3, "streamed reply deltas", "p1-2"),
    (3, 0, "TextDelta events, with seq, from the log", None),
    (4, 3, "permission request for a tool call", "p1-4"),
    (3, 0, "ApprovalRequested", None),
    (0, 3, "Resolve { decision }", None),
    (3, 4, "control response: allow or deny", "p1-4"),
    (4, 3, "result: the turn ends", "p1-4"),
    (3, 0, "TurnEnded; the transcript is final", None),
]


def turn_view(units):
    def layout(width, lanes, narrow):
        pad = 8
        step = (width - 2 * pad) / len(lanes)
        xs = [pad + step * (i + 0.5) for i in range(len(lanes))]
        row = 46 if narrow else 38
        top = 46
        height = top + row * len(TURN) + 16
        out = [svg_text(x, 18, name, "lane", "middle") for x, name in zip(xs, lanes)]
        out += [f'<line class="lifeline" x1="{x:.1f}" y1="28" x2="{x:.1f}" y2="{height - 8}"/>' for x in xs]
        for i, (a, b, text, unit) in enumerate(TURN):
            y = top + row * i + (30 if narrow else 22)
            st = "done" if unit is None or units[unit]["status"] == "done" else units[unit]["status"]
            when = "works today" if unit is None else f"arrives with {short(units[unit])}"
            x1, x2 = xs[a] + (4 if b > a else -4), xs[b] - (6 if b > a else -6)
            label = f"{i + 1}. {text}" + ("" if unit is None else f" ({short(units[unit]).lower()})")
            if narrow:
                lab = svg_text(pad, y - 10, label, "elabel")
            else:
                lab = svg_text((x1 + x2) / 2, y - 7, label, "elabel", "middle")
            out.append(f'<g class="edge e-{st}"><title>{escape(text)}: {when}</title>'
                       f'<line x1="{x1:.1f}" y1="{y}" x2="{x2:.1f}" y2="{y}" marker-end="url(#arrow)"/>{lab}</g>')
        return width, height, "".join(out)
    return two_svgs(layout(1040, LANES, False), layout(358, NARROW_LANES, True), "A turn from the phone to the CLI and back")


def health(crates, status):
    ours = [c for c in crates.values() if c.in_subset]
    rest = [c for c in crates.values() if not c.in_subset]
    worst = max([c.violations for c in ours] + [1])
    bars = "".join(
        f'<li><code>{escape(c.name)}</code><span class="bar"><span class="fill{" zero" if not c.violations else ""}" '
        f'style="width:{100 * c.violations / worst:.1f}%"></span></span><span class="count">'
        f'{"0 ✓" if not c.violations else f"{c.violations} ▲"}</span></li>'
        for c in sorted(ours, key=lambda c: -c.violations))
    bugs = []
    for bug in status.get("bug", []):
        extra = f" {crates[bug['crate']].violations} violations today." if "crate" in bug else ""
        bugs.append(f'<li><span class="bug-icon" aria-hidden="true">!</span><div><strong>{escape(bug["title"])}</strong>'
                    f'<p>{escape(bug["detail"])}{extra}</p></div></li>')
    def tile(label, cs):
        return (f'<div class="tile"><div class="big">{num(sum(c.tokens for c in cs))}<small> tokens</small></div>'
                f'<div class="big2">{num(sum(c.lines for c in cs))}<small> lines</small></div><div class="meta">{label}</div></div>')
    return (f'<div class="tiles">{tile("crates/, written in the subset", ours)}'
            f'{tile("vendor/ and tools/subset, outside it", rest)}</div>'
            f'<div class="health-cols"><div><h3>Subset violations by crate</h3><ul class="bars">{bars}</ul>'
            f'<p><a href="../report/">The full subset report</a></p></div>'
            f'<div><h3>Known bugs</h3><ul class="bugs">{"".join(bugs)}</ul></div></div>')


CSS = """
:root{color-scheme:light;--page:#f6f6f4;--surface:#fcfcfb;--ink:#0b0b0b;--ink2:#52514e;--muted:#898781;--rule:#e1e0d9;
--axis:#c3c2b7;--st:#1c5cab;--st-soft:#e3eefc;--bad:#d03b3b;--warn:#b4501f;--good:#006300;--hatch:#cde2fb}
@media (prefers-color-scheme:dark){:root{color-scheme:dark;--page:#0d0d0d;--surface:#1a1a19;--ink:#f0efec;--ink2:#c3c2b7;
--rule:#2c2c2a;--axis:#383835;--st:#6da7ec;--st-soft:#15253b;--bad:#e66767;--warn:#ec835a;--good:#0ca30c;--hatch:#184f95}}
*{box-sizing:border-box}
body{margin:0;background:var(--page);color:var(--ink);font:15px/1.5 system-ui,-apple-system,"Segoe UI",sans-serif}
main{max-width:1088px;margin:0 auto;padding:32px 24px 64px}
code,.branch{font-family:ui-monospace,"SF Mono",Menlo,Consolas,monospace;font-size:.88em}
a{color:var(--st)}
h1{font-size:40px;line-height:1.1;letter-spacing:-.02em;margin:0 0 8px;font-weight:750}
h2{font-size:24px;letter-spacing:-.01em;margin:56px 0 4px;font-weight:700}
h3{font-size:16px;margin:0 0 10px}
h4{font-size:14px;margin:0 0 8px;color:var(--ink2);font-weight:600}
.lede{font-size:18px;max-width:62ch;margin:0 0 6px}
.meta,.small{color:var(--ink2);font-size:13px}
.sub{color:var(--ink2);max-width:70ch;margin:0 0 16px}
nav{display:flex;flex-wrap:wrap;gap:4px 16px;margin:16px 0 0;font-size:14px}
.legend{display:flex;flex-wrap:wrap;gap:6px 18px;margin:18px 0 0;padding:10px 0;border-top:1px solid var(--rule);border-bottom:1px solid var(--rule);font-size:14px}
.legend span{display:inline-flex;align-items:center;gap:6px}
.mark{flex:none;vertical-align:-2px}
.m-done,.m-in_progress{color:var(--st)}.m-planned{color:var(--muted)}.m-delete{color:var(--bad)}
.phase{display:grid;grid-template-columns:180px 1fr;gap:12px 24px;padding:18px 0;border-top:1px solid var(--rule)}
.phase h3{margin:4px 0 0}.tally{display:block;font-weight:400;font-size:13px;color:var(--ink2);margin-top:2px}
.cards{list-style:none;margin:0;padding:0;display:grid;grid-template-columns:repeat(auto-fill,minmax(210px,1fr));gap:10px}
.card{background:var(--surface);border:1.5px solid var(--st);border-radius:8px;padding:10px 12px}
.card-head{display:flex;gap:8px;align-items:center;line-height:1.25}
.card .done-when{margin:6px 0 0;font-size:13px;color:var(--ink2);line-height:1.4}
.branch{display:inline-block;margin:6px 0 0;padding:1px 6px;border-radius:4px;background:var(--surface);border:1px solid var(--st);font-size:12.5px}
.s-in_progress.card,.mod.s-in_progress{border-width:2px;background:repeating-linear-gradient(135deg,var(--surface) 0 6px,var(--st-soft) 6px 9px)}
.s-planned.card,.mod.s-planned{border:1.5px dashed var(--muted);background:transparent}
.s-planned.card strong,.mod.s-planned code{color:var(--ink2)}
.tiles{display:flex;flex-wrap:wrap;gap:12px 40px;margin:12px 0 20px}
.big{font-size:34px;font-weight:700;letter-spacing:-.02em;line-height:1.1}.big2{font-size:20px;font-weight:600}
.big small,.big2 small{font-size:14px;font-weight:400;color:var(--ink2)}
.health-cols{display:grid;grid-template-columns:1fr 1fr;gap:24px 40px}
.bars{list-style:none;margin:0 0 8px;padding:0}
.bars li{display:grid;grid-template-columns:70px 1fr 56px;gap:10px;align-items:center;font-size:14px;padding:3px 0}
.bar{height:10px;border-radius:2px;background:var(--rule);overflow:hidden}
.fill{display:block;height:100%;background:var(--warn);border-radius:0 2px 2px 0}.fill.zero{width:0}
.count{text-align:right;font-variant-numeric:tabular-nums}
.bugs{list-style:none;margin:0;padding:0}.bugs li{display:flex;gap:10px;margin:0 0 12px}.bugs p{margin:2px 0 0;font-size:14px;color:var(--ink2)}
.bug-icon{flex:none;width:20px;height:20px;border-radius:50%;background:var(--bad);color:#fff;font-weight:800;font-size:13px;display:grid;place-items:center;margin-top:1px}
svg.wide,svg.narrow{width:100%;height:auto;display:block;background:var(--surface);border:1px solid var(--rule);border-radius:10px}
svg.narrow{display:none}
@media (max-width:760px){svg.wide{display:none}svg.narrow{display:block}}
svg text{fill:var(--ink);font-size:12.5px}
svg .name{font-weight:700;font-size:13.5px;font-family:ui-monospace,"SF Mono",Menlo,Consolas,monospace}
svg .small{fill:var(--ink2);font-size:12px}svg .v{fill:var(--warn)}svg .now{fill:var(--st);font-weight:600}
svg .lane{font-weight:650;font-size:12.5px}
svg .elabel{fill:var(--ink2);font-size:12px;paint-order:stroke;stroke:var(--surface);stroke-width:4px;stroke-linejoin:round}
.box rect{fill:var(--surface);stroke:var(--st);stroke-width:1.5}
.box.s-in_progress rect{fill:url(#hatch);stroke-width:2}
.box.s-planned rect{fill:var(--page);stroke:var(--muted);stroke-dasharray:6 4}
.box.s-planned .name{fill:var(--ink2)}
.box.s-delete rect{fill:var(--surface);stroke:var(--bad);stroke-dasharray:3 3}
.box.s-delete .name{text-decoration:line-through}
.hatch-bg{fill:var(--surface)}.hatch-line{stroke:var(--st-soft)}
.edge line,.edge path{stroke:var(--st);stroke-width:1.6}
.e-in_progress line,.e-in_progress path{stroke-dasharray:7 4}
.e-planned line,.e-planned path{stroke:var(--muted);stroke-dasharray:4 4}
.e-dev path{stroke:var(--muted);stroke-dasharray:1 3;stroke-linecap:round}
.lifeline{stroke:var(--axis);stroke-width:1}
.key{display:flex;flex-wrap:wrap;gap:6px 22px;font-size:13px;color:var(--ink2);margin:10px 0 0}
.key svg{vertical-align:middle}
details.crate{border-top:1px solid var(--rule);padding:10px 0}
details.crate summary{cursor:pointer;font-size:16px;font-weight:600}
details.crate summary .meta{font-weight:400;margin-left:6px}
.cols{margin-top:12px}.cols.two{display:grid;grid-template-columns:1fr 1fr;gap:20px}
.mods{list-style:none;margin:0;padding:0;display:grid;grid-template-columns:repeat(auto-fill,minmax(160px,1fr));gap:8px}
.mod{background:var(--surface);border:1.5px solid var(--st);border-radius:6px;padding:7px 9px;font-size:13px;min-width:0}
.mod.s-delete{border:1.5px dashed var(--bad)}.mod.s-delete code{text-decoration:line-through}
.mod-head{display:flex;gap:6px;align-items:center}.mod-head code{font-weight:600;overflow-wrap:anywhere}
.mod .meta{font-size:12px}.mod .v{color:var(--warn)}.mod .ok{color:var(--good)}.mod .note{margin-top:3px;line-height:1.35}
.mod .unit{color:var(--st);font-weight:600}
footer{margin-top:56px;padding-top:16px;border-top:1px solid var(--rule);font-size:14px;color:var(--ink2)}
@media (max-width:760px){main{padding:20px 16px 48px}h1{font-size:30px}.phase{grid-template-columns:1fr}
.health-cols,.cols.two{grid-template-columns:1fr}.cards{grid-template-columns:1fr 1fr;gap:8px}
.card{padding:8px 10px}.card:not(.s-done){grid-column:1/-1}.card.s-done .done-when{display:none}.card.s-done strong{font-size:14px}}
"""


def page(status, units, crates, commit, date):
    counts = {s: sum(u["status"] == s for u in units.values()) for s in STATUSES[:3]}
    legend = "".join(f"<span>{mark(s)}{LABEL[s]}</span>" for s in STATUSES)
    docs = [("docs/architecture.md", "Architecture"), ("docs/p1/design.md", "P1 design"),
            ("docs/subset.md", "The subset"), ("docs/status.toml", "Status file")]
    doc_links = ", ".join(f'<a href="{REPO}/blob/{commit}/{p}">{t}</a>' for p, t in docs)
    edge_key = (f'<div class="key"><span><svg width="40" height="10"><line x1="2" y1="5" x2="38" y2="5" class="kl done"/></svg> works today</span>'
                f'<span><svg width="40" height="10"><line x1="2" y1="5" x2="38" y2="5" class="kl wip"/></svg> being built</span>'
                f'<span><svg width="40" height="10"><line x1="2" y1="5" x2="38" y2="5" class="kl plan"/></svg> planned</span></div>')
    sections = [
        ("roadmap", "Roadmap", "Every unit of work, by phase. Hover a card for its purpose and the crates it touches.", roadmap(status, units)),
        ("health", "Health", "Measured on this commit by tools/subset.", health(crates, status)),
        ("system", "System", "What runs where. Dashed boxes are planned; the struck box is deleted by P1 unit 3.", system_view(units) + edge_key),
        ("crates", "Crates", "Arrows point at dependencies. The dotted arrow is a test-only dependency; the dashed one arrives with P1.",
         two_svgs(crate_graph(crates, units, 1040, 4), crate_graph(crates, units, 358, 2), "Crates and their dependencies")),
        ("inside", "Inside each crate", "Modules as files. Crates that P1 changes show today next to the target.", inside(crates, status, units)),
        ("turn", "A turn, end to end", "Solid steps work today, with the simulated agent in host.rs playing the session and the CLI. Dashed steps arrive with the unit named.", turn_view(units) + edge_key),
    ]
    nav = "".join(f'<a href="#{id}">{title}</a>' for id, title, _, _ in sections)
    body = "".join(f'<section id="{id}"><h2>{title}</h2><p class="sub">{escape(sub)}</p>{html}</section>' for id, title, sub, html in sections)
    return f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Conductor architecture</title><style>{CSS}
.kl{{stroke-width:2}}.kl.done{{stroke:var(--st)}}.kl.wip{{stroke:var(--st);stroke-dasharray:7 4}}.kl.plan{{stroke:var(--muted);stroke-dasharray:4 4}}
</style></head><body>{DEFS}<main>
<header><h1>Conductor architecture</h1>
<p class="lede">{counts["done"]} units built, {counts["in_progress"]} being built, {counts["planned"]} planned.</p>
<p class="meta">Generated at <a href="{REPO}/commit/{commit}"><code>{commit[:9]}</code></a> on {date} from docs/status.toml (updated {status["updated"]}) and the code. Docs: {doc_links}.</p>
<nav>{nav}</nav><div class="legend">{legend}</div></header>
{body}
<footer>Regenerate with <code>python3 tools/archmap/archmap.py --out DIR</code>. Edit <code>docs/status.toml</code> when a unit starts or merges.</footer>
</main></body></html>
"""


def main():
    args = argparse.ArgumentParser()
    args.add_argument("--out", required=True, type=Path)
    args.add_argument("--subset", default=str(Path(run("tools/target-dir").strip()) / "debug/subset"))
    args = args.parse_args()
    status, units = load_status(ROOT / "docs/status.toml")
    crates = load_crates(status, args.subset)
    commit = run("git", "rev-parse", "HEAD").strip()
    date = datetime.date.today().isoformat()
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / "index.html").write_text(page(status, units, crates, commit, date))
    print(args.out / "index.html")


if __name__ == "__main__":
    main()
