#![no_main]
//! Whatever `compress` makes of an input must decode back to exactly
//! that input.
//!
//! Not panicking is the weaker half of what this codec owes a caller.
//! A compressor that emits a stream its own decoder reads back one byte
//! short is a silent corruption of every file written through it, and
//! no amount of fuzzing the decoder alone would show it.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let packed = lzo1x::compress(data);
    // No cap here, unlike the gate's arm: the explorer has the time,
    // and a length the gate never reaches is exactly what it is for.
    let back = lzo1x::decompress(&packed, data.len())
        .expect("this crate must be able to decode its own output");
    assert_eq!(back, data, "compress then decompress lost the input");
});
