#!/usr/bin/env bash
# test-floor.sh TIER FLOOR  -- the tier ran at least FLOOR tests
#
# THE FAILURE A BUDGET CANNOT SEE. scripts/tier.sh fails a tier that PRINTS
# more than it is allowed to; nothing fails a tier that prints almost nothing
# because it RAN almost nothing. `cargo test` exits 0 on "0 passed; 0 failed",
# so a filter that selected nothing, a build that produced no test binary and
# a suite that ran in full all report the same green -- and the quieter this
# repository's output gets, the less anybody would notice. That is why every
# tier carries a number as well as a budget (#20).
#
# It reads the tier's log (tmp/logs/<TIER>.log, written by tier.sh) rather
# than a pipe, so it cannot swallow the test run's own verdict: a
# `cargo test | test-floor.sh` would report THIS script's exit status and
# discard the suite's. That is the property tests/ci_profile.rs exists to
# protect, and the reason the floors that used to be eight inline lines in
# four ci.yml steps are a file now -- the numbers stayed at the call site,
# where they belong, and only the counting moved.
#
# A COUNT ANSWERS "DID ANYTHING RUN", NOT "DID IT CHECK ANYTHING". A test that
# returns early because a tool is missing still counts as passed. The answer
# to that is a test that fails rather than returns -- AGENTS.md's "Nothing
# skips" -- not a bigger number here.
#
# The floor is MEASURED, and it only ever goes up: a floor lowered to make a
# run pass is a floor that has stopped measuring anything.
set -euo pipefail

[ $# -eq 2 ] || { echo "usage: test-floor.sh TIER FLOOR" >&2; exit 2; }
TIER="$1"
FLOOR="$2"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG="$REPO/tmp/logs/$TIER.log"

if [ ! -f "$LOG" ]; then
    echo "test-floor.sh: $LOG is missing -- the $TIER tier did not run." >&2
    exit 1
fi

# `test result: ok. 40 passed; 0 failed; ...`, one line per test binary.
# `-a` because a test that prints a byte sequence the log cannot decode makes
# grep call the file binary and report nothing at all -- and this crate is a
# decompressor whose tests print compressed streams.
#
# `|| true` because grep finding NOTHING exits 1, and under `pipefail` and
# `-e` that ended this script at the assignment -- still failing, but silently,
# in exactly the case this file exists to name: a tier that ran zero tests (#29).
ran="$({ grep -aoE 'test result: ok\. [0-9]+ passed' "$LOG" || true; } | awk '{ sum += $4 } END { print sum + 0 }')"
# The semver tier runs lints, not tests: cargo-semver-checks reports
# `Checked [ 0.013s] 196 checks: 196 pass, 58 skip`. Those are counted the
# same way, so a run that checked nothing -- a crate that did not build, a
# lint set that came back empty -- falls under its floor rather than passing.
# The escapes are stripped first: CI sets CARGO_TERM_COLOR=always, and
# cargo-semver-checks colours `Checked` under it.
checks="$(awk '{ gsub(/\033\[[0-9;]*m/, ""); print }' "$LOG" | { grep -aoE 'Checked \[ *[0-9.]+s\] [0-9]+ checks:' || true; } | awk '{ sum += $(NF-1) } END { print sum + 0 }')"
ran=$(( ran + checks ))
if [ "$ran" -lt "$FLOOR" ]; then
    echo "::error::only $ran tests executed in the $TIER tier, floor is $FLOOR -- a run that executes fewer than that stopped early rather than passed"
    echo "test-floor.sh: the $TIER tier executed $ran tests; the floor is $FLOOR." >&2
    echo "               A tier that runs fewer tests than it used to has stopped" >&2
    echo "               early rather than passed. The whole run is in $LOG." >&2
    exit 1
fi
printf '%s\n' "$TIER: $ran tests executed (floor $FLOOR)"
