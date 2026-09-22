// compile-flags: --crate-type bin -C opt-level=3 -C panic=abort

// Six args means the last one spills, and the linker rewrites that load to
// r10-0xff8. 4064 bytes of locals lands on those same bytes.
// The size is picky: 4056 doesn't overlap, 4072 trips LLVM's stack limit first.

#![no_std]
#![no_main]

// aux-build: loop-panic-handler.rs
extern crate loop_panic_handler;

#[inline(never)]
#[unsafe(no_mangle)]
pub extern "C" fn six_args_with_deep_locals(
    a: u64,
    b: u64,
    c: u64,
    d: u64,
    e: u64,
    f: u64,
) -> u64 {
    let mut buf = [0u64; 508];
    buf[0] = a;
    buf[507] = f;
    buf[506] = e;
    core::hint::black_box(&mut buf);
    buf[0] + buf[507] + b + c + d
}

#[unsafe(no_mangle)]
pub fn entrypoint(input: u64) -> u64 {
    six_args_with_deep_locals(
        input,
        input ^ 2,
        input ^ 3,
        input ^ 4,
        input ^ 5,
        input ^ 6,
    )
}

// CHECK: error: linking with
// CHECK: local stack variable overlaps incoming spilled-argument region in `six_args_with_deep_locals`
// CHECK-SAME: local [{{-[0-9]+}}, {{-[0-9]+}}) overlaps argument [{{-[0-9]+}}, {{-[0-9]+}})
// CHECK-NOT: local stack variable overlaps
