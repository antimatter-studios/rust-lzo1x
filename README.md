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

There is also a CLI, for working with raw blocks by hand:

```sh
lzo1x compress   input.bin  block.lzo1x
lzo1x decompress block.lzo1x output.bin 4096   # size is not in the stream
```

Raw blocks, deliberately — no container, no framing. That is what a Btrfs
extent or a SquashFS block holds. The reference CLI speaks only its own
container format, so the two are not interchangeable; the test suite translates
between them.

## Scope

| | |
|---|---|
| Variants decoded | LZO1X-1, LZO1X-1-15, LZO1X-999 (one shared grammar) |
| Encoder output | LZO1X, using two of the four match buckets (see below) |
| Dependencies | none |
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

## Building

```sh
cargo build --release
cargo clippy --all-targets -- -D warnings
```

The crate builds as both an `rlib` and a `staticlib`, so it can be linked into
a Rust dependency graph or into a C/Swift/Go consumer alongside its siblings.

Install the git hooks once per clone:

```sh
./scripts/install-hooks.sh
```

## License

MIT — see [LICENSE](LICENSE).
