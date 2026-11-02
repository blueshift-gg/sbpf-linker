// assembly-output: ptx-linker
// compile-flags: --crate-type bin -C opt-level=3 -C panic=abort -C relocation-model=static

// A private, constant table of pointers that is only ever indexed is a
// candidate for LLVM's relative lookup table conversion, which replaces the
// pointers with 32-bit offsets from the table. BPF objects cannot express the
// relocations those offsets need, so the table must stay absolute: one 64-bit
// relocation per entry.
//
// `relocation-model=static` keeps rustc from converting the table itself; the
// linker must then not convert it either, even though it builds its own
// position-independent target machine.

#![no_std]
#![no_main]

// aux-build: loop-panic-handler.rs
extern crate loop_panic_handler;

static FIRST: u64 = 11;
static SECOND: u64 = 22;
static THIRD: u64 = 33;
static FOURTH: u64 = 44;

static TABLE: [&u64; 4] = [&FIRST, &SECOND, &THIRD, &FOURTH];

#[unsafe(no_mangle)]
pub fn entrypoint(input: *const u8) -> u64 {
    let index = unsafe { core::ptr::read_volatile(input) } as usize;
    *TABLE[index & 3]
}

// CHECK: rodata-relocation[{{[0-9]+}}] -> rodata[{{[0-9]+}}] ({{.*}}FIRST)
// CHECK: rodata-relocation[{{[0-9]+}}] -> rodata[{{[0-9]+}}] ({{.*}}SECOND)
// CHECK: rodata-relocation[{{[0-9]+}}] -> rodata[{{[0-9]+}}] ({{.*}}THIRD)
// CHECK: rodata-relocation[{{[0-9]+}}] -> rodata[{{[0-9]+}}] ({{.*}}FOURTH)
// CHECK: label entrypoint
// CHECK: lddw
// CHECK: exit
