// assembly-output: ptx-linker
// ignore-sbpf-arch: v0
// revisions: udiv umod sdiv smod
// compile-flags: --crate-type bin -C opt-level=3 -C panic=abort

// 128-bit division and remainder are libcalls into compiler-builtins. LLVM
// only introduces the calls during code generation, after the linker has
// internalized and dropped every symbol nothing referenced, so the builtins
// have to be exported to still be there.

#![no_std]
#![no_main]

// aux-build: loop-panic-handler.rs
extern crate loop_panic_handler;

#[cfg(any(udiv, umod))]
type Wide = u128;
#[cfg(any(sdiv, smod))]
type Wide = i128;

#[unsafe(no_mangle)]
pub fn entrypoint(input: *const Wide) -> u64 {
    let a = unsafe { core::ptr::read_volatile(input) };
    let b = unsafe { core::ptr::read_volatile(input.add(1)) };
    // Rule out the panicking cases so only the libcall is emitted.
    unsafe { core::hint::assert_unchecked(b > 1) };
    #[cfg(any(udiv, sdiv))]
    return (a / b) as u64;
    #[cfg(any(umod, smod))]
    return (a % b) as u64;
}

// CHECK: label entrypoint
// udiv: call __udivti3
// umod: call __umodti3
// sdiv: call __divti3
// smod: call __modti3
// CHECK: exit
