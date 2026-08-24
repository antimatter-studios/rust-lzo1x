# rust-lzo1x

Pure-Rust **LZO1X decompressor**. No C bindings, no `unsafe`, no dependencies.

LZO1X is the byte-stream format produced by the LZO family of compressors
(LZO1X-1, LZO1X-1-15, LZO1X-999). All LZO1X-\* encoders share a single decode
grammar, so one decoder handles every variant. The format appears in SquashFS
(compression id 3), Btrfs compressed extents, JFFS2, and kernel crash dumps.

```rust
let data = lzo1x::decompress(&compressed, max_output_len)?;
```

## Scope

**Decode only.** The decode grammar is small and fully published; a competitive
encoder is a far larger undertaking with no consumer in sight. If you need to
*produce* LZO streams, use the reference compressor.

| | |
|---|---|
| Variants decoded | LZO1X-1, LZO1X-1-15, LZO1X-999 (one shared grammar) |
| Dependencies | none |
| `unsafe` | none (`#![forbid(unsafe_code)]`) |
| Untrusted input | safe — bounds-checked at every step, errors instead of panicking |
| MSRV | 1.94.1 (see `rust-toolchain.toml`) |

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
