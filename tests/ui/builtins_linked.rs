// no-prefer-dynamic
// revisions: no_lto thin fat
// compile-flags: --crate-type bin -C opt-level=3 -C panic=abort
//[thin] compile-flags: -C lto=thin
//[fat] compile-flags: -C lto=fat
// normalize-stderr-test: "(?s)warning: linker stderr: unable to open LLVM shared lib [^\n]*: dlopen failed\n  \|.*emitted\n\n" -> ""

// With LTO, rustc passes no rlibs, so the linker has to recognize
// `solana-compiler-builtins` from the thin-LTO object name or the symbols in
// the merged fat-LTO object. No `.stderr` file: the warning must not appear.
//
// The `rust-llvm` proxy tries every `libLLVM*` file in the toolchain, and
// warns when it reaches the linker script before the real library. That
// depends on directory order, so the rule above drops the warning when it is
// the only linker message.

#![no_std]
#![no_main]

// aux-build: loop-panic-handler.rs
extern crate loop_panic_handler;

// aux-build: solana-compiler-builtins.rs
extern crate solana_compiler_builtins;

#[unsafe(no_mangle)]
pub fn entrypoint(_input: *mut u8) -> u64 {
    0
}
