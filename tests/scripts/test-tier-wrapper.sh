#!/usr/bin/env bash
# scripts/tier.sh keeps every promise chores.yml makes on its behalf.
#
# The wrapper it runs is rust-fs-core's and is NOT committed here, so the one
# thing this guard must not do is check tier.sh against a stand-in that agrees
# with it. It asks the real script where the real wrapper is
# (`tier.sh --resolve-budget`), copies THAT into a sandbox, and drives tier.sh
# against it -- so a change in core's behaviour shows up here rather than in a
# CI log three repositories away.
#
# What is proven, and why each one is worth a check:
#
#   1. the resolver answers with a copy that says it is the canonical one
#   2. a green tier prints ONE line and puts the transcript in tmp/logs/
#   3. a tier that breaches its line budget exits 65, not 0 and not 1
#   4. a tier that breaches its byte budget exits 65 as well
#   5. a failing command's OWN status is what tier.sh exits with
#   6. a failure prints no tail by default, and OUTPUT_BUDGET_FAIL_TAIL brings
#      one back
#   7. --verbose streams the run AND still enforces the budget
#   8. a present-but-wrong wrapper is FATAL and does not fall through to a
#      good one -- "core is broken" reported as "core is missing" is the
#      quieter and more confusing failure
#   9. no wrapper anywhere fails, naming rust-fs-core and `chore siblings`
#  10. the private copy of the wrapper is removed when the tier ends
#  11. test-floor.sh refuses a tier that ran fewer tests than its floor,
#      refuses a tier that did not run at all, and names a tier that ran none
#
# It accumulates failures rather than stopping at the first: a guard that
# stops early answers one question per run, and these checks are independent.
#
#   bash tests/scripts/test-tier-wrapper.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
fails=0
ok()   { printf 'ok    %s\n' "$1"; }
fail() { printf 'FAIL  %s\n' "$1" >&2; fails=$(( fails + 1 )); }

# --- the resolver, against the real repository ---------------------------
WRAPPER="$(bash "$REPO/scripts/tier.sh" --resolve-budget 2>&1)"
if [ ! -f "$WRAPPER" ]; then
    echo "FAIL  tier.sh --resolve-budget did not name a wrapper:" >&2
    printf '%s\n' "$WRAPPER" >&2
    echo "      rust-fs-core owns the script and this repository keeps no copy" >&2
    echo "      of it. Run \`chore siblings\`, or set FS_CORE_ROOT." >&2
    exit 1
fi
version="$(bash "$WRAPPER" --version 2>&1)"
[ "$version" = "rust-fs-core-output-budget 1" ] \
    && ok "the resolved wrapper is the canonical one ($version)" \
    || fail "the resolved wrapper answered --version with '$version'"

# --- a sandbox repository with a good core beside it ----------------------
mkdir -p "$REPO/tmp"
SANDBOX="$(mktemp -d "$REPO/tmp/tier-wrapper.XXXXXX")"
trap 'rm -rf "$SANDBOX"' EXIT HUP INT TERM

mkdir -p "$SANDBOX/repo/scripts" "$SANDBOX/rust-fs-core/scripts" "$SANDBOX/wrong/scripts"
cp "$REPO/scripts/tier.sh" "$REPO/scripts/test-floor.sh" "$SANDBOX/repo/scripts/"
cp "$WRAPPER" "$SANDBOX/rust-fs-core/scripts/output-budget.sh"
# A copy that exists and is not it. `--version` is the whole contract, so
# answering something else is the only way to be wrong that matters.
sed 's/^OUTPUT_BUDGET_API_VERSION=1$/OUTPUT_BUDGET_API_VERSION=99/' \
    "$WRAPPER" > "$SANDBOX/wrong/scripts/output-budget.sh"

# A stand-in for `cargo test`: N lines, a cargo-shaped result line, a status.
cat > "$SANDBOX/repo/fake-suite.sh" <<'FAKE'
#!/usr/bin/env bash
# fake-suite.sh LINES PASSED STATUS
for i in $(seq 1 "$1"); do echo "running fixture $i with some words on the line"; done
echo "test result: ok. $2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out"
exit "$3"
FAKE
chmod +x "$SANDBOX/repo/fake-suite.sh"

TIER="$SANDBOX/repo/scripts/tier.sh"
FLOOR="$SANDBOX/repo/scripts/test-floor.sh"
SUITE="$SANDBOX/repo/fake-suite.sh"
run() {  # run TIER-ARGS...; captures stdout+stderr in $out and status in $rc
    out="$(cd "$SANDBOX/repo" && "$@" 2>&1)"; rc=$?
}

# --- 2. a green tier is one line, and the transcript is on disk -----------
run bash "$TIER" "test (debug)" debug 50 4000 -- bash "$SUITE" 10 12 0
[ "$rc" -eq 0 ] && [ "$(printf '%s\n' "$out" | wc -l)" -eq 1 ] \
    && grep -q '^test (debug): ok (' <<<"$out" \
    && grep -q 'tmp/logs/debug.log' <<<"$out" \
    && ok "a green tier prints one verdict line naming its log" \
    || fail "a green tier printed status $rc and:"$'\n'"$out"
[ "$(grep -c . "$SANDBOX/repo/tmp/logs/debug.log")" -eq 11 ] \
    && ok "the whole transcript is in tmp/logs/debug.log" \
    || fail "the log does not hold the run: $(wc -l < "$SANDBOX/repo/tmp/logs/debug.log") lines"

# --- 3/4. over budget is 65, on lines and on bytes ------------------------
run bash "$TIER" over-lines over-lines 5 100000 -- bash "$SUITE" 40 12 0
[ "$rc" -eq 65 ] && grep -q 'passed, but printed' <<<"$out" \
    && ok "a tier over its LINE budget exits 65" \
    || fail "over the line budget gave status $rc:"$'\n'"$out"
run bash "$TIER" over-bytes over-bytes 100000 200 -- bash "$SUITE" 40 12 0
[ "$rc" -eq 65 ] && grep -q 'bytes (budget 200)' <<<"$out" \
    && ok "a tier over its BYTE budget exits 65" \
    || fail "over the byte budget gave status $rc:"$'\n'"$out"

# --- 5/6. the command's own status, and the tail ---------------------------
run bash "$TIER" failing failing 50 4000 -- bash "$SUITE" 3 0 7
[ "$rc" -eq 7 ] && grep -q 'failing: FAILED (exit 7)' <<<"$out" \
    && ok "a failing command's own status (7) is the tier's status" \
    || fail "a command exiting 7 gave status $rc:"$'\n'"$out"
grep -q 'running fixture' <<<"$out" \
    && fail "a failure printed the log back by default:"$'\n'"$out" \
    || ok "a failure prints no tail by default"
out="$(cd "$SANDBOX/repo" && OUTPUT_BUDGET_FAIL_TAIL=3 bash "$TIER" failing failing 50 4000 \
    -- bash "$SUITE" 3 0 7 2>&1)"; rc=$?
[ "$rc" -eq 7 ] && grep -q 'last 3 lines' <<<"$out" \
    && ok "OUTPUT_BUDGET_FAIL_TAIL brings the tail back" \
    || fail "OUTPUT_BUDGET_FAIL_TAIL=3 gave status $rc:"$'\n'"$out"

# --- 7. verbose streams, and does not lift the budget ---------------------
out="$(cd "$SANDBOX/repo" && OUTPUT_BUDGET_VERBOSE=1 bash "$TIER" loud loud 5 100000 \
    -- bash "$SUITE" 40 12 0 2>&1)"; rc=$?
grep -q 'running fixture 40' <<<"$out" \
    && ok "--verbose streams the run" \
    || fail "OUTPUT_BUDGET_VERBOSE=1 printed nothing of the run:"$'\n'"$out"
[ "$rc" -eq 65 ] \
    && ok "--verbose does not lift the budget (still 65)" \
    || fail "verbose over budget gave status $rc, not 65"

# --- 8. a present-but-wrong wrapper is fatal, with a good one in reach ----
out="$(cd "$SANDBOX/repo" && FS_CORE_ROOT="$SANDBOX/wrong" bash "$TIER" wrong wrong 50 4000 \
    -- bash "$SUITE" 3 12 0 2>&1)"; rc=$?
[ "$rc" -eq 1 ] && grep -q "answered --version with 'rust-fs-core-output-budget 99'" <<<"$out" \
    && ok "a wrapper with the wrong API version is fatal" \
    || fail "the wrong wrapper gave status $rc:"$'\n'"$out"
[ -f "$SANDBOX/repo/tmp/logs/wrong.log" ] \
    && fail "the tier ran anyway after refusing the wrapper" \
    || ok "it did not fall through to the good sibling beside it"

# --- 9. no wrapper at all names rust-fs-core ------------------------------
out="$(cd "$SANDBOX/repo" && FS_CORE_ROOT="$SANDBOX/absent" bash "$TIER" gone gone 50 4000 \
    -- bash "$SUITE" 3 12 0 2>&1)"; rc=$?
[ "$rc" -eq 1 ] && grep -q 'rust-fs-core' <<<"$out" && grep -q 'v0\.2\.13' <<<"$out" \
    && grep -q 'chore siblings' <<<"$out" \
    && ok "a missing wrapper names rust-fs-core, the release and the task that fetches it" \
    || fail "an absent core gave status $rc:"$'\n'"$out"

# --- 10. the private copy is given back ------------------------------------
copies="$(find "$SANDBOX/repo/tmp" -maxdepth 1 -name 'output-budget.*.sh' | wc -l)"
[ "$copies" -eq 0 ] \
    && ok "the run's private copy of the wrapper is removed afterwards" \
    || fail "$copies copies of the wrapper were left in tmp/"

# --- 11. the floor ---------------------------------------------------------
run bash "$FLOOR" debug 12
[ "$rc" -eq 0 ] && grep -q 'debug: 12 tests executed (floor 12)' <<<"$out" \
    && ok "a tier that met its floor passes" \
    || fail "the floor rejected a tier that met it: status $rc:"$'\n'"$out"
run bash "$FLOOR" debug 13
[ "$rc" -ne 0 ] && grep -q 'floor is 13' <<<"$out" \
    && ok "a tier one test short of its floor fails" \
    || fail "the floor accepted 12 tests against a floor of 13: status $rc"
run bash "$FLOOR" never-ran 1
[ "$rc" -ne 0 ] && grep -q 'did not run' <<<"$out" \
    && ok "a tier with no log at all fails rather than counting zero" \
    || fail "a missing log gave status $rc:"$'\n'"$out"
# A log with no `test result:` line at all -- a build that produced no test
# binary -- is the case the floor exists for, and it must SAY so. grep finding
# nothing exits 1, and under `set -euo pipefail` that used to end the script
# silently at the assignment: still status 1, but with no message and no
# ::error:: annotation naming the tier (#29).
run bash "$TIER" "test (empty)" empty 50 4000 -- bash -c 'echo "compiled; no test binary"'
run bash "$FLOOR" empty 1
[ "$rc" -ne 0 ] && grep -q 'only 0 tests executed in the empty tier, floor is 1' <<<"$out" \
    && ok "a tier whose log holds no result line fails, and says it ran 0" \
    || fail "a log with no result line gave status $rc and no verdict:"$'\n'"$out"

# --- usage -----------------------------------------------------------------
run bash "$TIER" only three args
[ "$rc" -eq 2 ] && ok "too few arguments is a usage error (2)" \
    || fail "three arguments gave status $rc"

if [ "$fails" -gt 0 ]; then
    echo "FAIL  $fails check(s) failed in $(basename "${BASH_SOURCE[0]}")" >&2
    exit 1
fi
echo "PASS  scripts/tier.sh and scripts/test-floor.sh keep their contract"
