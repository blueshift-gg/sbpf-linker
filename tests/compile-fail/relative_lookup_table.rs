// assembly-output: ptx-linker
// compile-flags: --crate-type bin -C opt-level=3 -C panic=abort

// Without `-C relocation-model=static`, rustc itself converts the pointer
// table below into a relative lookup table before the linker sees it. The
// 32-bit entries cannot be relocated, so the linker has to say so instead of
// misreading them.

#![no_std]
#![no_main]

// aux-build: loop-panic-handler.rs
extern crate loop_panic_handler;

#[unsafe(no_mangle)]
pub fn entrypoint(input: *const u8) -> u64 {
    const NAMES: [&str; 4] = ["a", "bb", "ccc", "dddd"];
    let index = unsafe { core::ptr::read_volatile(input) } as usize;
    NAMES[index & 3].as_ptr() as u64
}

// error-pattern: 32-bit relocations are not supported in read-only data; compile with `-C relocation-model=static`
