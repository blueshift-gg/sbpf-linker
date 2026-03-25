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

### Upstream Gallery: Early Feature Gate

Integrates the latest LLVM commits from the Blueshift LLVM fork to experiment with upcoming changes during the upstreaming process. `cargo install-with-gallery` detects the LLVM major from `rustup run nightly rustc -vV` and clones the matching gallery branch, currently [`upstream-gallery-21`](https://github.com/blueshift-gg/llvm-project/tree/upstream-gallery-21) or [`upstream-gallery-22`](https://github.com/blueshift-gg/llvm-project/tree/upstream-gallery-22), before building sbpf-linker with static LLVM linking.

```sh
cargo install-with-gallery 
```

### Generate a Program

```sh
cargo generate --git https://github.com/blueshift-gg/solana-upstream-bpf-template
```

```sh
cargo +nightly build-bpf
```
