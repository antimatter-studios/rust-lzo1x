# Human-code review — am-lzo1x

**This document is analysis only. No code was changed, no tests were added, nothing was
committed.** It is the output of Phase 0 (Understand) and Phase 1 (Scan and Triage) of the
`human-code` skill. Phase 2 (the implementation dev-loop) was deliberately not run — the
findings below are for you to read and rule on first.

| | |
|---|---|
| Date | 2026-08-28 |
| Scope | full crate — `src/lib.rs` (461 lines), `tests/oracle_lzop.rs` (308 lines) |
| Items found | **19** |
| Items fixed | **0** (report-only run) |
| Severity split | 4 High · 9 Medium · 6 Low |
| Baseline tests | 11 unit + 1 doctest passing; 6 oracle tests passing with `--ignored` |
| Static analysis | `cargo clippy --all-targets -- -D warnings` — clean |

## Reading note on density

A decompressor is dense by nature and that is not a defect. Nothing below is filed because
"bit twiddling is hard to read". Every item is one of: a specific literal that appears
multiple times unnamed, a name that means two different things in one scope, a comment that
describes different behaviour from the code beneath it, or a check whose correctness depends
on an invariant that is never stated. Where the density is inherent to the format, it is
left alone and said so.

## What was verified, not assumed

Three claims in this report are load-bearing, so they were checked against the running code
rather than inferred by reading. A scratch crate outside this repository was pointed at the
library and run; the repository working tree was not touched.

- The end-of-stream condition (H1) — confirmed that `0x13 00 00` and `0x17 00 00` both
  terminate the stream successfully, and that `0x19 00 00` does not.
- Trailing input after the marker (L6) — confirmed silently accepted.
- Reachability of the untested buckets (H4) — confirmed that the `64..=127`, `128..=255`,
  and `t < 16` short-match paths all decode correctly from fixtures under 16 bytes, and that
  the other two gaps require multi-kilobyte fixtures.

---

# Findings

## High

### H1 — The end-of-stream comment describes a stricter rule than the code implements

- **Files:** [`src/lib.rs:56`](../src/lib.rs), [`src/lib.rs:303-307`](../src/lib.rs)
- **Category:** Comments that lie
- **Coverage:** the canonical marker is covered by every unit test (`EOS` fixture); the
  divergence itself is covered by nothing.

The grammar block says:

```
//!     dist==16384 with no length bits is the END-OF-STREAM marker.
```

and the inline comment repeats it:

```rust
// EOS: the canonical marker is `11 00 00`
// (t=0x11, LE16=0x0000) -> dist == 16384, len == 3.
if op >> 2 == 0 && h == 0 {
    return Ok(());
}
```

The code does not test the length bits at all. It terminates on *any* command byte in
`16..=31` whose distance contribution is zero, whatever `LLL` holds. Verified against the
built library:

| stream | doc predicts | actual |
|---|---|---|
| `0x11 0x00 0x00` (`LLL`=1) | end of stream | end of stream |
| `0x13 0x00 0x00` (`LLL`=3) | a match | **end of stream** |
| `0x17 0x00 0x00` (`LLL`=7) | a match | **end of stream** |
| `0x19 0x00 0x00` (`H`=1) | a match | a match (rejected here — no prior output) |

The code is the side that is right: the reference decoder tests only the distance
contribution, which is why the oracle tests pass. The danger is the direction of the error.
A maintainer who trusts the comment and "tightens" the check to also require `base == 0` is
making the decoder stricter than the format, and no test in the suite would catch it. This
is the single most likely way for a future change to break btrfs or squashfs.

The fix is a comment, not a code change: state that the marker is *any* token in this bucket
with a zero distance contribution, that `0x11 0x00 0x00` is merely the canonical spelling
emitted by encoders, and that the length bits are ignored here by design.

Two smaller things sit in the same block. `len` is computed at lines 296-300 and then
discarded on the EOS path, which reads as if it were forgotten rather than deliberate — and
it must stay computed, because the `base == 0` case consumes extension bytes before the
operand is read, so the order is load-bearing and unremarked. And `dist` (line 302) is
likewise computed before the test that may discard it.

### H2 — `state` carries two incompatible meanings, and the sentinel `4` is unnamed at three sites

- **Files:** [`src/lib.rs:254`](../src/lib.rs), [`src/lib.rs:328`](../src/lib.rs),
  [`src/lib.rs:339`](../src/lib.rs), [`src/lib.rs:349-351`](../src/lib.rs)
- **Category:** Magic numbers · misleading names
- **Coverage:** the `state == 0` literal path is covered; `state` in `1..=3` and `state == 4`
  are **not** covered by any default-run test (see H4).

`state` is a `usize` that means "how many trailing literals to copy" for values `0..=3`, and
means "a long literal run just happened" for the value `4`. The second meaning is not a
count. The value appears raw at three sites:

```rust
state = n.min(4);            // line 254 — bootstrap
state = 4;                   // line 328 — after a long literal run
let (len, dist) = if state == 4 { ... }   // line 339 — dispatch on the sentinel
```

The consequence is at lines 349-351:

```rust
if state > 0 {
    self.copy_literals(state)?;
}
```

This is only correct because `state` can never be `4` at that point — every path that
reaches it has just assigned `t & 3` or `op & 3`, and the literal-run path `continue`s at
line 329 before getting there. That is a real invariant and the code depends on it for
correctness, but a reader has to reconstruct it by tracing all four assignment sites. If
some future arm assigned `4` and fell through, the decoder would silently emit four garbage
literal bytes rather than fail.

Worth naming the sentinel (`LONG_LITERAL_RUN_STATE`) and stating the invariant at line 349 —
ideally as a `debug_assert!(state < 4)`, which documents and enforces in one line.

### H3 — The zero-run extension base is always the command byte's field mask, and that link is invisible

- **Files:** [`src/lib.rs:284`](../src/lib.rs), [`src/lib.rs:297`](../src/lib.rs),
  [`src/lib.rs:323`](../src/lib.rs), against [`src/lib.rs:282`](../src/lib.rs) and
  [`src/lib.rs:295`](../src/lib.rs)
- **Category:** Magic numbers
- **Coverage:** `extend_length(15)` is covered by `zero_run_length_extension`.
  `extend_length(31)` and `extend_length(7)` are **not covered**.

Three call sites, three bare numbers:

```rust
let base = (t & 31) as usize;                  // line 282
let len = if base == 0 { self.extend_length(31)? + 2 } else { base + 2 };   // line 284

let base = (t & 7) as usize;                   // line 295
let len = if base == 0 { self.extend_length(7)? + 2 } else { base + 2 };    // line 297

let n = if t == 0 { self.extend_length(15)? + 3 } else { t as usize + 3 };  // line 323
```

The rule is uniform and never stated: **the extension base equals the width mask of the
length field in the command byte**, because zero is the escape and the extension therefore
starts where the field saturates. `31` at line 284 is the same `31` as line 282; `7` at line
297 is the same `7` as line 295; the `15` at line 323 is the mask of a field whose mask never
appears in the code at all, because `t < 16` is checked instead.

Three unnamed literals with a shared derivation is exactly the case for a named group. Named
as a family — `MATCH_LEN_FIELD_MASK_5BIT = 31`, `MATCH_LEN_FIELD_MASK_3BIT = 7`,
`LITERAL_LEN_FIELD_MASK = 15` — the mask and the base become the same identifier at both
sites, and the invariant enforces itself. As written, someone widening a field would change
the mask and not the base, and the two covered tests would not notice.

This also sits oddly beside the constants that *are* named right above
(`LONG_MATCH_DISTANCE_BASE`, `POST_LITERAL_MATCH_DISTANCE_BASE`): the crate already decided
these numbers deserve names, and then stopped halfway.

### H4 — The densest dispatch arms have no coverage in the default test run

- **Files:** [`src/lib.rs:268-345`](../src/lib.rs), [`tests/oracle_lzop.rs`](../tests/oracle_lzop.rs)
- **Category:** Coverage gap gating every other item
- **Coverage:** this *is* the finding.

The 11 unit tests use only these command bytes: `21`, `2`, `1`, `0`, `0x22`, and the `0x11`
marker. Mapping that onto the five instruction buckets:

| bucket | line | covered by default `cargo test`? |
|---|---|---|
| bootstrap (`first >= 18`) | 250 | yes |
| `t < 16`, state 0 — literal run | 318 | yes |
| `t < 16`, state 1..=3 — 2-byte match | 341 | **no** |
| `t < 16`, state 4 — 3-byte match | 340 | **no** |
| `16..=31` — long-distance match (non-EOS) | 302 | **no** |
| `16..=31` — end-of-stream | 305 | yes |
| `32..=63` | 280 | yes |
| `64..=127` | 274 | **no** |
| `128..=255` | 268 | **no** |

Five of nine paths — including both short-distance match buckets, which is where most real
LZO output actually lives — are exercised only by the oracle tests, and those are
`#[ignore]`-gated. A contributor without `lzop` installed can refactor the match decoder,
see 11 green tests, and ship a broken decoder. CI does run the oracle layer, so this is a
local-development and review hazard rather than a release one, but it is the reason to fix
this item before any other.

Three of the five gaps are cheap. Verified working against the built library with fixtures
under 16 bytes:

| bucket | fixture | decodes to |
|---|---|---|
| `128..=255` | `01 'w' 'x' 'y' 'z' 8C 00` + EOS | `wxyzwxyzw` |
| `64..=127` | `01 'w' 'x' 'y' 'z' 4C 00` + EOS | `wxyzwxy` |
| `t<16` state 1..=3 | `01 'w' 'x' 'y' 'z' 22 0D 00 'A' 04 00` + EOS | `wxyzwxyzAzA` |

The other two are structurally expensive and that is worth recording so nobody rediscovers
it: the `state == 4` match has a distance base of 2049 and the non-EOS `16..=31` bucket a
base of 16384, so `copy_match`'s `dist > self.out.len()` guard means those paths cannot be
reached without ~2 KiB and ~16 KiB of prior output respectively. Fixtures are a literal run
of that size followed by the match token — mechanical, just not small.

## Medium

### M1 — `h` names a whole byte in three places and a single bit in a fourth

- **Files:** [`src/lib.rs:270`](../src/lib.rs), [`src/lib.rs:276`](../src/lib.rs),
  [`src/lib.rs:294`](../src/lib.rs), [`src/lib.rs:337`](../src/lib.rs)
- **Category:** Misleading names
- **Coverage:** lines 270, 276, 337 uncovered; 294 covered only on the EOS path.

```rust
let h = self.next()? as usize;        // 270 — an 8-bit distance extension byte
let h = self.next()? as usize;        // 276 — same
let h = ((t >> 3) & 1) as usize;      // 294 — one bit out of the command byte
let h = self.next()? as usize;        // 337 — an 8-bit distance extension byte
```

All four feed a distance calculation, so the reader cannot disambiguate by context. Line 294
is the `H` of the published grammar (`0 0 0 1 H L L L`), so `h` is the right name *there*
and the wrong name at the other three, which the grammar calls `next_byte`. Renaming the
byte sites to `dist_hi` or `ext` leaves `h` meaning exactly what the spec says it means.

### M2 — The `t >= 128` and `t >= 64` arms are byte-identical apart from one expression

- **Files:** [`src/lib.rs:268-279`](../src/lib.rs)
- **Category:** Duplicated code
- **Coverage:** neither arm is covered by default-run tests.

```rust
let (len, dist, new_state) = if t >= 128 {
    let h = self.next()? as usize;
    let len = 5 + ((t >> 5) & 3) as usize;
    let dist = (h << 3) + ((t >> 2) & 7) as usize + 1;
    (len, dist, (t & 3) as usize)
} else if t >= 64 {
    let h = self.next()? as usize;
    let len = 3 + ((t >> 5) & 1) as usize;
    let dist = (h << 3) + ((t >> 2) & 7) as usize + 1;
    (len, dist, (t & 3) as usize)
}
```

Three of the four lines are identical; only `len` differs. A reader has to diff two blocks by
eye to discover that the distance and state encodings are shared — which is a genuine fact
about the format worth making visible.

This is two instances, below the three-instance threshold for extracting a helper, so the fix
is to merge the arms rather than add indirection: one branch computing `len`, then the shared
`dist` and `new_state` below it.

### M3 — `run()` holds three unrelated jobs in one 110-line scope

- **Files:** [`src/lib.rs:244-353`](../src/lib.rs)
- **Category:** God function
- **Coverage:** partial (see H4).

To be clear about what is *not* being claimed: the arms themselves are appropriately dense
and each is only 5-10 lines. The problem is that one function scope holds the stream
bootstrap (246-257), a five-way instruction dispatch (266-346), and the trailing-literal
epilogue (348-351) — and it is precisely that shared scope which makes H2's `state` invariant
invisible, because the variable is live across all three jobs.

Extracting each match bucket to a method returning `(len, dist, new_state)` and the bootstrap
to its own method would leave `run()` as pure dispatch and make `state`'s lifetime short
enough to see whole. Worth doing *after* H4, not before — refactoring five arms of which
three are untested is the wrong order.

### M4 — The allocation cap `1 << 20` is unexplained

- **Files:** [`src/lib.rs:161`](../src/lib.rs)
- **Category:** Magic numbers
- **Coverage:** exercised by every test, asserted by none.

```rust
out: Vec::with_capacity(max_out.min(1 << 20)),
```

This is the only bare, uncommented literal in the public entry point, and it is a security
control: `max_out` is caller-supplied, so pre-allocating it outright would let a bogus bound
drive a large allocation before a single byte is validated. The `.min()` caps the *eager*
allocation at 1 MiB while leaving the real bound enforced later in `copy_*`. None of that is
visible. `INITIAL_CAPACITY_CAP` plus one line of why would carry it.

Note that the crate documents this exact threat model for `max_out` at lines 148-150 and for
`MAX_EXTENDED_LENGTH` at 126-131 — this is the third leg of the same defence and the only one
left anonymous.

### M5 — `16384` and `<< 14` are the same constant written two ways

- **Files:** [`src/lib.rs:137`](../src/lib.rs), [`src/lib.rs:302`](../src/lib.rs)
- **Category:** Magic numbers
- **Coverage:** covered only via the EOS path.

```rust
const LONG_MATCH_DISTANCE_BASE: usize = 16384;
...
let dist = LONG_MATCH_DISTANCE_BASE + (h << 14) + (op >> 2);
```

`16384 == 1 << 14` is not a coincidence: the `H` bit selects the second 16 KiB window, so the
shift and the base are the same quantity. Written as they are, the relationship is invisible,
and the reader cannot tell whether `14` is tied to the base or independent of it. Expressing
the base as `1 << LONG_MATCH_DISTANCE_SHIFT` makes the pairing structural.

### M6 — The LE16 operand's split into distance and state is open-coded at five sites

- **Files:** [`src/lib.rs:289-290`](../src/lib.rs), [`src/lib.rs:302`](../src/lib.rs),
  [`src/lib.rs:305`](../src/lib.rs), [`src/lib.rs:308`](../src/lib.rs)
- **Category:** Dense expressions · duplication
- **Coverage:** the `32..=63` site is covered; the `16..=31` sites only on the EOS path.

`op >> 2` (distance contribution) and `op & 3` (next trailing-literal state) appear five
times as raw shifts. `next_le16`'s doc comment already explains the split in prose — the code
never expresses it. A small `fn split_operand(op: usize) -> (usize, usize)` returning
`(dist_bits, next_state)`, or two named locals per site, would put the spec's `D`/`SS` naming
into the code.

The reason this matters more than a typical shift is collision: `>> 2` at line 338 means
something completely different (`(t >> 2) & 3` extracts the *command byte's* distance bits),
so the same token carries two meanings within thirty lines.

### M7 — Overflow discipline differs between the three bounds checks with no stated reason

- **Files:** [`src/lib.rs:221`](../src/lib.rs), [`src/lib.rs:234`](../src/lib.rs),
  [`src/lib.rs:237`](../src/lib.rs)
- **Category:** Non-obvious check
- **Coverage:** `output_exceeding_max_out_errors` covers the failing branch of 234.

```rust
if self.out.len() + len > self.max_out { return Err(E); }   // 221 — unchecked add
if self.out.len() + n > self.max_out { return Err(E); }     // 234 — unchecked add
let end = self.ip.checked_add(n).ok_or(E)?;                 // 237 — checked add
```

Line 237 guards against overflow three lines after line 234 declines to. **This is not a live
bug** — `n` is bounded by `MAX_EXTENDED_LENGTH`, so the unchecked adds only overflow if a
caller passes a `max_out` within ~16 MiB of `usize::MAX`, and the consequence would be a
failed allocation rather than anything unsound (the crate is `#![forbid(unsafe_code)]`). But
a reader auditing the decoder for untrusted input — which the crate docs explicitly invite at
lines 73-77 — has to derive that bound themselves to conclude the asymmetry is deliberate.
Either make all three consistent or add one line saying why 237 is the only one that can
overflow.

### M8 — The lzop header-version gate `0x0940` appears three times unnamed

- **Files:** [`tests/oracle_lzop.rs:90`](../tests/oracle_lzop.rs),
  [`tests/oracle_lzop.rs:94`](../tests/oracle_lzop.rs),
  [`tests/oracle_lzop.rs:103`](../tests/oracle_lzop.rs)
- **Category:** Magic numbers
- **Coverage:** exercised whenever the oracle tests run.

```rust
if version >= 0x0940 { let _version_needed = c.u16(); }
...
if version >= 0x0940 { let _level = c.u8(); }
...
if version >= 0x0940 { let _mtime_high = c.u32(); }
```

Three occurrences of the same threshold, each gating a different optional header field. The
file already names its other container constants (`LZOP_MAGIC`, `F_ADLER32_D`, …), so this is
an omission rather than a house style. `const LZOP_VERSION_0_94: u16 = 0x0940;` next to the
flag constants.

### M9 — The "was this block compressed" predicate is written twice, in opposite polarity

- **Files:** [`tests/oracle_lzop.rs:125`](../tests/oracle_lzop.rs),
  [`tests/oracle_lzop.rs:133`](../tests/oracle_lzop.rs)
- **Category:** Duplicated code
- **Coverage:** exercised by `roundtrip_incompressible`, which is the test that depends on it.

```rust
if c_csum && compressed_len < uncompressed_len { let _ = c.u32(); }   // 125
...
stored: compressed_len >= uncompressed_len,                            // 133
```

The same domain question — did the encoder actually compress this block — decides both
whether a checksum field is present in the stream and whether the payload is an LZO1X stream.
Written twice, eight lines apart, in inverted form. If the two ever disagree the cursor
desyncs and every subsequent block is parsed from the wrong offset, which would surface as a
confusing decode failure rather than a parse error. Compute `let stored = compressed_len >=
uncompressed_len;` once, above line 125, and use `!stored` at the checksum skip.

## Low

### L1 — `const E: Error` is a single-letter name used at ten sites

- **Files:** [`src/lib.rs:120`](../src/lib.rs)
- **Category:** Opaque names

Terse to the point of collision — `E` reads as a generic error type parameter. `MALFORMED`
costs nothing at the call sites (`ok_or(MALFORMED)`) and says what happened. Judgement call;
the current form is compact and consistent, so this is genuinely cosmetic.

### L2 — A comment in `extend_length` describes the code two lines below it

- **Files:** [`src/lib.rs:204-208`](../src/lib.rs)
- **Category:** Comment placement

```rust
// Cap the run length far below any legitimate block size so
// a corrupt stream is rejected early rather than looping.
len = len.checked_add(ZERO_RUN_INCREMENT).ok_or(E)?;
if len > MAX_EXTENDED_LENGTH { return Err(E); }
```

The comment sits above the accumulate and describes the cap. It also restates what
`MAX_EXTENDED_LENGTH`'s own doc comment (126-131) already says better. Move it or drop it.

### L3 — `copy_match`'s self-referential loop is safe for a reason that is not given

- **Files:** [`src/lib.rs:225-228`](../src/lib.rs)
- **Category:** Non-obvious logic

```rust
let start = self.out.len() - dist;
for src in start..start + len {
    let b = self.out[src];
    self.out.push(b);
}
```

This indexes a vector while pushing to it, and `start + len` may exceed the `out.len()` that
held when the loop began — which is the point, it is how overlapping runs work. It cannot
panic because each iteration pushes exactly one byte, so `src < self.out.len()` stays true by
induction. The comment above explains *that* overlapping copies are byte-at-a-time; it does
not explain why the indexing is in-bounds, which is the part a reader stops at.

### L4 — The magic length `9` is hardcoded twice instead of `LZOP_MAGIC.len()`

- **Files:** [`tests/oracle_lzop.rs:85-86`](../tests/oracle_lzop.rs)
- **Category:** Magic numbers

```rust
assert_eq!(&data[..9], &LZOP_MAGIC, "not an lzop file");
let mut c = Cursor { b: data, i: 9 };
```

The constant it derives from is right there on line 34.

### L5 — Short field and binding names in non-obvious positions

- **Files:** [`tests/oracle_lzop.rs:43-44`](../tests/oracle_lzop.rs),
  [`src/lib.rs:158`](../src/lib.rs)
- **Category:** Opaque names

`Cursor { b, i }` (buffer and index) and `let mut d = Decoder { … }`. Both are small scopes
and both are guessable, hence Low. Noted only because `b` is also the name used for a single
*byte* elsewhere in the same file.

Explicitly **not** filed: `t`, `dd`, `op`, `len`, `dist`. These are the published grammar's
own names for these quantities, and the grammar is reproduced at the top of the file, so they
are more useful than longer inventions would be.

### L6 — Trailing input after the end-of-stream marker is silently accepted, undocumented

- **Files:** [`src/lib.rs:305-307`](../src/lib.rs), [`src/lib.rs:143-156`](../src/lib.rs)
- **Category:** Undocumented behaviour

`run()` returns as soon as the marker is decoded, without checking that the input is
exhausted. Verified: `[21, 'a','b','c','d', 0x11,0x00,0x00, 0xFF,0xFF,0xFF]` returns
`Ok("abcd")`. This is defensible leniency and matches how block-oriented callers use the
crate, but `decompress`'s doc comment carefully enumerates every rejection condition and does
not mention this acceptance. Both callers here (btrfs, squashfs) hand over exactly one block
and would arguably prefer to know if bytes were left over. One sentence in the doc, or a note
that the input length is not validated, closes it.

---

# Items skipped

Nothing was fixed, so nothing is "skipped" in the usual sense. This table records candidates
that were considered during triage and **deliberately not filed** as findings.

| Candidate | Reason |
|---|---|
| Bucket boundary literals `16` / `32` / `64` / `128` at lines 266-280 | *Acceptable pattern* — they read directly off the grammar block 200 lines above, appear once each, and `t >= 128` is more legible than a named constant would be. |
| Bootstrap literals `18` / `17` at lines 250-252 | *Acceptable pattern* — same reasoning; documented in prose at lines 68-71 and used once each. Borderline with H3; filed separately only if that item is taken. |
| Match length bases `+ 2`, `+ 3`, `+ 5` | *Acceptable pattern* — minimum match lengths, each stated in the grammar block and used once per bucket. Naming them would add five constants for no comprehension gain. |
| `t`, `dd`, `op`, `dist`, `len` as names | *False positive* — these are the specification's own identifiers. |
| Unchecked indexing in `Cursor` (`tests/oracle_lzop.rs:47-72`) | *Acceptable pattern* — test-only parser over output from a known-good encoder; a panic is the correct failure mode and is as informative as an error would be. |
| `assert_roundtrip_inner`'s three parameters | *Below threshold* — three is under the ~4 parameter guideline and the `expect_compression` flag is well documented at lines 184-186. |
| `pseudo_random` xorshift constants `13` / `7` / `17` | *Acceptable pattern* — a named xorshift triple; naming the shifts would not help anyone. |
| `Error` carrying no position information | *False positive* — deliberate, and the rationale is already documented at lines 96-98. |

---

# Test results

No code changed, so before and after are identical. Recorded as the baseline any future
dev-loop must hold.

| | before | after |
|---|---|---|
| Unit tests passing (`cargo test`) | 11 | 11 (unchanged) |
| Doctests passing | 1 | 1 (unchanged) |
| Oracle tests passing (`-- --ignored`, `lzop` installed) | 6 | 6 (unchanged) |
| Tests failing | 0 | 0 |
| `cargo clippy --all-targets -- -D warnings` | clean | clean |
| Instruction buckets covered by default run | 4 of 9 | 4 of 9 |

Coverage is stated as bucket coverage rather than line percentage because no coverage tool is
configured in this repository and line coverage would flatter the result — the uncovered
paths are short, so they would barely move a line-percentage number while representing five
of the nine ways a stream can be decoded.

---

# What to fix first

The ordering below is chosen so that every risky change lands on top of tests that already
exist, and so that the zero-risk items clear early.

1. **H4 — close the coverage gap.** Nothing else should be touched first. Add the three cheap
   unit tests (fixtures verified above), then the 2 KiB and 16 KiB fixtures for the
   `state == 4` and long-distance buckets. This converts every remaining item from "refactor
   and hope" into a test-gated change, and it is the item with standalone value even if you
   stop there.
2. **H1 — correct the end-of-stream comments.** Pure documentation, zero behavioural risk,
   and it removes the most dangerous piece of misdirection in the file. Could equally go
   first; placed second only because a test asserting `0x13 0x00 0x00` terminates (from step
   1) makes the corrected comment enforceable rather than merely accurate.
3. **H3 — name the field masks as a group.** Naming only, and it retires three unnamed
   literals plus the invisible mask/base identity. Do it after step 1 because two of the
   three call sites are currently untested.
4. **H2 — name the `4` sentinel and assert its invariant.** Naming plus one `debug_assert!`.
5. **M5, M1, M6, M4 — the remaining naming items.** All local, all mechanical, in that order
   (M5 and M1 are single-expression edits; M6 touches five sites; M4 adds a constant plus a
   comment).
6. **M2 — merge the two identical short-distance arms.** First item that moves code rather
   than names it; needs step 1's tests.
7. **M3 — extract the buckets out of `run()`.** Last of the source items. Largest diff,
   highest regression surface, and its main benefit (making `state`'s lifetime visible) is
   partly delivered by H2 already, so it is the easiest to defer or decline.
8. **M8, M9 — the two test-file items.** Independent of everything above; can be done at any
   point by anyone.
9. **L1-L6 — cosmetic.** Take or leave individually. L3 and L6 are the two with real value
   (both add an explanation a reader currently has to derive); L1 is the most arguable item
   in this report.

If only one thing is done, do H4. If only one *change* is wanted, do H1 — it is a comment fix
that removes the likeliest path to a future regression in a crate two filesystems depend on.
