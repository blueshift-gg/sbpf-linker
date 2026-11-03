// no-prefer-dynamic
// compile-flags: --crate-type rlib

// Stands in for the `solana-compiler-builtins` crate with one symbol from each
// category the linker looks for when LTO hides the rlib.

#![no_std]

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcpy(dst: *mut u8, _src: *const u8, _n: usize) -> *mut u8 {
    dst
}

#[unsafe(no_mangle)]
pub extern "C" fn __multi3(a: i128, _b: i128) -> i128 {
    a
}

#[unsafe(no_mangle)]
pub extern "C" fn __adddf3(a: f64, _b: f64) -> f64 {
    a
}
