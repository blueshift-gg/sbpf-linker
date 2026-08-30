// assembly-output: ptx-linker
// ignore-sbpf-arch: v0
// compile-flags: --crate-type bin -C opt-level=3 -C panic=abort

#![no_std]
#![no_main]

// aux-build: loop-panic-handler.rs
extern crate loop_panic_handler;

#[inline(never)]
#[unsafe(no_mangle)]
pub extern "C" fn add_six(a: u64, b: u64, c: u64, d: u64, e: u64, f: u64) -> u64 {
    a + b + c + d + e + f
}

#[unsafe(no_mangle)]
pub fn entrypoint(input: u64) -> u64 {
    add_six(
        input,
        input ^ 2,
        input ^ 3,
        input ^ 4,
        input ^ 5,
        input ^ 6,
    )
}

// CHECK: label add_six
// CHECK-NOT: r11
// CHECK: ldxdw r{{[0-9]+}}, [r10-0xff8]
// CHECK-NOT: r11
// CHECK: exit

// CHECK: label entrypoint
// CHECK-NOT: r11
// CHECK: stxdw [r10+0x8], r{{[0-9]+}}
// CHECK-NOT: r11
// CHECK: call add_six
// CHECK-NOT: r11
// CHECK: exit
