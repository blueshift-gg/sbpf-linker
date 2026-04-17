// assembly-output: ptx-linker
// revisions: inline_small helper_large
// compile-flags: --crate-type bin -C opt-level=3 -C panic=abort -C relocation-model=static -C link-arg=--disable-memory-builtins --cfg feature="mem_unaligned"

// Example assembly fixture.
//
// compiletest_rs only recognizes a fixed set of assembly-output backends.
// We use the "ptx-linker" compatibility path here, while the harness still
// invokes sbpf-linker as the real linker.
//
// How to use the harness:
// - Put the fixture under `tests/assembly`.
// - Keep the compiletest directives above the crate attributes.
// - Add ordered check lines for the normalized AST dump from `tests/tests.rs`.
// - Run `cargo test --test tests compile_test -- --nocapture`.
//
// These two revisions exercise the thresholded memcmp lowering through
// `slice.ne`:
// - `inline_small` compares 32 bytes and keeps the inline `ldxdw` sequence.
// - `helper_large` compares 128 bytes and lowers to the syscall helper path.

#![no_std]
#![no_main]

// aux-build: loop-panic-handler.rs
extern crate loop_panic_handler;

#[cfg(target_arch = "bpf")]
#[inline(always)]
pub fn sol_memcmp_(a: *const u8, b: *const u8, n: usize) -> i32 {
    let mut result = 0i32;
    let sol_memcmp_: unsafe extern "C" fn(
        a: *const u8,
        b: *const u8,
        n: usize,
        result: *mut i32,
    ) -> i32 = unsafe { core::mem::transmute(0x5FDCDE31usize) };
    unsafe {
        sol_memcmp_(a, b, n, &mut result as *mut i32);
    }
    result
}

#[cfg(target_arch = "bpf")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    if n > 64 {
        return sol_memcmp_(a, b, n);
    }

    let mut i = 0usize;
    while i + 8 <= n {
        let wa = unsafe { core::ptr::read_unaligned(a.add(i) as *const u64) };
        let wb = unsafe { core::ptr::read_unaligned(b.add(i) as *const u64) };

        if wa != wb {
            return 1;
        }

        i += 8;
    }

    while i < n {
        if unsafe { *a.add(i) } != unsafe { *b.add(i) } {
            return 1;
        }

        i += 1;
    }
    0
}

#[cfg(inline_small)]
static SMALL_LEN: usize = 32;

#[cfg(helper_large)]
static LARGE_LEN: usize = 128;

#[cfg(inline_small)]
static SMALL_EXPECTED: [u8; 32] = [3u8; 32];

#[cfg(helper_large)]
static LARGE_EXPECTED: [u8; 128] = [3u8; 128];

#[unsafe(no_mangle)]
pub fn entrypoint(input: *mut u8) -> u64 {
    #[cfg(inline_small)]
    {
        let pubkey =
            unsafe { core::slice::from_raw_parts(input.add(16), SMALL_LEN) };
        let expected = unsafe {
            core::slice::from_raw_parts(SMALL_EXPECTED.as_ptr(), SMALL_LEN)
        };
        if pubkey.ne(expected) {
            return 1;
        }
        return 0;
    }

    #[cfg(helper_large)]
    {
        let pubkey =
            unsafe { core::slice::from_raw_parts(input.add(16), LARGE_LEN) };
        let expected = unsafe {
            core::slice::from_raw_parts(LARGE_EXPECTED.as_ptr(), LARGE_LEN)
        };
        if pubkey.ne(expected) {
            return 1;
        }
        return 0;
    }

    0
}

// CHECK,inline_small: rodata-count: 0
// CHECK,inline_small: label entrypoint
// CHECK,inline_small: ldxdw r3, [r1+16]
// CHECK,inline_small: ldxdw r3, [r1+24]
// CHECK,inline_small: ldxdw r3, [r1+32]
// CHECK,inline_small: ldxdw r1, [r1+40]

// CHECK,helper_large: rodata-count: 1
// CHECK,helper_large: rodata-label[0]: data_0000
// CHECK,helper_large: rodata[0]: byte 3, 3, 3, 3, 3, 3, 3, 3
// CHECK,helper_large: label entrypoint
// CHECK,helper_large: add64 r1, 16
// CHECK,helper_large: lddw r2, data_0000
// CHECK,helper_large: mov64 r3, 128
// CHECK,helper_large: call sol_memcmp_
