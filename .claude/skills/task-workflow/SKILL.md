---
name: task-workflow
description: Branch-verify-merge workflow for this repo. Use when starting a new task or unit of work, when finishing one, or when about to commit or merge. Covers branching from dev, the full verification gate (fmt, clippy, cargo tests, the domain boundary check), and merging back into dev.
---

# Task Workflow

Every unit of work is a branch off `dev`, verified before it lands, merged back
into `dev` when green. `dev` is the integration branch and must always build.

This repo has **no remotes** — everything is local. There is no push step, and
no PR. `master` exists but is not part of this workflow; `dev` is where work
integrates.

## 1. Start: branch from dev

```bash
git switch dev
git switch -c <branch-name>
```

Branch names describe the *unit of work*, matching the existing style
(`domain-primitives`): lowercase, hyphenated, no prefix, named for the thing
being built rather than the activity. `stripe-adapter`, `webhook-verification`,
`billing-provider-port` — not `feature/x` or `wip`.

Confirm you actually branched from an up-to-date `dev`:

```bash
git rev-list --left-right --count dev...HEAD
```

The left number must be `0`. If it isn't, `dev` has moved on and this branch
needs rebasing before it can merge cleanly.

## 2. Work: commit as you go

Commit at meaningful boundaries, not once at the end. The repo's convention,
visible in `git log`:

- **Category tag first**: `[Fix]`, `[Feature]`, `[Refactor]`, `[Remove]`,
  `[Docs]`, `[Test]`, etc.
- Then an imperative subject, no trailing period —
  `[Test] Add OutboundRequest repository integration tests`
- Blank line, then a prose body saying **what** and **why**, wrapped at ~72
  columns
- A `Co-Authored-By:` trailer. **Do not add `Claude-Session:`.**

Commits before `dev`'s current tip predate the category tag, so `git log` shows
bare subjects (`Add OutboundRequest repository integration tests`). Tag new
commits; do not rewrite old ones.

The existing history is one commit per coherent step (aggregate → migration →
adapter → tests). Keep that granularity — it is what makes the work reviewable
and explainable in the defense.

## 3. Gate: verify before finishing

**Run all four. Never merge on a red gate.** Ordered fastest-failing first, so
a break surfaces in seconds rather than minutes.

```bash
# 1. Domain boundary — the load-bearing architectural invariant
grep -E "sqlx|axum|stripe" crates/domain/Cargo.toml

# 2. Formatting
cargo fmt --all --check

# 3. Lints (also compiles everything, including tests)
cargo clippy --workspace --all-targets

# 4. Tests
cargo test --workspace
```

**Step 1 must produce no output.** `domain/Cargo.toml` gaining `sqlx`, a Stripe
client, or `axum` breaks the separation the whole design rests on. It is a
one-second check and it is the single most checkable proof of the architecture,
so it runs every time rather than being assumed.

**Step 3 is not advisory.** `unwrap_used`, `expect_used` and `panic` are
`deny` at the workspace level. Clippy failing means the code does not meet
a stated standard, not that a nitpick is available.

**Step 4 needs Docker.** The `persistence` tests bring up a disposable Postgres
via `testcontainers`. Check first, so a failure reads as "Docker is down"
rather than as a mysterious test error:

```bash
docker info --format "{{.ServerVersion}}"
```

If Docker is unavailable, say so plainly and run `cargo test -p domain`
instead — but the gate is **not** satisfied, and that has to be stated rather
than glossed. A task is not finished on an unverified persistence layer.

### Known pre-existing noise

`missing_docs` is set to `warn` workspace-wide, which also hits integration-test
binaries and `demo`'s `main.rs` — neither has a crate-level doc comment. Those
warnings predate this workflow. Do not treat them as a new break, and do not
"fix" them by suppressing the lint; either add `//!` headers deliberately or
narrow the lint's scope as its own task.

## 4. Finish: merge into dev

```bash
git switch dev
git merge --no-ff <branch-name>
```

**`--no-ff` is deliberate.** `dev` is currently an ancestor of the working
branch, so a default merge would fast-forward and the task would dissolve into
a flat run of commits. `--no-ff` creates a merge commit that keeps the task
visible as a unit — which is what makes "what landed in this task?" answerable
later, and gives a revert a single target. There are no merge commits in the
history yet; this establishes the pattern.

Then confirm `dev` is genuinely green — the merge itself can introduce breakage
that neither branch had:

```bash
cargo clippy --workspace --all-targets && cargo test --workspace
```

## Rules

- **Never merge a red gate.** Fix it on the branch, or say explicitly that the
  task is not done.
- **Never report a skipped step as passed.** If Docker was down and the
  persistence tests did not run, that is part of the result.
- **Do not merge without being asked**, unless the user has said to run the
  whole workflow through. Merging is the irreversible-ish step; confirm it.
- **`dev` always builds.** If something lands broken, fixing `dev` takes
  priority over starting the next task.
