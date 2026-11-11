---
name: pair
description: Implement Conductor code as a pair, with an implementer writing in the Rust subset and a reviewer who checks it and proposes new subset constructs backed by measurements. Use for any change to code under `crates/`, including rewriting existing code into the subset.
---

# Pair loop

Conductor code is written in a Rust subset that grows on evidence
(`docs/subset.md`; `subset.toml` is the source of truth). Every task in the
subset runs as a pair: one agent implements, a second reviews. The reviewer's
second job is the **ratchet**: finding constructs that would make the code
shorter without costing any of the subset's qualities, and proving it.

Use two fresh subagents, both on Opus (the owner's choice for now). The
reviewer gets the diff and the friction log, never the implementer's
reasoning, so the review isn't anchored on the implementer's choices. The coordinator (you) runs the steps, owns the
branch, and talks to the owner.

## 1. Implement

Give the implementer the task, `docs/subset.md`, `subset.toml`, and the
files it owns. It works in its own worktree.

- It writes the code and tests using **only** allowed constructs, and adds
  the new files to `include` in `subset.toml`.
- Where a missing construct makes the code longer or more roundabout, it
  writes the code the long way anyway and records a **friction point** in
  `.audit/<task>.friction.md`: file and line, what it wanted to write, and
  what it wrote instead. It never works around the subset with an unlisted
  construct, a macro, or code moved outside `include`.

**Done when** `tools/subset check`, `tools/check.sh` and the task's own
verification all pass, and every friction point is logged.

## 2. Review

Give the reviewer the diff, the friction log, `docs/subset.md`,
`subset.toml`, and `docs/subset/proposals/` (including the rejected ones).

1. **Compliance.** Confirm that `tools/subset check` passes and that the
   code honours each of the seven qualities in `docs/subset.md`. A breach
   is a required fix, not a proposal. Once `subset repeats` exists, run it
   on the change: repeated shapes the subset can already express as data
   are required fixes, and those that need a new construct become
   proposals. A group of tests becomes a table only at **4 or more
   cases**. At three, the named struct that 0001 requires costs about 20%
   more than it saves, so three similar tests are not a breach.
2. **Ratchet.** Pick the candidate constructs from the friction log and
   from patterns repeated in the diff. For each candidate:
   - Rewrite the affected code with the construct, in a scratch copy.
   - Run `tools/subset score` and `tools/subset measure` on the before and
     after versions of the same files.
   - Judge each quality as kept or weakened, with a one-line reason.
   - Choose the tier the construct would take in `[weights]`, and whether
     its boundary use applies (`docs/subset.md`, Weights).

   A candidate becomes a proposal when it **keeps all seven qualities**,
   and either the **weighted score** of the affected code drops by at
   least 15% or it removes at least three friction points of one kind.
   Score the after version with the proposed tier. Skip any candidate
   already rejected for the same reason.
3. **Weights.** Where the reviewed code shows that an existing weight is
   wrong, for example a tolerated construct that the code needs everywhere
   or a neutral one that hides a cost, propose changing it as a proposal
   of its own, with the evidence from the code and the score before and
   after. The owner decides it as for a construct.
4. **Write each proposal** to `docs/subset/proposals/NNNN-<construct-id>.md`
   using the template below.

**Done when** every friction point has been considered for the ratchet:
either it is covered by a proposal, or the review gives the reason it
isn't (volume too small, a quality weakened, or already rejected).

## 3. Decide

Bring the proposals to the owner with the score, the tokens and the tier
in one line each. The owner accepts or rejects each one.

- **Accepted:** add the id (and any `derives` / `macros` entry) to
  `subset.toml` and its tier to `[weights.ids]`, increase `version`, add
  its row and rule to `docs/subset.md`, and teach `tools/subset check` the
  construct if it doesn't know it yet. Then apply the reviewer's rewrite
  to the code. For an accepted weight change, edit the tier in
  `[weights.ids]` and the Weights table in `docs/subset.md`.
- **Rejected:** set `status: rejected` in the proposal, with the owner's
  reason.

The coordinator never accepts a proposal itself.

## 4. Clean up

When the implementer's branch and the review branch are merged (or
abandoned), the coordinator removes both worktrees and their branches
(`git worktree remove ../conductor-<task>`, `git branch -d <branch>`), and
any scratch worktree or `/tmp` run logs the round created. Each worktree
builds in its own `./target` (`AGENTS.md`), and removing the worktree removes
that build output, which is most of the disk a round uses.

**Done when** `git worktree list` shows only the main checkout and the
worktrees of rounds still in progress.

## Proposal template

```markdown
---
construct: <id, e.g. expr.let_else>
status: proposed | accepted | rejected
task: <the task that surfaced it>
---

# <construct in plain words>

## Rule
When code may use it, in one or two sentences. Narrow beats broad.

## Before
<the affected code as written in the current subset>

## After
<the same code using the construct>

## Weight
<tier: encouraged | neutral | tolerated | costly>. <one-line reason>
Boundary use: applies | doesn't apply. <one line: where a library fixes
the construct's shape, if anywhere>

## Measurements
| | score | tokens | lines |
|---|---|---|---|
| before | | | |
| after | | | |
| change | | | |

The after score uses the proposed tier.

Friction points removed: <n> (<log references>)

## Qualities
1. Named states: kept | weakened. <reason>
2. Visible branching: …
3. Forward data flow: …
4. Errors are values: …
5. Local reasoning: …
6. One way: …
7. Repetition is data: …

## Owner decision
<reason, filled in when decided>
```
