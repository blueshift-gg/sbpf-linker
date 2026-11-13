// assembly-output: ptx-linker
// revisions: extend trunc
// compile-flags: --crate-type bin -C opt-level=3 -C panic=abort

// Converting between f32 and f64 on BPF requires compiler-builtins because
// the target has no native floating-point instructions. The linker must
// export these builtins by default so they remain available when LLVM lowers
// the casts to libcalls. Volatile inputs prevent constant folding.

#![no_std]
#![no_main]

// aux-build: loop-panic-handler.rs
extern crate loop_panic_handler;

#[unsafe(no_mangle)]
pub fn entrypoint(input: *const u8) -> u64 {
    #[cfg(extend)]
    {
        let value = unsafe { core::ptr::read_volatile(input.cast::<f32>()) };
        return (value as f64).to_bits();
    }

    #[cfg(trunc)]
    {
        let value = unsafe { core::ptr::read_volatile(input.cast::<f64>()) };
        return (value as f32).to_bits() as u64;
    }
}

// CHECK-LABEL: label entrypoint
// extend: call __extendsfdf2
// trunc: call __truncdfsf2
// CHECK: exit
