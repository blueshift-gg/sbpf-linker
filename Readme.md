<h1 align="center">
  SBPF Linker
</h1>
<p align="center">
  An upstream BPF linker to relink upstream BPF binaries into an SBPF V0 compatible binary format.
</p>

### Install

```sh
cargo install sbpf-linker
```

### Usage

```
Usage: sbpf-linker [OPTIONS] --output <OUTPUT> <INPUTS>...

Arguments:
  <INPUTS>...                                    Input files. Can be object files or static
                                                 libraries

Options:
      --target <TARGET>                          LLVM target triple. When not provided, the target
                                                 is inferred from the inputs
      --override-cpu-flag <OVERRIDE_CPU_FLAG>    Override the target-cpu attribute to expose the
                                                 desired CPU features to bpf-linker [default: v2]
      --cpu-features <features>                  Enable or disable CPU features. The available
                                                 features are: alu32, dummy, dwarfris. Use +feature
                                                 to enable a feature, or -feature to disable it. For
                                                 example --cpu-features=+allows-misaligned-mem-
                                                 access,+alu32,-dwarfris [default: ""]
  -o, --output <OUTPUT>                          Write output to <output>
      --emit <EMIT>                              Output type. Can be one of `llvm-bc`, `asm`,
                                                 `llvm-ir`, `obj` [default: obj]
      --btf                                      Emit BTF information. Can get DWARF symbols only if
                                                 BTF is enabled and if requested from `rustc` with
                                                 `-C debuginfo=N`
      --allow-bpf-trap                           Permit automatic insertion of __bpf_trap calls.
  -L <LIBS>                                      Add a directory to the library search path
  -O <OPTIMIZE>                                  Optimization level. 0-3, s, or z [default: 2]
      --export-symbols <path>                    Export the symbols specified in the file `path`.
                                                 The symbols must be separated by new lines
      --log-file <path>                          Output logs to the given `path`
      --log-level <level>                        Set the log level. If not specified, no logging is
                                                 used. Can be one of `error`, `warn`, `info`,
                                                 `debug`, `trace`
      --unroll-loops                             Try hard to unroll loops. Useful when targeting
                                                 kernels that don't support loops
      --ignore-inline-never                      Ignore `noinline`/`#[inline(never)]`. Useful when
                                                 targeting kernels that don't support function calls
      --dump-module <path>                       Dump the final IR module to the given `path` before
                                                 generating the code
      --dump-cfg-dir <dir>                       Write CFG .dot dumps to this directory
      --sbpf-optimize <SBPF_OPTIMIZE>            Enable SBPF assembler optimizations [default: true]
                                                 [possible values: true, false]
      --arch <ARCH>                              sBPF target architecture. Can be one of `v0`, `v3`
                                                 [default: v3]
      --llvm-args <args>                         Extra command line arguments to pass to LLVM
      --disable-memory-builtins                  Disable exporting memcpy, memmove, memset, memcmp
                                                 and bcmp. Exporting those is commonly needed when
                                                 LLVM does not manage to expand memory intrinsics to
                                                 a sequence of loads and stores
      --export <symbols>                         Comma separated list of symbols to export. See also
                                                 `--export-symbols`
      --fatal-errors <FATAL_ERRORS>              Whether to treat LLVM errors as fatal
                                                 [default: true] [possible values: true, false]
  -h, --help                                     Print help
  -V, --version                                  Print version
```



### Generate a Program


```sh
cargo generate --git https://github.com/blueshift-gg/solana-upstream-bpf-template
```

```sh
cargo +nightly build-bpf
```
