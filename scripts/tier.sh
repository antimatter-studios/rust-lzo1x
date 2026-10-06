#!/usr/bin/env bash
# tier.sh LABEL LOG-NAME MAX-LINES MAX-BYTES -- COMMAND [ARG...]
#
# One test tier, run QUIETLY and under a budget. The whole run goes to
# tmp/logs/<LOG-NAME>.log; a pass prints one verdict line naming that log, a
# failure prints one line naming the command's own status, and a run that
# passed but printed more than its budget fails with status 65 -- a status a
# reader can tell apart from a failing suite.
#
# WHY THIS CRATE, WHICH DOES NOT DEPEND ON rust-fs-core, REACHES INTO IT.
#
# `output-budget.sh` lives in rust-fs-core and NOWHERE ELSE. That is the
# owner's rule and it exists because a committed copy is a copy that drifts:
# the family had several, reached four different ways, each repository
# internally consistent and nothing comparing them.
#
# Every other consumer resolves it through its `rust-fs-core` dependency. This
# crate has NO dependencies at all -- "no C bindings, no unsafe, no
# dependencies" is the first line of its description -- so there is nothing
# for cargo to unpack and nothing to ask. The alternative was a repo-local
# script meeting the same contract, and that is the option this repository
# did not take: written locally it would be a near-copy of core's under
# another name, which is precisely how the family ended up with three
# divergent copies in the first place.
#
# So the sibling checkout is the route, `chore siblings` is what produces it,
# and a missing one FAILS LOUDLY here rather than falling back to anything.
# This adds no Cargo dependency: nothing in src/ or tests/ knows core exists,
# and `cargo build` on a bare checkout is unaffected.
#
# THE VERSION STRING IS THE CONTRACT, AND NO DIGEST IS PINNED. The same
# SHA-256 recorded in seven repositories has to be chased through seven
# repositories every time core touches a comment, which is the lockstep this
# arrangement exists to remove. A copy that answers `--version` with the
# expected string is the copy we asked for; one that answers anything else is
# FATAL rather than a reason to look elsewhere, because "core is broken"
# reported as "core is missing" is the quieter and more confusing failure.
#
# ONE WRAPPER FOR BOTH CALLERS. chores.yml runs the tiers for a person at a
# terminal and .github/workflows/ci.yml runs them for the gate, through the
# same command and the same budget, so a tier that has outgrown its budget
# says so before the push. The two files repeat the numbers because the
# workflow cannot read chores.yml without installing chore on two runner
# platforms, and tests/scripts/test-tier-budgets-agree.sh checks that they
# still match.
#
# VERBOSE. `OUTPUT_BUDGET_VERBOSE=1`, or `--verbose`/`-v` in the chore
# invocation's CLI_ARGS (`chore test:debug -- --verbose`), streams the run as
# it happens as well as logging it. It does NOT lift the budget. THE VARIABLE
# IS `OUTPUT_BUDGET_VERBOSE`, NOT `FLTH_VERBOSE` -- the wrapper was
# fs-linux-test-harness's before it was core's, and that rename fails SILENTLY
# where it is not caught, so it is written down here where somebody grepping
# for the old name lands.
#
# A FAILURE PRINTS NO TAIL BY DEFAULT, from rust-fs-core v0.2.13 on. CI
# uploads each tier log as an artifact, and `OUTPUT_BUDGET_FAIL_TAIL=40 chore
# test` restores the tail for one run at a terminal.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

SCRIPT_REL="scripts/output-budget.sh"
EXPECTED_API="rust-fs-core-output-budget 1"
MIN_CORE_VERSION="v0.2.13"
SIBLING_ROOT="${FS_CORE_ROOT:-$REPO/../rust-fs-core}"

die() {
    echo "tier.sh: $*" >&2
    echo "         The output budget wrapper is rust-fs-core's, at $SCRIPT_REL," >&2
    echo "         and this repository deliberately keeps no copy of it." >&2
    echo "         Expected: bash \$core/$SCRIPT_REL --version  ->  $EXPECTED_API" >&2
    echo "         Looked in: $SIBLING_ROOT" >&2
    echo "         Minimum rust-fs-core release carrying it: $MIN_CORE_VERSION." >&2
    echo "         Run \`chore siblings\` to check it out, or set FS_CORE_ROOT." >&2
    exit 1
}

SOURCE="$SIBLING_ROOT/$SCRIPT_REL"
[ -f "$SOURCE" ] || die "$SOURCE does not exist."
FOUND="$(bash "$SOURCE" --version 2>/dev/null || true)"
[ "$FOUND" = "$EXPECTED_API" ] || \
    die "$SOURCE answered --version with '$FOUND', not '$EXPECTED_API'."

# `tier.sh --resolve-budget` prints the wrapper this repository would use and
# does nothing else. It is how tests/scripts/test-tier-wrapper.sh gets hold of
# the REAL canonical script to run its behaviour checks against, so those
# checks cannot pass against a stand-in that merely agrees with them.
if [ "${1:-}" = "--resolve-budget" ]; then
    printf '%s\n' "$SOURCE"
    exit 0
fi

[ $# -ge 5 ] || { echo "tier.sh: usage: tier.sh LABEL LOG MAX-LINES MAX-BYTES -- CMD..." >&2; exit 2; }
LABEL="$1"; LOG_NAME="$2"; MAX_LINES="$3"; MAX_BYTES="$4"; shift 4
[ "${1:-}" = "--" ] && shift
[ $# -gt 0 ] || { echo "tier.sh: no command" >&2; exit 2; }

# THE RUN GETS ITS OWN COPY, AND GIVES IT BACK. Core is a checkout somebody
# else may be moving while this runs -- `chore siblings` half way through a
# long tier would otherwise change the script underneath it. tmp/ is
# gitignored and is already where the tier logs live; $$ keeps two concurrent
# tiers apart.
mkdir -p "$REPO/tmp"
BUDGET="$REPO/tmp/output-budget.$$.sh"
cp "$SOURCE" "$BUDGET"
trap 'rm -f "$BUDGET"' EXIT

case " ${CLI_ARGS:-} " in
    *" --verbose "*|*" -v "*) export OUTPUT_BUDGET_VERBOSE=1 ;;
esac

# `bash "$BUDGET"` rather than executing it: a copy's mode is not this
# script's business. It is not `exec`ed either -- that would replace this
# shell, the EXIT trap would never fire, and the copy would be left behind.
# `set -e` hands the command's own status on, which is the status this script
# must exit with, and which tests/ci_profile.rs requires of every gating run.
bash "$BUDGET" \
    --log "$REPO/tmp/logs/$LOG_NAME.log" \
    --max-lines "$MAX_LINES" \
    --max-bytes "$MAX_BYTES" \
    --label "$LABEL" \
    -- "$@"
