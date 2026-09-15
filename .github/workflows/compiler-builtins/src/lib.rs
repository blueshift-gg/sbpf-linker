#![no_std]

// Cargo builds solana-compiler-builtins from the manifest, but this crate does
// not import it. sbpf-linker must discover the generated rlib itself.

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[unsafe(no_mangle)]
pub extern "C" fn entrypoint(lhs: u64, rhs: u64) -> u64 {
    (f64::from_bits(lhs) * f64::from_bits(rhs)).to_bits()
}
