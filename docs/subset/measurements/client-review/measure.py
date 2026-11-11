"""Reprints the numbers in docs/subset/reviews/client.md.

Run from the repository root, after `cargo build -p subset`:

    python3 docs/subset/measurements/client-review/measure.py [SCORE_TOOL SCORE_CONFIG]

It checks out 4a29f1f into two temporary worktrees, applies
0035-endpoints.patch to one and 0008-loop-owned.patch to the other, and
measures the touched items on both sides with `subset measure`. With the two
optional arguments (the `subset` binary and `subset.toml` from branch
`subset-score`), it also prints the weighted score, counting the new `&mut`
sites as costly (3.0) where the tool reports them as outside (5.0).
"""
import json, os, subprocess, sys, tempfile

ROOT = os.getcwd()
HERE = os.path.dirname(os.path.abspath(__file__))
TOOL = os.path.join(ROOT, "target/debug/subset")
BASE = "4a29f1f"
SCORE = sys.argv[1:3] if len(sys.argv) == 3 else None

ns = {"__file__": os.path.join(ROOT, ".audit/rw-client.friction/measure.py")}
src = open(ns["__file__"]).read().replace("\nmain()\n", "\n")
exec(compile(src, "friction-measure", "exec"), ns)
cut = ns["cut"]

W, L, R, N = (
    "crates/client/src/wire.rs", "crates/client/src/link.rs",
    "crates/client/src/platform/rtc.rs", "crates/client/src/platform/native.rs",
)
F, U = "crates/proto/src/frame.rs", "crates/client/src/upgrade.rs"

SETS = {
    "0035 client": ("0035-endpoints.patch", [
        (W, r"^impl SendHalf"), (W, r"^impl RecvHalf"), (W, r"^fn decode"),
        (W, r"^async fn read_iroh\("), (W, r"^async fn read_iroh_body"),
        (L, r"^async fn subscribe"), (L, r"^async fn hello"), (L, r"^fn frames"), (L, r"^async fn rpc"),
        (R, r"^fn signals"), (R, r"    async fn send\("), (R, r"    fn stop\("),
        (R, r"^pub\(crate\) struct ChannelRecv"), (R, r"^enum ReadStep"), (R, r"^impl ChannelRecv"),
        (N, r"^impl ChannelRecv")]),
    "0035 FrameBuffer": ("0035-endpoints.patch", [(F, r"^pub enum Reassembly"), (F, r"^impl FrameBuffer")]),
    "0008 loop-owned (b+)": ("0008-loop-owned.patch", [
        (U, r"^impl Upgrade"), (L, r"    async fn on\("), (L, r"    fn armed"), (L, r"    fn arm\("),
        (L, r"    async fn carry_on"), (L, r"    async fn carry\("), (L, r"    async fn upgraded"),
        (L, r"    async fn fall_back"), (L, r"^impl Connected")]),
}


def run(*args):
    return subprocess.run(args, capture_output=True, text=True, check=True).stdout


def fragment(root, items):
    return "\n".join(cut(open(os.path.join(root, f)).read(), pat, True) for f, pat in items)


def numbers(text):
    d = tempfile.mkdtemp()
    path = os.path.join(d, "crates", "client", "src")
    os.makedirs(path)
    file = os.path.join(path, "x.rs")
    open(file, "w").write(text)
    total = json.loads(run(TOOL, "measure", file, "--json"))["total"]
    out = (total["tokens"], total["lines"])
    if SCORE:
        score = json.loads(run(SCORE[0], "score", file, "--config", SCORE[1], "--json"))["total"]["score"]
        rows = json.loads(run(SCORE[0], "score", file, "--config", SCORE[1], "--json", "--by", "construct"))["rows"]
        outside = sum(r["tokens"] for r in rows if r.get("tier") == "outside")
        out += (score - 2 * outside,)
    return out


def worktree(patch):
    d = tempfile.mkdtemp()
    run("git", "worktree", "add", "--detach", d, BASE)
    if patch:
        run("git", "-C", d, "apply", os.path.join(HERE, patch))
    return d


def main():
    base = worktree(None)
    trees = {}
    try:
        for name, (patch, items) in SETS.items():
            tree = trees.setdefault(patch, worktree(patch))
            b, a = numbers(fragment(base, items)), numbers(fragment(tree, items))
            line = f"{name:22} tokens {b[0]:5} -> {a[0]:5} ({(a[0] - b[0]) * 100 / b[0]:+.1f}%)  lines {b[1]} -> {a[1]}"
            if SCORE:
                line += f"  score {b[2]:.1f} -> {a[2]:.1f} ({(a[2] - b[2]) * 100 / b[2]:+.1f}%)"
            print(line)
        for patch, tree in trees.items():
            crate = run(TOOL, "measure", os.path.join(tree, "crates/client")).splitlines()[-1]
            print(f"crates/client with {patch}: {crate}")
        print(f"crates/client at {BASE}: {run(TOOL, 'measure', os.path.join(base, 'crates/client')).splitlines()[-1]}")
    finally:
        for d in [base, *trees.values()]:
            run("git", "worktree", "remove", "--force", d)


main()
