# cxvm

Per-project C/C++ compiler versions for x86_64 Linux. What [FVM](https://fvm.app) is to
Flutter, `cxvm` is to native GCC and Clang: a shared store of installed toolchains in your home
directory, a committed `.cxvmrc` that pins an exact version per project, and a gitignored
`.cxvm/` folder with a `current` symlink and generated CMake / editor integration files. No
root, no wrapping of compiler calls: generated files point at real paths, so builds keep working
even when `cxvm` is not on `PATH`.

**Status: pre-alpha.** Pinning per project, linking compilers you already have, and installing
prebuilt releases work: official LLVM builds of Clang (`cxvm install clang@19`) and xPack builds
of GCC (`cxvm install gcc@13`). GCC releases without a prebuilt archive are built from source
(`cxvm install gcc@10 --from-source`). Clang can be paired with the libstdc++ of a chosen GCC
(`cxvm use clang@17 --gcc gcc@13`), and CMake presets and VS Code settings follow the pin when
a project asks for them: see [docs/integrations.md](docs/integrations.md). `cxvm doctor` comes
next. The specification and milestone plan live in [PLAN.md](PLAN.md).

## Building

```sh
cargo build --release
target/release/cxvm --help
```

Requires Rust 1.89 or newer. A static binary for `x86_64-unknown-linux-musl` is built in CI.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
this work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without
any additional terms or conditions.
