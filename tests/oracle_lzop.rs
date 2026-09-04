//! Oracle tests: decode LZO1X streams produced by a real LZO compressor.
//!
//! The unit tests in `src/lib.rs` decode streams this project hand-built
//! from the grammar. That proves internal consistency, not correctness —
//! a misreading of the spec would produce an encoder-shaped blind spot
//! that hand-built streams can never expose. These tests close that gap
//! by feeding the decoder bytes emitted by `lzop` (the reference LZO
//! command-line compressor) and requiring the original payload back.
//!
//! `lzop` is invoked as an external process only. Nothing from it is
//! linked, copied, or redistributed — it is a test oracle in the same way
//! a filesystem driver's tests shell out to the canonical `mkfs`.
//!
//! All tests here are `#[ignore]`-gated so a fresh checkout without
//! `lzop` still has a green `cargo test`. Opt in with:
//!
//! ```sh
//! cargo test -- --ignored
//! ```

use std::io::Write;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

// ---------------------------------------------------------------------
// lzop container parsing
//
// A `.lzo` file is a small header followed by a sequence of blocks. Only
// the block payloads are LZO1X streams; the container itself is lzop's
// own framing and is documented in the lzop distribution. We parse just
// enough of it to reach the compressed blocks.
// ---------------------------------------------------------------------

const LZOP_MAGIC: [u8; 9] = [0x89, b'L', b'Z', b'O', 0x00, 0x0d, 0x0a, 0x1a, 0x0a];

const F_ADLER32_D: u32 = 0x0000_0001;
const F_ADLER32_C: u32 = 0x0000_0002;
const F_H_FILTER: u32 = 0x0000_0800;
const F_CRC32_D: u32 = 0x0000_0100;
const F_CRC32_C: u32 = 0x0000_0200;

struct Cursor<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Cursor<'a> {
    fn u8(&mut self) -> u8 {
        let v = self.b[self.i];
        self.i += 1;
        v
    }
    /// lzop stores every multi-byte header field in network byte order.
    fn u16(&mut self) -> u16 {
        let v = u16::from_be_bytes([self.b[self.i], self.b[self.i + 1]]);
        self.i += 2;
        v
    }
    fn u32(&mut self) -> u32 {
        let v = u32::from_be_bytes([
            self.b[self.i],
            self.b[self.i + 1],
            self.b[self.i + 2],
            self.b[self.i + 3],
        ]);
        self.i += 4;
        v
    }
    fn skip(&mut self, n: usize) {
        self.i += n;
    }
}

/// One block of an lzop file: the raw LZO1X stream plus the length the
/// container says it decodes to.
struct Block {
    compressed: Vec<u8>,
    uncompressed_len: usize,
    /// lzop stores a block verbatim when compression did not shrink it.
    /// Those blocks are not LZO1X streams and must be skipped.
    stored: bool,
}

fn parse_lzop(data: &[u8]) -> Vec<Block> {
    assert_eq!(&data[..9], &LZOP_MAGIC, "not an lzop file");
    let mut c = Cursor { b: data, i: 9 };

    let version = c.u16();
    let _lib_version = c.u16();
    if version >= 0x0940 {
        let _version_needed = c.u16();
    }
    let _method = c.u8();
    if version >= 0x0940 {
        let _level = c.u8();
    }
    let flags = c.u32();
    if flags & F_H_FILTER != 0 {
        let _filter = c.u32();
    }
    let _mode = c.u32();
    let _mtime_low = c.u32();
    if version >= 0x0940 {
        let _mtime_high = c.u32();
    }
    let fname_len = c.u8() as usize;
    c.skip(fname_len);
    let _header_checksum = c.u32();

    let d_csum = flags & (F_ADLER32_D | F_CRC32_D) != 0;
    let c_csum = flags & (F_ADLER32_C | F_CRC32_C) != 0;

    let mut blocks = Vec::new();
    loop {
        let uncompressed_len = c.u32() as usize;
        if uncompressed_len == 0 {
            break; // end-of-file marker
        }
        let compressed_len = c.u32() as usize;
        if d_csum {
            let _ = c.u32();
        }
        // One domain question — did the encoder actually compress this
        // block — decides two things eight lines apart: whether a
        // compressed-side checksum is present in the stream, and whether
        // the payload is an LZO1X stream or the original bytes. Written
        // twice in inverted form, a disagreement between them would
        // desync the cursor and every later block would be read from the
        // wrong offset, surfacing as a confusing decode failure rather
        // than a parse error. Asked once.
        let stored = compressed_len >= uncompressed_len;

        // The compressed-side checksum is only written when the block
        // actually got compressed.
        if c_csum && !stored {
            let _ = c.u32();
        }
        let payload = data[c.i..c.i + compressed_len].to_vec();
        c.skip(compressed_len);
        blocks.push(Block {
            compressed: payload,
            uncompressed_len,
            stored,
        });
    }
    blocks
}

/// Compress `payload` with `lzop` at the given level and return the
/// parsed blocks. Returns `None` if `lzop` is not installed.
fn lzop_blocks(payload: &[u8], level: &str) -> Option<Vec<Block>> {
    if Command::new("lzop").arg("--version").output().is_err() {
        return None;
    }
    // Tests run in parallel and several use the same payload length, so
    // the directory name needs more than pid+len to stay unique — a
    // collision lets one test delete another's input mid-run.
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "lzo1x-oracle-{}-{}-{}-{}",
        std::process::id(),
        level.trim_start_matches('-'),
        payload.len(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("payload.bin");
    let mut f = std::fs::File::create(&raw).unwrap();
    f.write_all(payload).unwrap();
    drop(f);

    let out = Command::new("lzop")
        .arg(level)
        .arg("-f")
        .arg("-o")
        .arg(dir.join("payload.lzo"))
        .arg(&raw)
        .output()
        .expect("run lzop");
    assert!(
        out.status.success(),
        "lzop failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let data = std::fs::read(dir.join("payload.lzo")).unwrap();
    let blocks = parse_lzop(&data);
    std::fs::remove_dir_all(&dir).ok();
    Some(blocks)
}

/// Decode every compressed block of `payload` and require the bytes back.
///
/// `expect_compression` guards against a test that silently proves
/// nothing: if the encoder stored every block verbatim, no LZO1X stream
/// was ever decoded. Deliberately incompressible payloads pass `false`.
fn assert_roundtrip_inner(payload: &[u8], level: &str, expect_compression: bool) {
    let Some(blocks) = lzop_blocks(payload, level) else {
        eprintln!("lzop not installed — skipping");
        return;
    };
    let mut decoded = Vec::new();
    let mut saw_compressed = false;
    for b in &blocks {
        if b.stored {
            // Incompressible block stored verbatim by the container.
            decoded.extend_from_slice(&b.compressed);
            continue;
        }
        saw_compressed = true;
        let out = lzo1x::decompress(&b.compressed, b.uncompressed_len)
            .unwrap_or_else(|e| panic!("{level}: decode failed on a real lzop block: {e}"));
        assert_eq!(
            out.len(),
            b.uncompressed_len,
            "{level}: block decoded to {} bytes, container says {}",
            out.len(),
            b.uncompressed_len
        );
        decoded.extend_from_slice(&out);
    }
    assert_eq!(decoded, payload, "{level}: round-trip mismatch");
    if expect_compression {
        assert!(
            saw_compressed,
            "{level}: nothing was actually compressed — the test proved nothing"
        );
    }
}

/// Round-trip a payload that the encoder is expected to compress.
fn assert_roundtrip(payload: &[u8], level: &str) {
    assert_roundtrip_inner(payload, level, true);
}

/// Highly compressible input: long literal runs plus heavy repetition,
/// which drives the long-match and zero-run-extension paths.
fn repetitive(len: usize) -> Vec<u8> {
    "the quick brown fox jumps over the lazy dog\n"
        .bytes()
        .cycle()
        .take(len)
        .collect()
}

/// Deterministic pseudo-random bytes. A simple xorshift keeps the test
/// reproducible without pulling in an RNG dependency.
fn pseudo_random(len: usize, seed: u64) -> Vec<u8> {
    let mut s = seed | 1;
    (0..len)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 24) as u8
        })
        .collect()
}

#[test]
#[ignore = "requires lzop"]
fn roundtrip_repetitive_fast() {
    assert_roundtrip(&repetitive(64 * 1024), "-1");
}

#[test]
#[ignore = "requires lzop"]
fn roundtrip_repetitive_best() {
    // -9 selects LZO1X-999, a different encoder to -1's LZO1X-1. Both
    // emit the same grammar, so both must decode.
    assert_roundtrip(&repetitive(64 * 1024), "-9");
}

#[test]
#[ignore = "requires lzop"]
fn roundtrip_large_multiblock() {
    // Larger than lzop's default block size, so the container holds
    // several independent LZO1X streams.
    assert_roundtrip(&repetitive(1024 * 1024), "-9");
}

#[test]
#[ignore = "requires lzop"]
fn roundtrip_mixed_compressibility() {
    // Alternating compressible and incompressible regions: exercises
    // long literal runs (which the encoder falls back to on noise)
    // interleaved with matches.
    let mut v = Vec::new();
    for i in 0..16 {
        v.extend_from_slice(&repetitive(4096));
        v.extend_from_slice(&pseudo_random(4096, i + 1));
    }
    assert_roundtrip(&v, "-9");
}

#[test]
#[ignore = "requires lzop"]
fn roundtrip_incompressible() {
    // Pure noise: the encoder stores every block verbatim, so this
    // asserts the container path rather than the decoder. Kept because a
    // decoder that mistakenly tried to decode a stored block would fail
    // here, and that is a real regression.
    assert_roundtrip_inner(&pseudo_random(256 * 1024, 0xDEAD_BEEF), "-9", false);
}

#[test]
#[ignore = "requires lzop"]
fn roundtrip_many_small_sizes() {
    // Sweep lengths around the encoder's short-match and literal-run
    // boundaries, where off-by-one grammar errors hide.
    for len in [
        1usize, 2, 3, 4, 5, 15, 16, 17, 18, 19, 31, 32, 33, 255, 256, 257, 1023, 1024,
    ] {
        // Very short payloads have nothing to match against, so the
        // encoder may legitimately store them.
        assert_roundtrip_inner(&repetitive(len), "-9", len >= 256);
    }
}

// =====================================================================
// The other direction: we compress, the reference decompresses.
//
// Everything above proves the DECODER accepts real streams. It says
// nothing about the encoder, and the encoder cannot be checked by
// round-tripping through our own decoder either: that decoder is
// deliberately more permissive than the format (see the end-of-stream
// comment in src/lib.rs), so a stream this crate reads back perfectly
// may still be one the reference refuses.
//
// The only way to know is to hand our output to the reference and see
// whether it gives the bytes back. That needs a container built around
// our raw block, which is the same format parsed above — so it is built
// here, from the same constants, rather than in a second place.
// =====================================================================

/// Exit status used by [`lzop_decompress`] to mean "this input cannot be
/// expressed in the container", as distinct from "the reference rejected
/// our stream". Conflating them would let an unrepresentable input read
/// as a passing check.
enum Wrapped {
    Container(Vec<u8>),
    /// The container identifies a compressed block SOLELY by its length
    /// being below the uncompressed length — there is no flag. So a
    /// block that did not shrink cannot be carried as compressed at all;
    /// the reference would read it back as raw bytes and report a
    /// checksum error that looks exactly like a broken encoder.
    NotRepresentable(&'static str),
}

/// Build an lzop container around one raw block we produced.
fn wrap_lzop(block: &[u8], original: &[u8]) -> Wrapped {
    if original.is_empty() {
        // A block declaring zero uncompressed bytes IS the end-of-blocks
        // marker, so an empty payload has no representation. Nothing to
        // do with our stream — the unit tests cover empty input.
        return Wrapped::NotRepresentable("empty payload has no container representation");
    }
    if block.len() >= original.len() {
        return Wrapped::NotRepresentable("block did not shrink, so it cannot be carried");
    }

    let mut header = Vec::new();
    header.extend_from_slice(&0x1030u16.to_be_bytes()); // version
    header.extend_from_slice(&0x20A0u16.to_be_bytes()); // library version
    header.extend_from_slice(&0x0940u16.to_be_bytes()); // version needed
    header.push(1); // method: LZO1X-1
    header.push(1); // level
                    // Checksum the UNCOMPRESSED side only. The reference verifies it
                    // after decoding, so it is a second, independent check that our
                    // encoder preserved the bytes — not merely that it produced
                    // something decodable.
    header.extend_from_slice(&F_ADLER32_D.to_be_bytes());
    header.extend_from_slice(&0o644u32.to_be_bytes()); // mode
    header.extend_from_slice(&0u32.to_be_bytes()); // mtime low
    header.extend_from_slice(&0u32.to_be_bytes()); // mtime high
    header.push(0); // no filename
    let header_checksum = adler32(&header);
    header.extend_from_slice(&header_checksum.to_be_bytes());

    let mut out = Vec::new();
    out.extend_from_slice(&LZOP_MAGIC);
    out.extend_from_slice(&header);
    out.extend_from_slice(&(original.len() as u32).to_be_bytes());
    out.extend_from_slice(&(block.len() as u32).to_be_bytes());
    out.extend_from_slice(&adler32(original).to_be_bytes());
    out.extend_from_slice(block);
    out.extend_from_slice(&0u32.to_be_bytes()); // end of blocks
    Wrapped::Container(out)
}

/// Adler-32, as the container specifies. Written out rather than pulled
/// in as a dependency: this crate has none, and a test is not a reason
/// to acquire the first one.
fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
}

/// Compress `payload` with THIS crate, hand it to the reference, and
/// require the original bytes back.
///
/// Returns `false` when the check could not run, so a caller can tell a
/// skip from a pass.
fn assert_reference_reads_our_stream(payload: &[u8]) -> bool {
    if Command::new("lzop").arg("--version").output().is_err() {
        eprintln!("lzop not installed — skipping");
        return false;
    }

    let block = lzo1x::compress(payload);
    let container = match wrap_lzop(&block, payload) {
        Wrapped::Container(c) => c,
        Wrapped::NotRepresentable(why) => {
            eprintln!("skipping {} byte payload: {why}", payload.len());
            return false;
        }
    };

    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "lzo1x-emit-{}-{}-{}",
        std::process::id(),
        payload.len(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let packed = dir.join("ours.lzo");
    std::fs::write(&packed, &container).unwrap();

    let unpacked = dir.join("ours.out");
    let out = Command::new("lzop")
        .arg("-d")
        .arg("-f")
        .arg("-o")
        .arg(&unpacked)
        .arg(&packed)
        .output()
        .expect("run lzop");

    let verdict = if out.status.success() {
        let back = std::fs::read(&unpacked).unwrap();
        assert_eq!(
            back.len(),
            payload.len(),
            "the reference decoded our stream to {} bytes, expected {}",
            back.len(),
            payload.len()
        );
        assert!(
            back == payload,
            "the reference decoded our stream to different bytes"
        );
        true
    } else {
        panic!(
            "the reference refused a stream we produced ({} bytes -> {} bytes): {}",
            payload.len(),
            block.len(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    };
    std::fs::remove_dir_all(&dir).ok();
    verdict
}

#[test]
#[ignore = "requires lzop"]
fn the_reference_reads_what_we_emit() {
    let mut ran = 0;
    for payload in [
        repetitive(64 * 1024),
        repetitive(4096),
        repetitive(300),
        vec![b'A'; 100_000],
        // Long literal runs interleaved with matches: the encoder has to
        // switch between the inline and long-form literal encodings.
        {
            let mut v = Vec::new();
            for i in 0..16 {
                v.extend_from_slice(&repetitive(4096));
                v.extend_from_slice(&pseudo_random(4096, i + 1));
            }
            v
        },
        // Structured binary, the shape a filesystem block actually has.
        (0..=255u8).cycle().take(16 * 1024).collect(),
    ] {
        if assert_reference_reads_our_stream(&payload) {
            ran += 1;
        }
    }
    assert!(
        ran > 0,
        "no payload was actually checked — the test proved nothing"
    );
}

/// Back-reference distances at and either side of the boundary between
/// the two match buckets this encoder uses.
///
/// The boundary is where an off-by-one is silent rather than loud: a
/// distance that should use the ≤16384 bucket, encoded in the other one,
/// produces a token contributing no distance — which IS the
/// end-of-stream marker. The stream truncates and decodes cleanly to
/// fewer bytes.
///
/// The filler is repetitive, not random. A random filler makes the
/// payload incompressible, the container cannot carry a block that did
/// not shrink, and the check would skip — leaving the boundary these
/// cases exist to test entirely unexercised.
#[test]
#[ignore = "requires lzop"]
fn the_reference_reads_our_matches_across_the_bucket_boundary() {
    const MARKER: &[u8] = b"MATCHME!";
    let mut ran = 0;
    for distance in [16_383usize, 16_384, 16_385, 32_768, 49_150, 60_000] {
        let filler: Vec<u8> = b"0123456789abcdef"
            .iter()
            .copied()
            .cycle()
            .take(distance - MARKER.len())
            .collect();
        let mut payload = Vec::with_capacity(distance + 16);
        payload.extend_from_slice(MARKER);
        payload.extend_from_slice(&filler);
        payload.extend_from_slice(MARKER);
        payload.extend_from_slice(b"tail");
        if assert_reference_reads_our_stream(&payload) {
            ran += 1;
        }
    }
    assert_eq!(ran, 6, "some boundary cases did not run");
}

/// Lengths either side of where the zero-run extension needs another
/// continuation byte, on both the literal and the match side.
#[test]
#[ignore = "requires lzop"]
fn the_reference_reads_our_length_extensions() {
    let mut ran = 0;
    for len in [253usize, 254, 255, 256, 257, 509, 510, 511, 1023, 1024] {
        if assert_reference_reads_our_stream(&vec![b'Z'; len]) {
            ran += 1;
        }
    }
    assert!(ran >= 8, "only {ran} of 10 length cases ran");
}

/// A leading literal run of 238 bytes is the longest the first command
/// byte can carry as `length + 17`; at 239 the encoding changes shape.
/// Each payload carries a compressible tail so the block shrinks and the
/// container can hold it — otherwise the very case being tested skips.
#[test]
#[ignore = "requires lzop"]
fn the_reference_reads_our_leading_literal_runs() {
    let mut ran = 0;
    for len in [17usize, 237, 238, 239, 240, 4096] {
        let mut payload = pseudo_random(len, len as u64 + 1);
        payload.extend_from_slice(&repetitive(2048));
        if assert_reference_reads_our_stream(&payload) {
            ran += 1;
        }
    }
    assert_eq!(ran, 6, "some leading-literal cases did not run");
}
