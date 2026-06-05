// assembly-output: ptx-linker
// revisions: panic_path bounds_check named_call unwrap_case
// compile-flags: --crate-type bin -C opt-level=3 -C panic=abort

// A panic path lowers into `.text.unlikely.*`. Calls relocated to that section
// must resolve against the unlikely-section labels instead of leaving an empty
// identifier behind. A direct named call still needs to keep its normal
// relocation path intact.

#![no_std]
#![no_main]

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[cfg(named_call)]
#[unsafe(no_mangle)]
#[inline(never)]
pub fn callee(input: *mut u8) -> u64 {
    unsafe { core::ptr::read_volatile(input) as u64 }
}

#[unsafe(no_mangle)]
pub fn entrypoint(input: *mut u8) -> u64 {
    #[cfg(named_call)]
    {
        return callee(input);
    }

    #[cfg(bounds_check)]
    {
        let idx = unsafe { core::ptr::read_volatile(input) } as usize;
        return [7u8][idx] as u64;
    }

    #[cfg(unwrap_case)]
    {
        let v = unsafe { core::ptr::read_volatile(input) };
        let maybe = if v == 0 { Some(1u64) } else { None };
        return maybe.unwrap();
    }

    if unsafe { core::ptr::read_volatile(input) } != 0 {
        panic!();
    }
    0
}

// CHECK,panic_path: rodata-count: 0
// CHECK,panic_path: label {{.*panic_fmt}}
// CHECK,panic_path: label entrypoint
// CHECK,panic_path: jne r1, 0x0, +0x2
// CHECK,panic_path: call {{.*panic_fmt}}

// CHECK,bounds_check: rodata-count: 0
// CHECK,bounds_check: label {{.*panic_bounds_check}}
// CHECK,bounds_check: label {{.*panic_fmt}}
// CHECK,bounds_check: label entrypoint
// CHECK,bounds_check: jne r1, 0x0, +0x2
// CHECK,bounds_check: mov64 r0, 0x7
// CHECK,bounds_check: call {{.*panic_bounds_check}}
// CHECK,bounds_check: call {{.*panic_fmt}}

// CHECK,named_call: rodata-count: 0
// CHECK,named_call: label callee
// CHECK,named_call: label entrypoint
// CHECK,named_call: call callee

// CHECK,unwrap_case: rodata-count: 0
// CHECK,unwrap_case: label {{.*unwrap_failed}}
// CHECK,unwrap_case: label entrypoint
// CHECK,unwrap_case: call {{.*unwrap_failed}}
// CHECK,unwrap_case-NOT: call -0x1
