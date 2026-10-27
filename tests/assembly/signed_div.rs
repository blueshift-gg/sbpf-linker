// assembly-output: ptx-linker
// ignore-sbpf-arch: v0
// compile-flags: --crate-type bin -C opt-level=3 -C panic=abort

// Regression test for https://github.com/blueshift-gg/sbpf-linker/issues/152
//
// LLVM targeting BPF v4 (used for SBPF v3) emits div/mod opcodes with off=1
// to signal signed semantics. The signed_div_fixup pass must expand these into
// sign-manipulation + unsigned-divide sequences before the decoder sees them.
// Without the fix, linking fails with: "off: 1 supposed to be zeros".

#![no_std]
#![no_main]

// aux-build: loop-panic-handler.rs
extern crate loop_panic_handler;

/// Signed 64-bit division via volatile inputs — prevents LLVM constant folding.
#[unsafe(no_mangle)]
pub unsafe fn entrypoint(input: *mut u8) -> u64 {
    let a = unsafe { core::ptr::read_volatile(input as *const i64) };
    let b = unsafe { core::ptr::read_volatile(input.add(8) as *const i64) };
    let q = if b != 0 { a / b } else { 0 };
    q as u64
}

// LLVM places the division block after the zero-divisor `exit`, so only the
// expansion itself is checked: sign masks via `arsh64 0x3f`, then unsigned div64.
// CHECK-LABEL: label entrypoint
// CHECK:       arsh64 r{{[0-9]}}, 0x3f
// CHECK:       arsh64 r{{[0-9]}}, 0x3f
// CHECK:       div64 r{{[0-9]}}, r{{[0-9]}}
