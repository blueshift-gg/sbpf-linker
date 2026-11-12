// assembly-output: ptx-linker
// ignore-sbpf-arch: v0
// compile-flags: --crate-type bin -C opt-level=3 -C panic=abort

// LLVM lowers 64-bit atomic load-acquire and store-release operations to 0xdb opcode with
// imm values 0x100/0x110, which sbpf remaps to plain ldxdw/stxdw, preserving registers
// and offsets. Without the remap, linking fails with "invalid opcode: 0xdb".

#![no_std]
#![no_main]
#![allow(internal_features)]
#![feature(core_intrinsics)]

// aux-build: loop-panic-handler.rs
extern crate loop_panic_handler;

use core::intrinsics::{AtomicOrdering, atomic_load, atomic_store};

#[unsafe(no_mangle)]
pub fn entrypoint(input: *mut u8) -> u64 {
    let a = input.cast::<u64>();
    let b = unsafe { input.add(8).cast::<u64>() };
    let received = unsafe {
        atomic_load::<_, { AtomicOrdering::Acquire }, false>(a)
    };
    unsafe {
        atomic_store::<_, { AtomicOrdering::Release }, false>(
            b,
            received.wrapping_add(1),
        );
    }
    received
}

// CHECK: rodata-count: 0
// CHECK: label entrypoint
// CHECK: ldxdw r{{[0-9]+}}, [r1+0x0]
// CHECK: add64 r{{[0-9]+}}, 0x1
// CHECK: stxdw [r1+0x8], r{{[0-9]+}}
// CHECK: exit
