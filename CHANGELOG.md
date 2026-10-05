# Changelog

Notable changes to `am-lzo1x`, newest first. This is a `0.x` crate, so the
**minor** is the compatibility boundary: a minor bump may break API, a patch
never does.

## [Unreleased]

## [0.3.1] — 2026-10-05

### Fixed

- **The release attaches the tool's tarballs.** 0.3.0 published the crate
  and then failed to package the tool on both platforms, for a missing
  `packaging/CAVEATS`; a test now requires the file on every pull request
  (#43).

## [0.3.0] — 2026-10-05

### Added

- **`.lzo` files, read and written the way gzip handles `.gz` (#39).**
  `lzo1x FILE` makes `FILE.lzo` and removes `FILE`; `-d` gives it back with
  its mode and modification time; `-k`, `-c`, `-f`, `-o`, `-t`, `-l` and
  standard input to standard output behave as they do in gzip. The container
  is `lzo1x::lzop`: streamed in 256 KiB blocks, every header and block checksum
  (Adler-32 or CRC-32) checked, a damaged or truncated file refused by name
  and never left half-written. `lzop` reads what it writes and it reads what
  `lzop` writes (`tests/oracle_lzop_files.rs`).

### Changed

- **Raw blocks are `--raw`.** `lzo1x compress IN OUT` is now
  `lzo1x --raw -o OUT IN`, and `lzo1x decompress IN OUT SIZE` is
  `lzo1x --raw -d --size SIZE -o OUT IN`; the tool's plain form is the `.lzo`
  file.

### Added

- **The `lzo1x` tool is tested, packaged and released (#38).** It is now one
  multi-call binary, `rust-lzo1x`, linked as `lzo1x`, built with the `cli`
  feature on the family's shared CLI plumbing: `--version`, `--help` with
  examples, structured errors, man pages and completions. Each release
  attaches a tarball packaged and attested by rust-fs-core's release-cli
  workflow. `tests/cli.rs` runs the binary as a user would: round trips from
  0 bytes to 1 MiB, and every wrong command line, file and stream refused
  with a reason and no panic. The default build no longer produces a binary;
  the library still has no dependencies.

- **An agent guide, `AGENTS.md`, with `CLAUDE.md` importing it.** It carries
  the agent-core block shared byte-identically across the sibling
  repositories, then what is specific to this crate: the `lzop` oracle in both
  directions, the clean-room rule, the tiers and floors, and what a newcomer
  trips on. `scripts/agents-core-check.sh` fails on drift, and
  `tests/scripts/test-agents-core.sh` proves it refuses a modified, unmarked,
  mis-declared or absent block; CI runs it on both legs.
- Releases carry a build-provenance attestation: the published `.crate` is
  attached to the GitHub release for its tag, checked first against the
  crates.io checksum, and verifiable with `gh attestation verify` (see the
  README, "Verifying a release").
- **Every test tier is quiet, budgeted and floored** (#20). `scripts/tier.sh`
  runs a tier through `rust-fs-core`'s canonical `output-budget.sh` — resolved
  from the `../rust-fs-core` sibling at run time, verified by its `--version`
  string and never committed here — so the transcript goes to
  `tmp/logs/<tier>.log`, a pass prints one verdict line and a run that printed
  more than its measured budget exits 65. A green CI run printed 2,132 lines
  across its two legs, 627 of them these four steps, two of which captured
  their log and then `cat` it straight back on every run.
- `chores.yml` gains the four tiers CI actually runs — release, release
  oracle, debug, debug oracle — each with a measured budget and the floor that
  used to live inline in `ci.yml` alone, where nobody running the suite
  locally ever met it.
- CI uploads `tmp/logs/` as an artifact with `if: always()`. There was no
  `actions/upload-artifact` in this repository at all.
- `chore siblings` checks out `../rust-fs-core` for the wrapper, which is the
  only thing this crate reaches outside itself for; it is not a Cargo
  dependency and `cargo build` on a bare checkout is unaffected.

### Changed

- **`chore tools` fails when `lzop` is absent, where `test:oracle` used to
  print `SKIPPED` and exit 0.** A skip that a floor cannot see reads exactly
  like a pass, and the oracle is the only check here that is not this crate
  marking its own homework. The `#[ignore]` gate that keeps a fresh
  `cargo test` green without `lzop` is untouched; what changed is a task
  claiming to have run the oracle when it had not.
- The release floor moves from 26 to 121 and the debug floor from 110 to 121.
  Both were measured before `tests/ci_profile.rs` and `tests/fuzz_decoders.rs`
  existed; 135 tests execute in each profile today.
- `tests/ci_profile.rs` reads a tier wrapper as the run it wraps, so every
  judgement it makes about a `cargo test` — whether its status reaches the
  step, whether an `&&` list can skip it, whether it receives the overflow
  handshake — still applies. What it can no longer read off the text, that the
  wrapper passes the command's own status through, is proved by running it in
  `tests/scripts/test-tier-wrapper.sh`, and `ci_profile.rs` fails if that
  guard is ever removed.

- **The codec is fuzzed, on two tiers.** This crate decompresses bytes it
  did not write, and reaches that input through two others — `am-fs-erofs`
  and `am-fs-squashfs` both hand it blocks lifted straight off a mounted
  image — so a length or distance that walks the output pointer past its
  end is reachable from any EROFS or SquashFS image a user is asked to
  open. `fuzz/` holds `cargo-fuzz` targets for `decompress` and for the
  round trip; `tests/fuzz_decoders.rs` is the gate that replays and
  mutates the same corpus deterministically on the stable toolchain,
  38,912 cases in under five seconds. It fails on a hang as well as a
  panic, naming the target, seed and case, and refuses a case count below
  a floor.

  The corpus is streams `lzop` produced, lifted out of its container by
  `scripts/make-fuzz-corpus.sh`, each carrying the length `lzop` recorded
  for it. That makes the corpus an oracle as well as fuel: this crate is
  now checked against the reference encoder on every pull request, on
  machines with no `lzop` installed — which is the one thing
  `tests/oracle_lzop.rs` cannot do (#18).

## [0.2.0] — 2026-09-04

Minor rather than patch: `compress` is new public API, and for a `0.x`
crate the minor is the compatibility boundary. Nothing existing changed —
`decompress` and `Error` are untouched, so `0.1` callers need no edits.

### Added

- **An LZO1X encoder — `lzo1x::compress`.** Written by inverting this
  crate's own decoder, so it needed no external source and its
  provenance is the decoder's: clean-room, MIT. Inverting a decoder is
  also a much weaker obligation than writing one, because an encoder
  need only emit streams a conforming decoder accepts and is free to use
  a subset of the grammar. This one uses two of the four match buckets;
  the two it omits encode a short match one byte more cheaply, so the
  cost is ratio, not correctness.

  `compress` is infallible. Input with no exploitable redundancy comes
  back larger than it went in — a per-literal-run overhead the format
  cannot avoid — so callers should compare lengths and store the
  original when this is longer, which is what both filesystems using
  this format already do.

- **A CLI, `lzo1x`**, with `compress` and `decompress` subcommands over
  raw blocks — no container, no framing, which is what a Btrfs extent or
  a SquashFS block actually holds. Adds no dependency.

- **Bidirectional cross-validation against the reference implementation.**
  The existing oracle only ran one way: the reference compresses and we
  decompress. That says nothing about an encoder, and neither does
  round-tripping through our own decoder — it is deliberately more
  permissive than the format, so a stream this crate reads back
  perfectly may still be one the kernel refuses. The new tests hand our
  output to the reference and require the bytes back, covering the match
  bucket boundary, the length extensions and the leading-literal-run
  encodings.

  The container these need is built from the same constants the existing
  parser reads, rather than in a second place. The reference tool stays
  at arm's length: separate process, never linked, never copied from.

### Notes

- The encoder reserves three trailing literals rather than ending a
  stream on a match. Measured, not assumed: removing the reserve leaves
  the whole cross-validation gate passing, so the reference decompressor
  does not require it. It stays because LZO ships a second, faster
  decompressor that copies in machine words and wants slack past the end
  of the stream — which this gate cannot exercise. Three bytes is a
  cheap way to stay inside what both variants read.


## [0.1.2] — 2026-09-04

No public API change — the diff touches no `pub` signature, so `^0.1`
consumers are unaffected.

### Changed

- **The decoder's grammar is written down rather than inferred.** The values
  the instruction dispatch turns on now have names: the state sentinel is
  `LONG_LITERAL_RUN_STATE`, the single bit formerly called `h` is
  `dist_high_bit` (and says that it is *not* an extension byte, unlike the
  `h` in the arms either side), and the allocation cap is
  `INITIAL_CAPACITY_CAP`, documented as the third leg of the same defence as
  `max_out` and `MAX_EXTENDED_LENGTH`.
- **`split_operand` expresses the `DDDDDDDD DDDDDDSS` layout once**, replacing
  five open-coded shifts. `>> 2` on an operand and `>> 2` on a command byte
  look identical and mean different things, which is the kind of duplication
  worth removing even at two instances.
- **The "was this block compressed" predicate is computed once.** It had two
  inverted spellings, and it decides both whether a checksum field is present
  and whether the payload is an LZO1X stream at all — so a disagreement
  between them would desync the cursor and misread every later block.

### Fixed

- The invariant the trailing-literal copy depends on — that the state sentinel
  is never live at that point — is now a `debug_assert!` at the site that
  relies on it, instead of a fact the reader had to reconstruct from four
  separate assignment sites.

## [0.1.1] — 2026-08-29

### Added

- Test coverage for every instruction bucket in the decoder.
- A `chore` task owns this crate's build, so how to build it is knowledge that
  lives in this repo rather than in whatever consumes it.
- Release-on-tag workflow.

### Changed

- Pinned toolchain moves to 1.95.0. Every crate in this family moves its
  `rust-toolchain.toml` in lockstep; a straggler links two copies of
  `_rust_eh_personality` into any consumer that binds both.

### Fixed

- The end-of-stream comment described behaviour the code did not have.

### Removed

- A dependency-pinning script that was never wired to anything.

## [0.1.0] — 2026-08-24

### Added

- Initial release: a pure-Rust LZO1X **decompressor**, no `unsafe`, no C
  dependency.

  It exists because the `lzo1x` crate already on crates.io is **GPL-2.0**,
  which this project cannot take a dependency on. That is also why this crate
  is named `am-lzo1x` rather than `lzo1x`. Decompression only — the two
  consumers, `am-fs-squashfs` and `am-fs-btrfs`, both only ever read an LZO1X
  stream. SquashFS has no write path at all, and btrfs writes uncompressed
  extents.

[Unreleased]: https://github.com/antimatter-studios/rust-lzo1x/compare/v0.3.1...HEAD
[0.3.1]: https://github.com/antimatter-studios/rust-lzo1x/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/antimatter-studios/rust-lzo1x/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/antimatter-studios/rust-lzo1x/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/antimatter-studios/rust-lzo1x/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/antimatter-studios/rust-lzo1x/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/antimatter-studios/rust-lzo1x/releases/tag/v0.1.0
