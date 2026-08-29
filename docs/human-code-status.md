# Human-code findings — status

Tracks every **High** and **Medium** finding from
[`human-code-report-2026-08-28.md`](human-code-report-2026-08-28.md) and what
was done about it.

The report was written before any of it was acted on, so it records everything
as open. This file is the current position. Updated 2026-08-29.

**19 findings total** — 4 High, 9 Medium, 6 Low. This file covers the 13 High
and Medium; the Low items are untouched and still described in the report.

| | High | Medium |
|---|---|---|
| Fixed | 4 | 5 |
| Left deliberately | 0 | 2 |
| Still open | 0 | 2 |

---

## High

### H1 — the end-of-stream comment described a stricter rule than the code — **fixed earlier**

Fixed by [#3](https://github.com/antimatter-studios/rust-lzo1x/pull/3), before
this pass. The grammar block now says the marker is *any* token in the bucket
whose distance contribution is zero, "whatever `LLL` holds", and names
`11 00 00` as merely the spelling encoders emit.

The report was right that the code was the correct side and the comment was
wrong. Tightening the check to match the comment would have made the decoder
stricter than the format and broken btrfs and squashfs, with no test to catch
it.

### H2 — `state` carried two meanings and the sentinel `4` was unnamed — **fixed**

`LONG_LITERAL_RUN_STATE` now names it, with a doc comment stating why the value
is not a count, and the three raw `4`s use it.

The invariant the report identified — that `state` can never hold the sentinel
at the trailing-literal copy, because every path there has just assigned
`t & 3` or `op & 3` and the long-literal path `continue`s first — is now a
`debug_assert!` at the site that depends on it. A future arm that assigned the
sentinel and fell through would fail a debug build instead of quietly copying
four bytes that are not literals.

### H3 — the zero-run extension base was not visibly the command byte's mask — **fixed earlier**

Fixed by [#3](https://github.com/antimatter-studios/rust-lzo1x/pull/3).

### H4 — the densest dispatch arms had no coverage in the default run — **fixed earlier**

Fixed by [#3](https://github.com/antimatter-studios/rust-lzo1x/pull/3), which
covered every instruction bucket. 18 unit tests run by default.

---

## Medium

### M1 — `h` named a whole byte in three places and one bit in a fourth — **fixed**

The single-bit one is now `dist_high_bit`, with a comment saying it is *not* an
extension byte like the `h` in the arms either side of it — it is one bit lifted
out of the command byte, contributing 2^14 to the distance.

### M2 — the `t >= 128` and `t >= 64` arms are near-identical — **left deliberately**

**Needs a human decision.** Two arms, not three, so it is below the threshold at
which extracting a shared helper pays for itself — and the report itself says
the arms are "appropriately dense and each is only 5-10 lines". Folding two
hot-path decode arms together to save one expression risks a subtle error in the
part of this crate that is hardest to test, for a gain that is a matter of
taste.

### M3 — `run()` holds three jobs in one 110-line scope — **left deliberately**

**Needs a human decision.** Splitting the bootstrap, the five-way dispatch and
the trailing-literal epilogue into separate methods is a real improvement to
read, and the report argues it well. It is also a restructure of the decode loop
with no behavioural test that would catch a mistake in the seams. H2's
`debug_assert` addresses the specific correctness risk the report attributed to
the shared scope, which lowers the urgency.

### M4 — the allocation cap `1 << 20` was unexplained — **fixed**

`INITIAL_CAPACITY_CAP`, documented as the third leg of the same defence as
`max_out` and `MAX_EXTENDED_LENGTH`: `max_out` is caller-supplied, so reserving
it outright would let a bogus bound drive a large allocation before a byte is
validated. Capping the *eager* reservation costs one reallocation on a genuinely
large block; the real limit is still enforced per copy.

### M5 — `16384` and `<< 14` were the same constant twice — **fixed earlier**

`LONG_MATCH_DISTANCE_BASE`, added by [#3](https://github.com/antimatter-studios/rust-lzo1x/pull/3).

### M6 — the LE16 operand's split was open-coded at five sites — **fixed**

`split_operand(op) -> (dist_bits, next_state)` expresses the grammar's
`DDDDDDDD DDDDDDSS` once. Five sites now use it.

The report's reason for caring is the one that made this worth doing: `>> 2` on
the operand and `>> 2` on a *command* byte mean different things and look
identical.

### M7 — overflow discipline differed between three bounds checks — **fixed**

Documented rather than changed, which is what the report offered as the
alternative. The comment now says the two output-bounds adds take values bounded
by `MAX_EXTENDED_LENGTH` against a length that cannot exceed `max_out`, so they
overflow only for a `max_out` within ~16 MiB of `usize::MAX` and the consequence
is a failed allocation; the checked one adds to an *input* offset bounded by
nothing the caller declares.

### M8 — the lzop header-version gate `0x0940` appeared three times — **fixed earlier**

Fixed by [#3](https://github.com/antimatter-studios/rust-lzo1x/pull/3).

### M9 — the "was this block compressed" predicate was written twice, inverted — **fixed**

`let stored = compressed_len >= uncompressed_len;` computed once, with `!stored`
at the checksum skip.

The consequence the report named is why this is worth more than tidiness: the
same question decides whether a checksum field is present *and* whether the
payload is an LZO1X stream. If the two spellings ever disagreed the cursor would
desync and every later block would be read from the wrong offset — surfacing as
a confusing decode failure rather than a parse error.

---

## Verification

`cargo test` — 18 unit, 1 doc, and 6 lzop oracle tests pass, unchanged in
number, which is the point: none of these are behavioural changes. `chore lint`
clean.

The `debug_assert` in H2 is the one addition that can fail, and it holds across
the whole suite.
