//! The stable-toolchain half of the fuzzing setup: replay the corpus,
//! then mutate it, and refuse if the decoder panics, hangs, or if the
//! suite quietly stopped doing any work.
//!
//! # Why this crate is worth fuzzing before the others
//!
//! `decompress` takes a byte stream and a bound, and everything in
//! between is attacker-controlled: token lengths, literal runs, and
//! back-reference distances that index backwards into output already
//! produced. LZO decoders have a long history of exactly one defect --
//! a length or a distance that walks the output pointer past where it
//! should stop -- and this one is reached through *two* other crates.
//! `rust-fs-erofs` and `rust-fs-squashfs` both hand it blocks lifted
//! straight off a mounted image, so a defect here is reachable from any
//! EROFS or SquashFS image a user is asked to open.
//!
//! # Why there are two halves
//!
//! `fuzz/` holds `cargo-fuzz` targets: the explorer, which runs for as
//! long as you give it and finds inputs nobody thought of. It cannot be
//! a required check, because how long it ran decides what it found.
//!
//! This suite is the gate. Deterministic, under a second, on the stable
//! toolchain, in every pull request, reading the same `fuzz/corpus/`
//! the explorer does. Anything the explorer finds is committed there
//! and replayed here from then on.
//!
//! # Why the corpus is lzop's output
//!
//! Every seed is an LZO1X stream the reference encoder produced, lifted
//! out of its container by `scripts/make-fuzz-corpus.sh`. Random bytes
//! are refused by the first token and never reach the match-copy loop,
//! which is the part worth testing. Each seed carries the length it
//! decodes to in a four-byte little-endian prefix, so the corpus is
//! also an oracle: `the_corpus_decodes_to_the_length_lzop_recorded`
//! checks this crate against `lzop` without needing `lzop` present.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

/// Distinct starting points for the mutation stream. Fixed, so a
/// failure reproduces from the message alone.
const SEEDS: u64 = 8;

/// Mutated cases per (corpus file, seed) pair.
const CASES_PER_SEED: usize = 256;

/// Below this, the suite is not doing its job.
const CASE_FLOOR: usize = 20_000;

/// Long enough that a loaded machine is never the reason, short enough
/// that a genuine hang is reported rather than left to the job timeout.
const DEADLINE: Duration = Duration::from_secs(120);

/// A decode bound a caller might plausibly pass. EROFS and SquashFS
/// both decompress into a block-sized buffer, so these are the sizes
/// that matter, plus one far larger than any seed needs.
const BOUNDS: [usize; 5] = [0, 4096, 65_536, 262_144, 1 << 22];

/// A bound no seed in the corpus needs, so the decode runs to the
/// stream's own end-of-stream marker rather than stopping at the bound.
const GENEROUS: usize = 1 << 20;

/// How much of a mutated seed the round-trip arm compresses. See the
/// comment on that target for why it is bounded at all.
const ROUNDTRIP_CAP: usize = 8192;

// ---------------------------------------------------------------- targets

struct Target {
    corpus: &'static str,
    name: &'static str,
    /// Whether this corpus's files carry the four-byte little-endian
    /// decoded-length prefix `scripts/make-fuzz-corpus.sh` writes.
    ///
    /// Compressed streams do, because a stream is only useful as an
    /// oracle if something records what it decodes to. Round-trip
    /// payloads do not: they are the input, and their length is their
    /// length.
    prefixed: bool,
    run: fn(&[u8]),
}

fn targets() -> Vec<Target> {
    vec![
        Target {
            corpus: "decompress",
            name: "decompress",
            prefixed: true,
            run: |stream| {
                // One bound chosen by the stream itself and one
                // generous, rather than all five every time. Five
                // decodes per case meant most of the run was spent
                // producing the same megabytes of output again; the
                // choice is still deterministic, so a failure still
                // reproduces from its case number.
                let pick = usize::from(stream.first().copied().unwrap_or(0));
                let _ = lzo1x::decompress(stream, BOUNDS[pick % BOUNDS.len()]);
                let _ = lzo1x::decompress(stream, GENEROUS);
            },
        },
        Target {
            corpus: "roundtrip",
            name: "roundtrip",
            prefixed: false,
            run: |stream| {
                // The stream is treated as an arbitrary payload here,
                // not as compressed data: whatever `compress` makes of
                // it must decode back to exactly it. A codec that loses
                // a byte is a defect no amount of not-panicking covers.
                //
                // Capped, because the round-trip property does not get
                // truer with size: every token shape the encoder can
                // emit is reachable within a few kilobytes, and the
                // uncapped version spent fifty seconds compressing the
                // same 200 KB seeds over and over. The explorer is
                // where large inputs belong, and libFuzzer generates
                // them for free.
                let stream = &stream[..stream.len().min(ROUNDTRIP_CAP)];
                let packed = lzo1x::compress(stream);
                match lzo1x::decompress(&packed, stream.len()) {
                    Ok(back) => assert_eq!(
                        back,
                        stream,
                        "compress then decompress did not return the input, at {} bytes",
                        stream.len()
                    ),
                    Err(e) => panic!(
                        "this crate could not decode its own output for a {}-byte input: {e}",
                        stream.len()
                    ),
                }
            },
        },
    ]
}

// ---------------------------------------------------------------- corpus

fn corpus_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fuzz/corpus")
}

/// One seed: its bytes, and — for a compressed stream — the length
/// `lzop` said it decodes to.
struct Seed {
    name: String,
    decoded_len: Option<usize>,
    stream: Vec<u8>,
}

/// Split the four-byte little-endian length prefix the corpus script
/// writes from the stream behind it.
fn split_prefix(name: &str, raw: &[u8], prefixed: bool) -> Seed {
    if !prefixed {
        return Seed {
            name: name.to_owned(),
            decoded_len: None,
            stream: raw.to_vec(),
        };
    }
    assert!(
        raw.len() > 4,
        "the seed {name} is {} bytes, too short to hold its length prefix and a stream",
        raw.len()
    );
    let len = u32::from_le_bytes(raw[..4].try_into().expect("4 bytes")) as usize;
    Seed {
        name: name.to_owned(),
        decoded_len: Some(len),
        stream: raw[4..].to_vec(),
    }
}

fn seeds(corpus: &str, prefixed: bool) -> Vec<Seed> {
    let dir = corpus_root().join(corpus);
    let mut out: Vec<Seed> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading the corpus directory {}: {e}", dir.display()))
        .map(|entry| {
            let path = entry.expect("corpus directory entry").path();
            let raw = std::fs::read(&path)
                .unwrap_or_else(|e| panic!("reading the seed {}: {e}", path.display()));
            let name = path
                .file_name()
                .expect("seed file name")
                .to_string_lossy()
                .into_owned();
            split_prefix(&name, &raw, prefixed)
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

// ---------------------------------------------------------------- mutation

/// xorshift64*. Small, deterministic, and not a dependency.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed ^ 0x9e37_79b9_7f4a_7c15)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next() % bound as u64) as usize
        }
    }
}

/// One mutation of a real stream.
///
/// Unlike a filesystem block, an LZO1X stream has no fixed length and a
/// caller never pads it, so truncation and extension are both things a
/// corrupt image genuinely produces -- a block whose stored compressed
/// length disagrees with what is there. Both are included.
fn mutate(seed: &[u8], rng: &mut Rng) -> Vec<u8> {
    let mut out = seed.to_vec();
    if out.is_empty() {
        return out;
    }

    match rng.below(6) {
        0 => {
            for _ in 0..=rng.below(8) {
                let at = rng.below(out.len());
                out[at] ^= 1u8 << rng.below(8);
            }
        }
        1 => {
            // Token bytes are what decide how the rest is read, so
            // replacing one outright reaches a different decode path
            // than flipping a bit in a length operand.
            let at = rng.below(out.len());
            out[at] = (rng.next() & 0xff) as u8;
        }
        2 => {
            let at = rng.below(out.len());
            let len = 1 + rng.below(16.min(out.len() - at));
            let fill = if rng.next() & 1 == 0 { 0x00 } else { 0xff };
            out[at..at + len].fill(fill);
        }
        3 => {
            // Truncation: a stream that stops mid-token, which is what a
            // short read or a lying block length gives the decoder.
            let keep = rng.below(out.len());
            out.truncate(keep);
        }
        4 => {
            // Extension: trailing bytes past the end-of-stream marker.
            let extra = 1 + rng.below(32);
            for _ in 0..extra {
                out.push((rng.next() & 0xff) as u8);
            }
        }
        _ => {
            // A little-endian 16-bit operand set to an extreme. These
            // carry the long-distance match buckets, which is where a
            // back-reference before the start of the output comes from.
            if out.len() >= 2 {
                let at = rng.below(out.len() - 1);
                let value: u16 = match rng.below(4) {
                    0 => 0,
                    1 => 1,
                    2 => u16::MAX,
                    _ => (rng.next() & 0xffff) as u16,
                };
                out[at..at + 2].copy_from_slice(&value.to_le_bytes());
            }
        }
    }
    out
}

/// The case in flight, readable even if the lock was poisoned by the
/// panic we are trying to describe.
fn describe(current: &Arc<Mutex<String>>) -> String {
    match current.lock() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

// ---------------------------------------------------------------- tests

#[test]
fn every_target_has_a_corpus() {
    for target in targets() {
        assert!(
            !seeds(target.corpus, target.prefixed).is_empty(),
            "the target {} reads fuzz/corpus/{}, which holds no seeds -- a target with an \
             empty corpus runs no cases and would pass in silence. Rebuild it with \
             scripts/make-fuzz-corpus.sh",
            target.name,
            target.corpus,
        );
    }
}

/// The corpus is an oracle, not just fuel.
///
/// Every seed is a stream `lzop` produced, carrying the length `lzop`
/// said it decodes to. Checking that here means this crate is measured
/// against the reference encoder on every pull request, on a machine
/// with no `lzop` installed -- which is the one thing
/// `tests/oracle_lzop.rs` cannot do.
#[test]
fn the_corpus_decodes_to_the_length_lzop_recorded() {
    let found = seeds("decompress", true);
    assert!(
        found.len() >= 12,
        "only {} seeds; the corpus has shrunk",
        found.len()
    );
    for seed in found {
        let recorded = seed
            .decoded_len
            .expect("a decompress seed carries its length");
        let decoded = lzo1x::decompress(&seed.stream, recorded).unwrap_or_else(|e| {
            panic!(
                "{}: a stream lzop produced would not decode: {e}",
                seed.name
            )
        });
        assert_eq!(
            decoded.len(),
            recorded,
            "{}: decoded to {} bytes where lzop recorded {recorded}",
            seed.name,
            decoded.len(),
        );
    }
}

/// A bound of exactly the right size must succeed, and one byte less
/// must fail rather than truncate. The bound is the only thing standing
/// between a crafted stream and an unbounded allocation, so an
/// off-by-one in it is the defect worth catching.
#[test]
fn the_bound_is_exact() {
    for seed in seeds("decompress", true) {
        let recorded = seed
            .decoded_len
            .expect("a decompress seed carries its length");
        assert!(
            lzo1x::decompress(&seed.stream, recorded).is_ok(),
            "{}: the exact decoded length was refused as a bound",
            seed.name,
        );
        if recorded > 0 {
            assert!(
                lzo1x::decompress(&seed.stream, recorded - 1).is_err(),
                "{}: a bound one byte short of the decoded length was accepted, so the \
                 bound does not bound",
                seed.name,
            );
        }
    }
}

#[test]
fn deterministic_mutations_of_real_streams_are_survived() {
    let cases = Arc::new(AtomicUsize::new(0));
    let current = Arc::new(Mutex::new(String::from("(not started)")));
    let (done_tx, done_rx) = mpsc::channel();

    let hook_current = Arc::clone(&current);
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        eprintln!("\nfuzz gate: panicked at {}", describe(&hook_current));
        previous_hook(info);
    }));

    let worker_cases = Arc::clone(&cases);
    let worker_current = Arc::clone(&current);
    let worker = std::thread::spawn(move || {
        for target in targets() {
            for seed in seeds(target.corpus, target.prefixed) {
                for start in 0..SEEDS {
                    let mut rng = Rng::new(start);
                    for case in 0..CASES_PER_SEED {
                        *worker_current.lock().expect("progress lock") = format!(
                            "{} / {} / seed {start} / case {case}",
                            target.name, seed.name
                        );
                        let mutated = mutate(&seed.stream, &mut rng);
                        (target.run)(&mutated);
                        worker_cases.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        let _ = done_tx.send(());
    });

    // A timeout means the worker is still running: a hang. A disconnect
    // means it panicked, and the panic is what is worth reporting.
    match done_rx.recv_timeout(DEADLINE) {
        Ok(()) => {}
        Err(mpsc::RecvTimeoutError::Disconnected) => {}
        Err(mpsc::RecvTimeoutError::Timeout) => {
            // Written to the process's stderr rather than through
            // `eprintln!`, which the harness captures into a buffer it
            // only prints when a test finishes -- and exiting here means
            // it never finishes.
            let _ = writeln!(
                std::io::stderr(),
                "\nhung: no progress for {:?} at {}\n\
                 The decoder did not return. A back-reference loop that never advances \
                 looks exactly like this.",
                DEADLINE,
                describe(&current),
            );
            let _ = std::io::stderr().flush();
            std::process::exit(1);
        }
    }

    let outcome = worker.join();
    let _ = std::panic::take_hook();
    if outcome.is_err() {
        panic!("a decode panicked at {}", describe(&current));
    }

    let total = cases.load(Ordering::Relaxed);
    assert!(
        total >= CASE_FLOOR,
        "only {total} mutated cases ran, below the floor of {CASE_FLOOR} -- the target \
         list or the corpus has collapsed, and a suite that runs nothing passes quickly",
    );
    eprintln!("{total} mutated cases");
}

#[test]
fn the_gate_covers_every_explorer_target() {
    // The two tiers drift apart the moment somebody adds a cargo-fuzz
    // target and forgets that nothing gates it on the stable toolchain.
    let manifest =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fuzz/Cargo.toml"))
            .expect("reading fuzz/Cargo.toml");

    let explorer: Vec<String> = manifest
        .lines()
        .filter_map(|line| line.strip_prefix("name = \""))
        .filter_map(|rest| rest.strip_suffix('"'))
        .map(str::to_owned)
        .skip(1) // the package name is the first `name =` in the file
        .collect();

    assert!(
        !explorer.is_empty(),
        "fuzz/Cargo.toml declares no [[bin]] targets",
    );

    let gated: Vec<&str> = targets().iter().map(|t| t.name).collect();
    for name in &explorer {
        assert!(
            gated.contains(&name.as_str()),
            "fuzz/fuzz_targets/{name}.rs has no counterpart in this suite, so nothing \
             replays its corpus on the stable toolchain and anything it finds would only \
             stay fixed for as long as somebody keeps running the fuzzer by hand",
        );
    }
}
