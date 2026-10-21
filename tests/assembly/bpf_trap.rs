// assembly-output: ptx-linker
// ignore-sbpf-arch: v0
// compile-flags: --crate-type bin -C opt-level=3 -C panic=abort

// LLVM lowers `llvm.trap` to a call to `__bpf_trap`, a kernel kfunc that SBPF
// programs cannot resolve. The linker binds it to the `abort` syscall.

#![no_std]
#![no_main]
#![feature(abort_immediate)]

// aux-build: loop-panic-handler.rs
extern crate loop_panic_handler;

#[unsafe(no_mangle)]
pub fn entrypoint(input: *const u64) -> u64 {
    let value = unsafe { core::ptr::read_volatile(input) };
    if value == 0 {
        core::process::abort_immediate();
    }
    value
}

// Static syscalls are called by hash: 0xb6fc1a11 is murmur3_32("abort").
// CHECK: label entrypoint
// CHECK: call 0xb6fc1a11
// CHECK: exit
