// assembly-output: ptx-linker
// compile-flags: --crate-type bin -C opt-level=3 -C panic=abort -C debuginfo=2

// `KEYS` contains two pointers to `RECORD.key`. `key` is located 8 bytes into
// `RECORD` struct, so each relocation should exactly point to the `key` at offset 8.

#![no_std]
#![no_main]

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[repr(C)]
pub struct Record {
    prefix: u64,
    key: [u8; 32],
}

#[unsafe(no_mangle)]
pub static RECORD: Record = Record { prefix: 7, key: [2; 32] };
pub static KEYS: [&[u8; 32]; 2] = [&RECORD.key, &RECORD.key];

#[unsafe(no_mangle)]
pub fn entrypoint(input: *mut u8) -> u64 {
    let index = (unsafe { core::ptr::read_volatile(input) } as usize);
    let keys = core::hint::black_box(&KEYS);
    keys[index & 1][index] as u64
}

// CHECK: rodata[0]: byte 7, 0, 0, 0, 0, 0, 0, 0, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2
// CHECK: rodata-relocation[40] -> rodata[8] (.rodata.__at__0x8)
// CHECK: rodata-relocation[48] -> rodata[8] (.rodata.__at__0x8)
