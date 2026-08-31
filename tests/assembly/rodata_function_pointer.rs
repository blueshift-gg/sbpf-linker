// assembly-output: ptx-linker
// compile-flags: --crate-type bin -C opt-level=3 -C panic=abort

// A `Desc` value with a function pointer field lives in rodata with a relocation to text.
// The pointer should resolve to `always_true`, and the key should remain unchanged.

#![no_std]
#![no_main]

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

pub struct Desc {
    pub validate: fn(&[u8]) -> bool,
    pub key: [u8; 32],
}

#[unsafe(no_mangle)]
#[inline(never)]
fn always_true(b: &[u8]) -> bool {
    !b.is_empty()
}

pub static DESC: Desc = Desc {
    validate: always_true,
    key: [
        0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xB1, 0xB2, 0xB3,
        0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xC1, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6,
        0xC7, 0xC8, 0xD1, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8,
    ],
};

#[unsafe(no_mangle)]
pub fn entrypoint(input: *mut u8) -> u64 {
    let probe = unsafe { core::slice::from_raw_parts(input, 1) };
    let d: &Desc = core::hint::black_box(&DESC);
    if (d.validate)(probe) { d.key[0] as u64 } else { 0 }
}

// CHECK: rodata[0]: byte {{.*}}161, 162, 163, 164, 165, 166, 167, 168, 177, 178, 179, 180, 181, 182, 183, 184, 193, 194, 195, 196, 197, 198, 199, 200, 209, 210, 211, 212, 213, 214, 215, 216
// CHECK: rodata-relocation[0] -> text[{{.*}}] (always_true)
// CHECK: label always_true
