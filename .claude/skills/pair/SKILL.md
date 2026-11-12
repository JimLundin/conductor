---
name: pair
description: Implement Conductor code as a pair, with an implementer writing in the Rust subset and a reviewer who checks it for correctness, subset compliance and needless volume. Use for any change to code under `crates/`.
---

# Pair loop

Conductor code is written in the Conductor Rust subset. The rules are in
`docs/subset.md` of `JimLundin/subset`; `subset.toml` here is the allowlist
and `tools/subset check` enforces it. Every change to `crates/` runs as a
pair: one agent implements, a second reviews. The reviewer gets the diff
and the friction log, never the implementer's reasoning, so the review
isn't anchored on the implementer's choices. The coordinator (you) runs
the steps, owns the branch, and talks to the owner.

## 1. Implement

Give the implementer the task, the subset rules, `subset.toml`, and the
files it owns. It works in its own worktree.

- It writes the code and tests using only allowed constructs.
- It writes the least code that does the task: no options, layers or
  variants for cases the task doesn't have.
- Where a missing construct makes the code longer, it writes the code the
  long way and records a **friction point** in `.audit/<task>.friction.md`:
  file and line, what it wanted to write, and what it wrote instead. It
  never works around the subset with an unlisted construct, a macro, or
  code moved outside `include`.

**Done when** `tools/check.sh` and the task's own verification pass, and
every friction point is logged.

## 2. Review

Give the reviewer the diff, the friction log and the subset rules.

1. **Correctness.** States that can't happen, lost events, races in the
   actors, error paths that drop information.
2. **Volume.** Code the task doesn't need: speculative options, duplicated
   shapes the subset can already express once, layers that only rename.
   Each is a required fix.
3. **Compliance.** `tools/subset check` passes and the code keeps the
   subset's qualities. A breach is a required fix.

The implementer applies the required fixes; the coordinator merges.

## 3. Friction goes to the subset repository

The ratchet (measured proposals for new constructs) is the subset
project's work, not part of each Conductor change. At the end of a
milestone, the coordinator collects the friction logs and files them in
`JimLundin/subset`, where proposals are written, measured and decided. An
accepted construct reaches Conductor when `tools/subset` moves its pinned
revision and `subset.toml` gains the id.

## 4. Clean up

When the branch is merged (or abandoned), remove the worktrees and their
branches (`git worktree remove ../conductor-<task>`, `git branch -d
<branch>`) and any scratch run logs. Each worktree builds in its own
`./target`, so removing it frees most of the disk a round used.

**Done when** `git worktree list` shows only the main checkout and the
worktrees of tasks still in progress.
