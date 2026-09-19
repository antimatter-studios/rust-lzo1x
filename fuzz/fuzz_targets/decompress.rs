#![no_main]
//! The decoder, over arbitrary bytes at the bounds a real caller uses.
//!
//! `max_out` is the only thing between a crafted stream and an
//! unbounded allocation, and it is also a length the decoder compares
//! against as it goes -- so the interesting inputs are the ones that
//! sit either side of it, not just the ones that blow past it. EROFS
//! and SquashFS both decompress into a block-sized buffer, which is
//! where 4096 and 65536 come from.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    for bound in [0usize, 4096, 65_536, 262_144, 1 << 22] {
        let _ = lzo1x::decompress(data, bound);
    }
});
