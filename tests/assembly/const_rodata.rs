// assembly-output: ptx-linker
// revisions: borrow_const_direct borrow_const_match borrow_static_direct borrow_static_match by_value_const_match mixed_match
// compile-flags: --crate-type bin -C opt-level=3

// Each revision compiles one standalone function so the packed rodata layout
// and the AST for const/static borrows are locked down independently.

#![no_std]
#![no_main]

// aux-build: loop-panic-handler.rs
extern crate loop_panic_handler;

#[cfg(any(
    borrow_const_direct,
    borrow_const_match,
    by_value_const_match,
))]
const CONST_A: [u8; 8] = [0x11, 1, 2, 3, 4, 5, 6, 7];

#[cfg(any(
    borrow_const_match,
    by_value_const_match,
    mixed_match,
))]
const CONST_B: [u8; 8] = [0x22, 1, 2, 3, 4, 5, 6, 7];

#[cfg(any(
    borrow_static_direct,
    borrow_static_match,
    mixed_match,
))]
static STATIC_A: [u8; 8] = [0x33, 1, 2, 3, 4, 5, 6, 7];

#[cfg(borrow_static_match)]
static STATIC_B: [u8; 8] = [0x44, 1, 2, 3, 4, 5, 6, 7];

#[cfg(borrow_const_direct)]
#[unsafe(no_mangle)]
pub extern "C" fn borrow_const_direct() -> u64 {
    let ptr = CONST_A.as_ptr();
    unsafe { core::ptr::read_volatile(ptr) as u64 }
}

#[cfg(borrow_const_match)]
#[unsafe(no_mangle)]
pub extern "C" fn borrow_const_match(flag: u64) -> u64 {
    let bytes = if flag == 0 { &CONST_A } else { &CONST_B };
    unsafe { core::ptr::read_volatile(bytes.as_ptr()) as u64 }
}

#[cfg(borrow_static_direct)]
#[unsafe(no_mangle)]
pub extern "C" fn borrow_static_direct() -> u64 {
    let ptr = STATIC_A.as_ptr();
    unsafe { core::ptr::read_volatile(ptr) as u64 }
}

#[cfg(borrow_static_match)]
#[unsafe(no_mangle)]
pub extern "C" fn borrow_static_match(flag: u64) -> u64 {
    let bytes = if flag == 0 { &STATIC_A } else { &STATIC_B };
    unsafe { core::ptr::read_volatile(bytes.as_ptr()) as u64 }
}

#[cfg(by_value_const_match)]
#[unsafe(no_mangle)]
pub extern "C" fn by_value_const_match(flag: u64) -> u64 {
    let bytes = if flag == 0 { CONST_A } else { CONST_B };
    unsafe { core::ptr::read_volatile(bytes.as_ptr()) as u64 }
}

#[cfg(mixed_match)]
#[unsafe(no_mangle)]
pub extern "C" fn mixed_match(flag: u64) -> u64 {
    let bytes = if flag == 0 { &STATIC_A } else { &CONST_B };
    unsafe { core::ptr::read_volatile(bytes.as_ptr()) as u64 }
}

// CHECK,borrow_const_direct: rodata-count: 1
// CHECK,borrow_const_direct: rodata[0]: byte 17, 1, 2, 3, 4, 5, 6, 7
// CHECK,borrow_const_direct: label borrow_const_direct
// CHECK,borrow_const_direct: lddw r1, rodata[0]
// CHECK,borrow_const_direct: ldxb r0, [r1+0]
// CHECK,borrow_const_direct: exit

// CHECK,borrow_const_match: rodata-count: 2
// CHECK,borrow_const_match: rodata[0]: byte 17, 1, 2, 3, 4, 5, 6, 7
// CHECK,borrow_const_match: rodata[8]: byte 34, 1, 2, 3, 4, 5, 6, 7
// CHECK,borrow_const_match: label borrow_const_match
// CHECK,borrow_const_match: lddw r2, rodata[0]
// CHECK,borrow_const_match: jeq r1, 0, +2
// CHECK,borrow_const_match: lddw r2, rodata[8]
// CHECK,borrow_const_match: ldxb r0, [r2+0]
// CHECK,borrow_const_match: exit

// CHECK,borrow_static_direct: rodata-count: 1
// CHECK,borrow_static_direct: rodata[0]: byte 51, 1, 2, 3, 4, 5, 6, 7
// CHECK,borrow_static_direct: label borrow_static_direct
// CHECK,borrow_static_direct: lddw r1, rodata[0]
// CHECK,borrow_static_direct: ldxb r0, [r1+0]
// CHECK,borrow_static_direct: exit

// CHECK,borrow_static_match: rodata-count: 2
// CHECK,borrow_static_match: rodata[0]: byte 51, 1, 2, 3, 4, 5, 6, 7
// CHECK,borrow_static_match: rodata[8]: byte 68, 1, 2, 3, 4, 5, 6, 7
// CHECK,borrow_static_match: label borrow_static_match
// CHECK,borrow_static_match: lddw r2, rodata[0]
// CHECK,borrow_static_match: jeq r1, 0, +2
// CHECK,borrow_static_match: lddw r2, rodata[8]
// CHECK,borrow_static_match: ldxb r0, [r2+0]
// CHECK,borrow_static_match: exit

// CHECK,by_value_const_match: rodata-count: 0
// CHECK,by_value_const_match: label by_value_const_match
// CHECK,by_value_const_match: mov64 r2, 17
// CHECK,by_value_const_match: jeq r1, 0, +1
// CHECK,by_value_const_match: mov64 r2, 34
// CHECK,by_value_const_match-NOT: lddw
// CHECK,by_value_const_match: stxb [r10-1], r2
// CHECK,by_value_const_match: ldxb r0, [r10-1]
// CHECK,by_value_const_match: exit

// CHECK,mixed_match: rodata-count: 2
// CHECK,mixed_match: rodata[0]: byte 51, 1, 2, 3, 4, 5, 6, 7
// CHECK,mixed_match: rodata[8]: byte 34, 1, 2, 3, 4, 5, 6, 7
// CHECK,mixed_match: label mixed_match
// CHECK,mixed_match: lddw r2, rodata[0]
// CHECK,mixed_match: jeq r1, 0, +2
// CHECK,mixed_match: lddw r2, rodata[8]
// CHECK,mixed_match: ldxb r0, [r2+0]
// CHECK,mixed_match: exit
