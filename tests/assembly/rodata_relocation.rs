// assembly-output: ptx-linker
// revisions: mixed anon_only named_only
// compile-flags: --crate-type bin -C opt-level=3

// Anonymous `.rodata` from compiler-generated lookup tables must be preserved,
// when it is packed next to named statics and when it is the only rodata
// section in the program. Named statics that cover the whole section must not
// spuriously synthesize anonymous gaps. In every case, lddw relocations must
// point at the correct packed rodata offset.

#![no_std]
#![no_main]

// aux-build: loop-panic-handler.rs
extern crate loop_panic_handler;

#[cfg(any(mixed, named_only))]
pub struct Hash32([u8; 32]);

#[cfg(mixed)]
static EXPECTED: Hash32 = Hash32([3u8; 32]);

#[cfg(named_only)]
static EXPECTED: Hash32 = Hash32([3u8; 32]);

#[unsafe(no_mangle)]
pub fn entrypoint(input: *mut u8) -> u64 {
    #[cfg(anon_only)]
    {
        let value = unsafe { core::ptr::read_unaligned(input as *const u64) };
        return value.trailing_zeros() as u64;
    }

    #[cfg(mixed)]
    {
        let value = unsafe { core::ptr::read_unaligned(input as *const u64) };
        let expected =
            unsafe { core::ptr::read_volatile(EXPECTED.0.as_ptr() as *const u8) };
        return value.trailing_zeros() as u64 ^ expected as u64;
    }

    #[cfg(named_only)]
    {
        let value = unsafe { core::ptr::read_unaligned(input as *const u8) };
        let expected =
            unsafe { core::ptr::read_volatile(EXPECTED.0.as_ptr() as *const u8) };
        return (value ^ expected) as u64;
    }

    let value = unsafe { core::ptr::read_unaligned(input as *const u64) };
    value.trailing_zeros() as u64
}

// CHECK,mixed: rodata[0]: byte 0, 1, 2, 7, 3, 13, 8, 19
// CHECK,mixed: rodata[64]: byte 3, 3, 3, 3, 3, 3, 3, 3
// CHECK,mixed: lddw r{{[0-9]+}}, rodata[64]
// CHECK,mixed: lddw r{{[0-9]+}}, rodata[0]

// CHECK,anon_only: rodata[0]: byte 0, 1, 2, 7, 3, 13, 8, 19
// CHECK,anon_only-NOT: rodata[64]:
// CHECK,anon_only: lddw r{{[0-9]+}}, rodata[0]

// CHECK,named_only: rodata-count: 1
// CHECK,named_only: rodata[0]: byte 3, 3, 3, 3, 3, 3, 3, 3
// CHECK,named_only-NOT: rodata[32]:
// CHECK,named_only: lddw r{{[0-9]+}}, rodata[0]
