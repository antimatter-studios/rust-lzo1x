# Working in rust-lzo1x (agent guide)

A pure-Rust LZO1X compressor and decompressor, published as `rust-lzo1x`: no C
bindings, no `unsafe` (`#![forbid(unsafe_code)]`), no dependencies. One decoder
handles every LZO1X-\* variant, because they share one decode grammar; the
encoder emits a deliberate subset of it. A small CLI works on raw blocks. The
input is untrusted — filesystem readers hand it blocks lifted straight off an
image — so every step is bounds-checked and a corrupt stream is an error, never
a panic or an unbounded allocation.

This file is the fast path for an agent picking up work here. It points at the
existing docs rather than duplicating them:

- **README** → `## Scope`, `## Provenance`, `## How the encoder is checked`,
  `## Test contract` (and `### The tiers, quietly`), `## Verifying a release`.
- **`chores.yml`** → every task, and the measured budgets and floors in the
  table at its top.
- **CHANGELOG.md** → what each release contains, and `[Unreleased]`.
- **`.github-guard`** → the required checks: `ci-ok`, `test-ubuntu-latest`,
  `test-macos-latest`.

The section between the BEGIN/END markers below is **shared, byte-identical,
with every repository in this family**. Do not edit it here: change the
canonical copy and propagate it, or `scripts/agents-core-check.sh` will fail.
Everything after the END marker is specific to this repository.

<!-- BEGIN SHARED BLOCK: agent-core v2 sha256:38af4d2c5377d38ab382baa4eab4aa679841e2b4eba4f4d01dacd255ffa7d32e -->
## Claiming work

Several agents work these repositories at the same time. Before you start on
an issue, claim it, so nobody else spends a session on what you are already
doing. The lock is a **GitHub label**, because labels are shared state that
every agent can read and change without posting comments into the thread.

**Before starting.** Check, claim, then read back:

```sh
gh issue view <N> --json labels                      # holds `claimed`? pick another
gh issue edit <N> --add-label claimed --add-label claim/<session>
gh issue view <N> --json labels                      # read back and confirm
```

`<session>` is your session name — `agent-<random4>-<isodate>`, e.g.
`agent-3f7c-2026-09-22`. Create the `claim/<session>` label if it does not
exist.

**Resolving a race.** Adding a label is not compare-and-swap: two agents can
both add `claimed` and both believe they won. That is what the read-back is
for. If it shows more than one `claim/*` label, the **lexically lowest**
session keeps the issue; every other agent removes its own `claim/*` label and
picks different work. Each racer computes the same answer independently, so no
further coordination is needed.

**When you finish or stop.** Remove both labels — on merge, or the moment you
abandon the work:

```sh
gh issue edit <N> --remove-label claimed --remove-label claim/<session>
```

Delete your `claim/<session>` label from the repository at the end of your
session so they do not accumulate.

**Reclaiming a stale claim.** An agent that dies holding a claim would block an
issue forever. If `claimed` was applied more than 12 hours ago and the holder's
branch has no commits since, any agent may take it: remove the stale `claim/*`,
add your own, and say so in the issue.

**This is a convention, not a fence.** Nothing enforces it. An agent that
ignores it duplicates work; it cannot corrupt anything. Honour it anyway.

## Work in a worktree

Every working copy is a **git worktree** of an existing checkout, made with
`git worktree add`. Never `git clone` a second, unlinked copy — not for a
branch, a PR, a review, or a sibling you need at another ref:

```sh
git -C <checkout> fetch origin
git -C <checkout> worktree add <path> -b <type>/<name> origin/main   # new work
git -C <checkout> worktree add --detach <path> <tag>                 # a sibling at a pinned ref
git -C <checkout> worktree remove <path>                             # when done
```

A worktree shares the checkout's objects and remotes, and `git worktree list`
shows it to every agent on the machine, so nobody else mistakes it for
abandoned work or loses track of it. An unlinked clone copies all the history
again, is invisible to that list, and gets left behind in `/tmp` long after the
work that made it is merged. Remove your worktree when you finish.

## Skills to use

- **`dev-loop`** — the required loop for any non-trivial change: baseline the
  full suite → change → re-run (no baseline test may regress) → enhance tests →
  vet. Always run it.
- **`commit`** / **`pr`** — for grouping commits and opening pull requests.

Each repository names any further skills of its own below.

## A bug fix starts with a red

**Prove it is broken first** — a failing check or test — *then* fix it, *then*
prove that same check is green, *then* confirm the full baseline still passes.
Never write the fix before you have a red. A fix with no failing test to its
name is a claim, not a result.

## Nothing skips

A test that cannot run **fails**, naming the task that would provide what it
needed. Never add an early return for a missing fixture, tool or VM: a skipped
test reads exactly like a passing one, and a suite that quietly declines to run
is indistinguishable from a suite that passes.

Where a tier reports skips or ignored tests, that is a gate, not a note.

## Validate against something that is not us

A driver's own readers share its interpretation of the format, so they cannot
catch a misreading: the mistake is baked into the fixture *and* the parser, and
they agree with each other while disagreeing with every real filesystem. Unit
tests over self-built fixtures prove self-consistency, not correctness.

Every structure that is parsed or written gets a cross-validation test against
an **independent oracle** — the platform's own tools, a real kernel, or a third
implementation — before it is considered done. Each repository names its
oracles below.

## Output is budgeted

Test tiers run through `scripts/tier.sh`, which runs the suite **quietly**: the
whole run goes to `tmp/logs/<tier>.log`, a pass prints one verdict line naming
that log, and a failure prints the verdict, the command's status and the log's
path — `--tail N`, or `OUTPUT_BUDGET_FAIL_TAIL=N`, prints the tail for whoever
is watching. **Read the log**: a failing tier names it and does not recite it.
CI keeps the logs as an artifact, so the detail is always retrievable.

The budget caps the log, not merely what is shown, and every number in the
table was measured. A run that passes but prints more than its budget **fails**.

The reader who pays most for a noisy suite is an agent that re-reads its whole
transcript on every step, and so pays for one loud run many times over. If a
tier legitimately grows, raise its row **with the measurement that justifies
it**. Do not silence output to fit, and do not route around `tier.sh`.

## Commits and branches

- Branches are `<type>/<name>`, matching the commit type: `fix/`, `feat/`,
  `ci/`, `docs/`, `chore/`, `test/`.
- A commit is a subject plus flat one-sentence bullets. Subjects are
  declarative, not imperative: "the run-end bound is checked", not "check the
  run-end bound".
- **No AI attribution and no co-author trailers**, in commits or in pull
  request descriptions.
- `main` takes **squash merges only**.

## Project rules

- **No GPL/LGPL/AGPL dependencies.** Permissive only (MIT/BSD/Apache).
  Shelling out to a copyleft CLI as a *test oracle* is fine — linking or
  copying it is not.
- **Each of these is a standalone project.** Never mention a consuming
  application in the README, the source, or CLI help.
<!-- END SHARED BLOCK: agent-core v2 -->

## The oracle is the reference LZO compressor, `lzop`

Round-tripping through our own decoder proves very little: that decoder is
deliberately more permissive than the format in at least one place, so an
encoder checked only against it could emit a stream this crate reads back
perfectly and every other implementation refuses. `tests/oracle_lzop.rs`
checks **both directions** against `lzop`, run as an external process:

- `lzop` compresses (`-1` and `-9`) → we decompress, and the payload matches;
- we compress → `lzop` decompresses, and the payload matches.

Those tests are `#[ignore]`-gated so a fresh `cargo test` is green without
`lzop`; CI installs it and runs `-- --ignored`, in both profiles. `chore tools`
**fails** when `lzop` is absent — the oracle is the only check here that is not
this crate marking its own homework. `lzop` itself only speaks its own
container, so the tests translate between it and the raw blocks this crate
reads and writes.

## Clean room

The widely used LZO implementations are GPL, and the `lzo1x` name on
crates.io belongs to one of them — that is why this crate is `rust-lzo1x`. The
decoder was written from the published **prose** description of the grammar
(reproduced in the crate docs so the mapping is auditable); the encoder is this
crate's own decoder run backwards. **Do not read, port or paste from any other
LZO implementation's source**, however convenient; running `lzop` as a
separate process is the only contact allowed. A change to the codec cites the
grammar, not somebody's code.

## Running tests

```sh
chore siblings         # ../rust-fs-core, which owns the output-budget wrapper
chore tools            # lzop is present, or a failure naming it
chore test             # every tier, exactly as CI runs them
chore test:debug       # one tier (release, release:oracle, debug, debug:oracle)
chore test:scripts     # tests/scripts/*.sh, by glob
chore lint             # the agent-core check, fmt, clippy -D warnings
```

`scripts/tier.sh` maps directly onto "Output is budgeted": it resolves
rust-fs-core's `scripts/output-budget.sh` from `$FS_CORE_ROOT` or the
`../rust-fs-core` sibling, verifies it by `--version`, and **keeps no copy
here**. The floor is rust-fs-core's too: `scripts/core.sh test-floor TIER N`
refuses a tier that executed fewer tests than its floor, and
`scripts/core.sh semver-check` runs the semver tier; `scripts/core.sh
family-check` fails CI if a copy of either is ever committed here. The
budgets and floors are written twice — `chores.yml` and
`.github/workflows/ci.yml` — and `tests/scripts/test-tier-budgets-agree.sh`
fails a pull request where the two disagree: raise a number in both or in
neither, with the measurement that justifies it.

CI (`.github/workflows/ci.yml`): one `test` job on `ubuntu-latest` and
`macos-latest` — fmt, clippy, every `tests/scripts/*.sh` (this includes the
agent-core self-test), then the four tiers — and `ci-ok`, which fails unless
the matrix concluded exactly `success`. `fuzz.yml` is the nightly explorer; its
deterministic half, `tests/fuzz_decoders.rs`, replays `fuzz/corpus/` on every
pull request, and a new finding's reproducer belongs in that corpus.

## Things a newcomer trips on

- **The debug run is not redundant.** `[profile.release]` does not enable
  overflow checks, on purpose, so only the debug tier can see arithmetic wrap
  in the decode loop — and in a decoder that arithmetic *is* the parser.
  `EXPECT_OVERFLOW_CHECKS=1` on that one CI step arms the probe in `src/lib.rs`
  that proves the build really traps; `tests/ci_profile.rs` fails if the step
  or its variable goes away.
- **`max_out` is a bound, not the length.** The true output length comes from
  the stream's own end marker; the bound only stops a corrupt stream driving
  an unbounded allocation. Raw blocks carry no size, which is why the CLI's
  `decompress` takes one.
- **`compress` is infallible and can expand.** Incompressible input comes back
  longer; callers compare lengths and store the original. Do not "fix" that by
  making `compress` fail.
- **No tracked file names an application built on this crate.**
  `tests/scripts/test-no-consumer-names.sh` reads every tracked file, comments
  included, and is the one place allowed to spell the names.
