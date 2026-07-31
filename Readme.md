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

### LLVM Main: Early Feature Gate

Build and install sbpf-linker against a cached
[`llvm/llvm-project`](https://github.com/llvm/llvm-project) `main` checkout
with static LLVM linking. The install command checks for Git, CMake, and Ninja,
installs missing Homebrew dependencies on macOS, and reuses the cached LLVM
checkout and build when available.

```sh
cargo install-with-llvm-main
```

Update the cached LLVM checkout and rebuild LLVM separately when you want to move to the latest `main`.

```sh
cargo update-llvm-main
```

### Generate a Program

```sh
cargo generate --git https://github.com/blueshift-gg/solana-upstream-bpf-template
```

```sh
cargo +nightly build-bpf
```
