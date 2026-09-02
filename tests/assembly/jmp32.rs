// assembly-output: ptx-linker
// ignore-sbpf-arch: v0
// compile-flags: --crate-type bin -C opt-level=3 -C panic=abort

// This fixture documents the harness-level sBPF arch filter. It should run
// only in the v3 pass and be skipped while the harness is running v0. It also
// covers v3-only jump32 decoding.

#![no_std]
#![no_main]

// aux-build: loop-panic-handler.rs
extern crate loop_panic_handler;

#[unsafe(no_mangle)]
pub fn entrypoint(input: u32) -> u32 {
    let expected = 7u32;
    let yes = 1u32;
    let no = 0u32;
    if input == expected { yes } else { no }
}

// CHECK: rodata-count: 0
// CHECK: label entrypoint
// CHECK: jeq32
// CHECK: exit
