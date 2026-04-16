// assembly-output: ptx-linker
// compile-flags: --crate-type bin -C opt-level=3 -C panic=abort

// core::panic::Location structs land in .data.rel.ro when the panic handler
// reads location fields and passes them to an opaque external. lddw
// relocations targeting .data.rel.ro must resolve to the correct rodata
// offsets.

#![no_std]
#![no_main]

unsafe extern "C" {
    fn sol_panic_(
        file: *const u8,
        len: u64,
        line: u64,
        col: u64,
    ) -> !;
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let loc = info.location().unwrap();
    unsafe {
        sol_panic_(
            loc.file().as_ptr(),
            loc.file().len() as u64,
            loc.line() as u64,
            loc.column() as u64,
        )
    }
}

#[unsafe(no_mangle)]
pub fn entrypoint(x: *mut u8) -> u64 {
    let a: Option<u64> =
        if unsafe { core::ptr::read_volatile(x as *const u64) } != 0 {
            Some(1)
        } else {
            None
        };
    let b: Option<u64> =
        if unsafe { core::ptr::read_volatile(x as *const u64) } != 1 {
            Some(2)
        } else {
            None
        };
    a.unwrap() + b.unwrap()
}

// CHECK: rodata-count: 3
// CHECK: rodata[0]: byte 116, 101, 115, 116, 115, 47, 97, 115, 115, 101, 109, 98, 108, 121, 47, 100, 97, 116, 97, 95, 114, 101, 108, 95, 114, 111, 95, 108, 100, 100, 119, 46, 114, 115, 0
// CHECK: rodata[35]: byte 0, 0, 0, 0, 0, 0, 0, 0, 34, 0, 0, 0, 0, 0, 0, 0, 48, 0, 0, 0, 7, 0, 0, 0
// CHECK: rodata[59]: byte 0, 0, 0, 0, 0, 0, 0, 0, 34, 0, 0, 0, 0, 0, 0, 0, 48, 0, 0, 0, 20, 0, 0, 0
// CHECK: lddw r{{[0-9]+}}, rodata[35]
// CHECK: lddw r{{[0-9]+}}, rodata[59]
