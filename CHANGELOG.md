# Changelog

All notable changes to cxvm are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Removed

- `install.sh`. Install with `cargo install cxvm --locked`, or download the static binary from
  the releases page. The 0.1.0 release keeps the script as an asset, but
  `releases/latest/download/install.sh` stops working once the next release is out.

## [0.1.0] - 2026-10-12

The first release.

### Added

- Toolchain ids: `gcc@13.3.0`, `clang@19.1.7`, partial versions (`gcc@13`), `system` (the `cc`
  on `PATH`) and suffixed ids for linked toolchains (`gcc@11.4.0-local`).
- A store of toolchains in `$CXVM_HOME`, else `$XDG_DATA_HOME/cxvm` or `~/.local/share/cxvm`,
  and a download cache in `~/.cache/cxvm`.
- `cxvm use <id>` pins the exact version in `.cxvmrc` and generates `.cxvm/`: a `current`
  symlink, a CMake toolchain file with real store paths and the rpath of the toolchain's
  libstdc++, and a source-able `env`. `.cxvm/` is added to `.gitignore`.
- `cxvm global <id>` sets the default outside projects. The active toolchain comes from
  `CXVM_COMPILER`, the nearest `.cxvmrc`, the global default or `system`, in that order, and
  `cxvm current [--json]` says which and why.
- `cxvm install clang@<version>`: official LLVM releases, every release from 19.1.0 and the
  newest patch release of each major from 11 to 18.
- `cxvm install gcc@<version>`: xPack builds of GCC 8.5.0 and 11.2.0 to 15.2.0.
- `cxvm install gcc@<version> --from-source`: builds a GNU release (every release from 7.1.0,
  and 6.5.0, 5.5.0, 4.9.4, 4.8.5), with `--bootstrap`, `--host-compiler`, `-j` and
  `--configure-arg`. A version without a prebuilt archive is offered as a build.
- Every download is checked against a sha256 embedded in cxvm, resumes after an interruption,
  and is unpacked into a staging directory that only enters the store complete. A toolchain is
  smoke-tested before it is installed.
- `cxvm install` without an id installs what `.cxvmrc` needs and regenerates `.cxvm/`.
- `cxvm link <prefix>` and `cxvm unlink <id>` register a toolchain installed elsewhere.
- `cxvm exec -- <cmd>` and `cxvm env [--shell bash|zsh|fish]` set `PATH`, `CC` and `CXX`.
- `cxvm use <clang> --gcc <id>` pairs clang with the libstdc++ of a chosen GCC
  (`--gcc-install-dir`, or `--gcc-toolchain` before clang 16).
- Opt-in integrations in `.cxvmrc`: a `CMakeUserPresets.json` preset per toolchain, and
  clangd and CMake Tools settings in `.vscode/settings.json`. Both files are edited in place.
- `cxvm list [--available] [--json]`, `cxvm remove <id> [--force]` (refuses the global default
  and the current directory's toolchains), `cxvm prune [--dry-run]` and `cxvm completions`.
- `cxvm doctor` checks the store, the active toolchain, the project's generated files and
  presets, and CMake build directories that build with another compiler than the pin.
- `-v`/`--verbose` debug output; coloured labels that follow `NO_COLOR` and `CLICOLOR`.

### Known limitations

- x86_64 Linux and native GCC and Clang only.
- The catalog of releases is built into cxvm, so new compiler releases come with new cxvm
  releases. `CXVM_CATALOG=<file>` uses another catalog.
- Downloads honour `HTTPS_PROXY` and `NO_PROXY`, but trust only the Mozilla root certificates
  built into cxvm, not the system's: a proxy that intercepts TLS makes them fail.
- Some upstream builds do not run on every host: LLVM 13.0.1 and 18.1.8 need `libtinfo.so.5`,
  xPack GCC 8.5.0 and 11.2.0 bundle a linker too old for recent glibc, and GCC 10 from source
  needs `--configure-arg=--disable-libsanitizer` with glibc 2.42. See
  [docs/backends.md](https://github.com/AndreaRicchi/cxvm/blob/main/docs/backends.md).

[Unreleased]: https://github.com/AndreaRicchi/cxvm/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/AndreaRicchi/cxvm/releases/tag/v0.1.0
