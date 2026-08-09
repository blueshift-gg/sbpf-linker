// assembly-output: ptx-linker
// ignore-sbpf-arch: v0
// min-llvm-version: 23.0
// compile-flags: --crate-type bin -C opt-level=3 -C panic=abort -C link-arg=--override-cpu-flag=v3

#![no_std]
#![no_main]

// aux-build: loop-panic-handler.rs
extern crate loop_panic_handler;

#[unsafe(no_mangle)]
pub fn entrypoint(input: u64) -> u64 {
    let target: extern "C" fn(u64, u64, u64, u64, u64, u64) -> u64 =
        unsafe { core::mem::transmute(input) };
    target(input, input ^ 2, input ^ 3, input ^ 4, input ^ 5, input ^ 6)
}

// CHECK: label entrypoint
// CHECK-NOT: r11
// CHECK: stxdw [r10+0x8], r{{[0-9]+}}
// CHECK-NOT: r11
// CHECK: callx
// CHECK-NOT: r11
// CHECK: exit
