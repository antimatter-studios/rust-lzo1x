# Changelog

Notable changes to `am-lzo1x`, newest first. This is a `0.x` crate, so the
**minor** is the compatibility boundary: a minor bump may break API, a patch
never does.

## [Unreleased]

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

[Unreleased]: https://github.com/antimatter-studios/rust-lzo1x/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/antimatter-studios/rust-lzo1x/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/antimatter-studios/rust-lzo1x/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/antimatter-studios/rust-lzo1x/releases/tag/v0.1.0
