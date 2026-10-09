// assembly-output: ptx-linker
// compile-flags: --crate-type bin -C opt-level=3 -C panic=abort -C link-arg=--disable-math-builtins

// The program uses `__adddf3` and `__multi3` compiler builtins. Disabling the
// default math builtin exports should make the linker fail with an error.

#![no_std]
#![no_main]

// aux-build: loop-panic-handler.rs
extern crate loop_panic_handler;

#[unsafe(no_mangle)]
pub fn entrypoint(_x: *mut u8) -> u64 {
    let mut var_a: f64 = 3.5;
    let mut var_b: f64 = 2.0;
    let mut var_c: u64 = 0x1111_2222_3333_4444;
    let mut var_d: u64 = 0xAAAA_BBBB_CCCC_DDDD;

    core::hint::black_box(&mut var_a);
    core::hint::black_box(&mut var_b);
    core::hint::black_box(var_a + var_b);

    core::hint::black_box(&mut var_c);
    core::hint::black_box(&mut var_d);
    core::hint::black_box(var_c.checked_mul(var_d));

    0
}

// error-pattern: Error: Symbol __adddf3 is missing. Export it with the `--export=__adddf3` flag.
// error-pattern: Symbol __multi3 is missing. Export it with the `--export=__multi3` flag.
