#![no_std]

#[unsafe(no_mangle)]
pub extern "C" fn __muldf3(lhs: f64, _rhs: f64) -> f64 {
    core::hint::black_box(b"compiler-builtins fixture linked");
    lhs
}
