// no-prefer-dynamic
// revisions: no_lto thin fat
// compile-flags: --crate-type bin -C opt-level=3 -C panic=abort -D linker-messages
//[thin] compile-flags: -C lto=thin
//[fat] compile-flags: -C lto=fat

// Without `solana-compiler-builtins`, the linker warns whether or not LTO
// merged the inputs. Denying `linker_messages` turns the warning into an error.

#![no_std]
#![no_main]

// aux-build: loop-panic-handler.rs
extern crate loop_panic_handler;

#[unsafe(no_mangle)]
pub fn entrypoint(_input: *mut u8) -> u64 {
    0
}

// error-pattern: `solana-compiler-builtins` was not linked
