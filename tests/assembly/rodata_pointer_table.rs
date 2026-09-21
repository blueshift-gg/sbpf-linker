// assembly-output: ptx-linker
// compile-flags: --crate-type bin -C opt-level=3 -C panic=abort

// A `[&T; N]` pointer table lives in rodata with one relocation per
// element. Each pointer should point at the matching constant.

#![no_std]
#![no_main]

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[repr(transparent)]
pub struct Pubkey([u8; 32]);

#[unsafe(no_mangle)]
pub static AUTH0: Pubkey = Pubkey([1; 32]);
#[unsafe(no_mangle)]
pub static AUTH1: Pubkey = Pubkey([2; 32]);
#[unsafe(no_mangle)]
pub static AUTH2: Pubkey = Pubkey([3; 32]);
#[unsafe(no_mangle)]
pub static AUTH3: Pubkey = Pubkey([4; 32]);

pub static REGISTRY: [&Pubkey; 4] = [&AUTH0, &AUTH1, &AUTH2, &AUTH3];

#[unsafe(no_mangle)]
pub fn entrypoint(x: *mut u8) -> u64 {
    let i = (unsafe { core::ptr::read_volatile(x) }) as usize;
    REGISTRY[i].0[0] as u64
}

// CHECK: rodata[0]: byte 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1
// CHECK: rodata[32]: byte 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2
// CHECK: rodata[64]: byte 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3
// CHECK: rodata[96]: byte 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4
// CHECK: rodata-relocation[128] -> rodata[0] (AUTH0)
// CHECK: rodata-relocation[136] -> rodata[32] (AUTH1)
// CHECK: rodata-relocation[144] -> rodata[64] (AUTH2)
// CHECK: rodata-relocation[152] -> rodata[96] (AUTH3)
