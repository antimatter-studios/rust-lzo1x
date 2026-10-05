# rust-lzo1x

Pure-Rust **LZO1X compressor and decompressor**. No C bindings, no `unsafe`, no
dependencies.

LZO1X is the byte-stream format produced by the LZO family of compressors
(LZO1X-1, LZO1X-1-15, LZO1X-999). All LZO1X-\* encoders share a single decode
grammar, so one decoder handles every variant. The format appears in SquashFS
(compression id 3), Btrfs compressed extents, JFFS2, and kernel crash dumps.

```rust
let packed = lzo1x::compress(&data);
let data = lzo1x::decompress(&packed, max_output_len)?;
```

There is also a command-line tool, `lzo1x`, which handles `.lzo` files the way
`gzip` handles `.gz` files, and raw blocks by hand:

```sh
lzo1x notes.txt                      # makes notes.txt.lzo, removes notes.txt
lzo1x -d notes.txt.lzo               # notes.txt back, with its mode and time
lzo1x -k -c big.bin > big.bin.lzo    # -k keeps the input, -c writes to stdout
tar cf - dir | lzo1x > dir.tar.lzo   # standard input to standard output
lzo1x -t dir.tar.lzo                 # check every checksum and block
lzo1x -l dir.tar.lzo                 # the name and sizes
lzo1x --raw -o block.lzo1x input.bin
lzo1x --raw -d --size 4096 -o output.bin block.lzo1x   # a raw block's size is not in it
```

The `.lzo` files are the ones `lzop` reads and writes: `lzop` tests and
decompresses what `lzo1x` writes, and `lzo1x` reads what `lzop` writes at any
level and with either checksum (`tests/oracle_lzop_files.rs`). The container was
implemented from files `lzop` wrote, measured byte by byte, not from its code.

Install it with `brew install antimatter-studios/tap/rust-lzo1x`, or download
the tarball attached to each GitHub release (an install prefix: `bin/`, man
pages and shell completions). It is one multi-call binary, `rust-lzo1x`, linked
as `lzo1x`; `rust-lzo1x lzo1x ...` is the same program under the name nothing
else on `PATH` can shadow. Building it needs the `cli` feature
(`cargo build --release --features cli`), which is also the only thing that
gives this crate dependencies: the library has none.

`--raw` is a raw block: no container, no framing. That is what a Btrfs extent or
a SquashFS block holds, and what the library's `compress` and `decompress` take
and return.

## Scope

| | |
|---|---|
| Variants decoded | LZO1X-1, LZO1X-1-15, LZO1X-999 (one shared grammar) |
| Encoder output | LZO1X, using two of the four match buckets (see below) |
| Dependencies | none (the command-line tool, behind the `cli` feature, uses clap and the family's shared CLI plumbing) |
| `unsafe` | none (`#![forbid(unsafe_code)]`) |
| Untrusted input | safe — bounds-checked at every step, errors instead of panicking |
| MSRV | 1.94.1 (see `rust-toolchain.toml`) |

The decoder takes a `max_out` upper bound so a corrupt stream cannot drive an
unbounded allocation. The true output length comes from the stream's own
end-of-stream marker, not from that bound.

### About the encoder

It emits two of the format's four match buckets: one for distances up to 16384
and one above. The two omitted buckets encode a short match one byte more
cheaply, so leaving them out costs ratio, not correctness — and halves the
number of field-packing paths that can be got wrong.

`compress` is infallible. An input with no exploitable redundancy comes back
larger than it went in; there is a per-literal-run overhead the format cannot
avoid. Callers that care should compare lengths and store the original when
this is longer, which is what both filesystems using this format do.

The decoder takes a `max_out` upper bound so a corrupt stream cannot drive an
unbounded allocation. The true output length comes from the stream's own
end-of-stream marker, not from that bound.

## Provenance

This decoder was written from the publicly published prose description of the
LZO1X byte-stream grammar, principally the format-description document
distributed as `Documentation/staging/lzo.rst` in the Linux source tree — a
prose specification of the on-the-wire token grammar, not decompressor code.
The grammar as used is reproduced in the crate-level documentation so the
mapping from specification to implementation is auditable in-tree.

The encoder needed no external source at all: it is this crate's own decoder
run backwards. Inverting a decoder is also a far weaker obligation than writing
one — a decoder must accept every stream a conforming encoder can produce,
while an encoder need only produce streams a conforming decoder accepts, and is
free to use a subset of the grammar.

Both are MIT. The `lzo1x` crate name on crates.io belongs to an unrelated
GPL-2.0 implementation, which is why this one is published as `am-lzo1x`.

## How the encoder is checked

Round-tripping through our own decoder proves very little. That decoder is
deliberately more permissive than the format in at least one place, so an
encoder validated only against it could emit a stream this crate reads back
perfectly and the kernel refuses.

So the test contract is bidirectional, against the reference implementation
invoked as an external process (`tests/oracle_lzop.rs`):

| direction | proves |
|---|---|
| reference compresses → we decompress | the decoder accepts real streams, from all three reference encoders |
| we compress → reference decompresses | the encoder emits streams the world accepts |

The second is the one that cannot be obtained any other way. Both need the
reference CLI, so those tests are `#[ignore]`-gated and a fresh checkout still
has a green `cargo test`; CI installs it and opts in with
`cargo test -- --ignored`.

The reference tool is used at arm's length — separate process, never linked,
never copied from. It is an oracle, in the same way a filesystem driver's tests
shell out to the canonical `mkfs`.

A byte-stream format is a set of facts, not an expressive work, and this crate
implements one from its published description. It carries no code from any
other LZO implementation. The crate is MIT licensed and has no copyleft
dependencies — deliberately, since the widely-used LZO implementations are
GPL-licensed and unusable in a permissively-licensed project.

## Test contract

Two layers, because one alone would be self-confirming.

1. **Unit tests** (`src/lib.rs`) decode streams hand-built from the grammar,
   covering each instruction bucket, the zero-run length extension, overlapping
   matches, and every malformed-input rejection path. These prove internal
   consistency.

2. **Oracle tests** (`tests/oracle_lzop.rs`) decode streams emitted by `lzop`,
   the reference LZO command-line compressor, and require the original payload
   back. This is the layer that catches a *misreading of the specification* —
   hand-built streams can only ever confirm the reader's own interpretation.
   Covered: both encoders (`-1` and `-9`), multi-block payloads, mixed
   compressibility, incompressible input, and a length sweep across the
   short-match and literal-run boundaries.

`lzop` is invoked as an external process. Nothing from it is linked, copied, or
redistributed.

```sh
cargo test              # unit tests; green without lzop installed
cargo test -- --ignored # adds the oracle tests (requires lzop)
```

Install `lzop` with `brew install lzop` or `apt-get install lzop`.

### The tiers, quietly

`chore` runs the same four selections CI does, each through
`scripts/tier.sh`: the whole transcript goes to `tmp/logs/<tier>.log`, a pass
prints one verdict line naming it, and a run that passed but printed more than
its measured budget exits **65**.

```sh
chore siblings            # check out ../rust-fs-core, which owns the wrapper
chore test                # every tier: release, release oracle, debug, debug oracle
chore test:debug          # one tier
chore test -- --verbose   # stream it as well; the budget still applies
```

The budgets and the executed-test floors are in `chores.yml`, measured, in a
table at the top of it. `OUTPUT_BUDGET_FAIL_TAIL=40` brings back the tail of a
failure for whoever is watching.

`chore tools` **fails** when `lzop` is absent rather than printing a skip: the
oracle is the only check here that is not this crate marking its own homework,
and a skip reads exactly like a pass.

## Building

```sh
cargo build --release
cargo clippy --all-targets -- -D warnings
```

The crate builds as both an `rlib` and a `staticlib`, so it can be linked into
a Rust dependency graph or into a C/Swift/Go consumer alongside its siblings.

Install the git hooks once per clone:

```sh
~/.claude/skills/github-guard/install.sh .
```

The guards live in `.git/hooks`, outside the working tree, so no branch
checkout can rewrite the hook that is about to run. They are per-clone rather
than tracked: re-run the installer in a fresh clone, and after the guards are
updated.

## Verifying a release

From the next release onward, every version published to crates.io is
also attached to the GitHub release for its tag, with a build-provenance
attestation signed by this repository's release workflow. It proves the
crate was built by `.github/workflows/release.yml` from a commit in this
repository, not uploaded from someone's machine. To check the crates.io
download of version `X.Y.Z`:

```sh
curl -sSfLo am-lzo1x-X.Y.Z.crate https://static.crates.io/crates/am-lzo1x/am-lzo1x-X.Y.Z.crate
gh attestation verify am-lzo1x-X.Y.Z.crate \
  --repo antimatter-studios/rust-lzo1x \
  --signer-workflow antimatter-studios/rust-lzo1x/.github/workflows/release.yml
```

The workflow refuses to attest a `.crate` whose sha256 differs from the
checksum crates.io records for that version, so the file on the release
page and the crates.io download are the same bytes.

## License

MIT — see [LICENSE](LICENSE).
