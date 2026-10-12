# cxvm

[![CI](https://github.com/AndreaRicchi/cxvm/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/AndreaRicchi/cxvm/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/cxvm)](https://crates.io/crates/cxvm)
![MSRV](https://img.shields.io/crates/msrv/cxvm)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

Per-project C/C++ compiler versions for x86_64 Linux. What [FVM](https://fvm.app) is to
Flutter, `cxvm` is to native GCC and Clang: a shared store of installed toolchains in your home
directory, a committed `.cxvmrc` that pins an exact version per project, and a gitignored
`.cxvm/` folder with a `current` symlink and generated CMake / editor integration files. No
root, no wrapping of compiler calls: generated files point at real paths, so builds keep working
even when `cxvm` is not on `PATH`.

```sh
cxvm install gcc@13      # GCC 13.4.0, into ~/.local/share/cxvm
cd my-project
cxvm use gcc@13          # pins gcc@13.4.0 in .cxvmrc, generates .cxvm/
cmake -S . -B build/gcc-13.4.0 -DCMAKE_TOOLCHAIN_FILE=.cxvm/toolchain.cmake
```

**Status: 0.1.0, the first release.** It installs official LLVM builds of Clang, xPack builds
of GCC, and builds GCC releases from source. See [CHANGELOG.md](CHANGELOG.md) for what it
does and its known limitations.

## Why

Different projects need different native compilers: a host GCC with the same major version as
an older cross toolchain, a GCC that a customer's code still builds with, a newer Clang to try
something out. A distribution ships one or a few versions, installing others by hand is
tedious, and switching the global `gcc` with `update-alternatives` changes it for every
project at once. cxvm keeps any number of versions side by side and lets each project say
which one it uses.

## Install

With Rust 1.89 or newer and a C compiler (two dependencies build C code):

```sh
cargo install cxvm --locked
```

This puts `cxvm` in `~/.cargo/bin`, which rustup adds to `PATH`. Run the same command again to
update it.

Without Rust, download the static binary from the
[releases page](https://github.com/AndreaRicchi/cxvm/releases):

```sh
sha256sum -c cxvm-x86_64-unknown-linux-musl.tar.gz.sha256
gh attestation verify cxvm-x86_64-unknown-linux-musl.tar.gz --repo AndreaRicchi/cxvm   # optional, after 0.1.0
tar -xzf cxvm-x86_64-unknown-linux-musl.tar.gz
cp cxvm-x86_64-unknown-linux-musl/cxvm ~/.local/bin/
```

Shell completions: `cxvm completions bash|zsh|fish`, see
[docs/integrations.md](docs/integrations.md#shell-completions).

## Quickstart

```sh
cxvm list --available                # what can be installed
cxvm install gcc@13                  # the newest 13.x; prints the exact version
cd my-project
cxvm use gcc@13                      # writes .cxvmrc and .cxvm/, adds .cxvm/ to .gitignore
cxvm current                         # gcc@13.4.0 (set by /…/my-project/.cxvmrc)

cmake -S . -B build/gcc-13.4.0 -DCMAKE_TOOLCHAIN_FILE=.cxvm/toolchain.cmake
cmake --build build/gcc-13.4.0
```

Commit `.cxvmrc`. Whoever clones the project runs `cxvm install` with no arguments to get the
pinned toolchain and their own `.cxvm/`.

Outside CMake:

```sh
cxvm exec -- make                    # PATH, CC and CXX set for one command
eval "$(cxvm env)"                   # the same for the current shell (fish: cxvm env | source)
```

Clang, using the libstdc++ of a GCC that cxvm knows instead of whichever GCC the host has:

```sh
cxvm install clang@19
cxvm use clang@19 --gcc gcc@13
```

A compiler you already have, and a default for directories without a `.cxvmrc`:

```sh
cxvm link /opt/gcc-11                # registered as gcc@11.4.0-local, for example
cxvm global gcc@11.4.0-local
```

`cxvm doctor` checks the store, the project's generated files and its CMake build
directories. CMake presets, VS Code / clangd settings and direnv are described in
[docs/integrations.md](docs/integrations.md).

## How it works

- **Store.** Toolchains live in `$CXVM_HOME`, else `$XDG_DATA_HOME/cxvm`, else
  `~/.local/share/cxvm`, one directory per version (`toolchains/gcc/13.4.0`). Downloads are
  cached in `~/.cache/cxvm` until `cxvm prune`.
- **Pin.** `.cxvmrc` always holds an exact version, `compiler = "gcc@13.4.0"`, even when you
  typed `gcc@13`.
- **Resolution.** The active toolchain comes from `CXVM_COMPILER`, else the nearest `.cxvmrc`
  walking up from the current directory, else the global default, else `system` (the `cc` on
  `PATH`). `cxvm current` says which one applied.
- **Point, don't wrap.** `.cxvm/toolchain.cmake` and `.cxvm/env` name the compilers by their
  real paths in the store, and `.cxvm/current` links there for people and editors. Nothing
  sits between the build and the compiler.
- **Runtime libraries.** A program built with a GCC newer than the host's must load that GCC's
  `libstdc++.so.6`. The toolchain file adds an rpath to it; cxvm never sets
  `LD_LIBRARY_PATH`.
- **Verified, atomic installs.** Every archive is checked against a sha256 built into cxvm,
  unpacked into a staging directory and moved into the store only once it is complete and has
  passed a smoke test. Where the archives come from, and how their checksums were obtained, is
  in [docs/backends.md](docs/backends.md).

## Commands

| Command | Purpose |
|---|---|
| `cxvm install [<id>] [--from-source]` | Install a toolchain. With no id: install what `.cxvmrc` needs and regenerate `.cxvm/`. With `--from-source`: build GCC, optionally with `--bootstrap`, `--host-compiler <id>`, `-j <n>`, `--configure-arg <arg>` |
| `cxvm remove <id> [--force]` | Remove from the store (refuse if it's the global default, or the current directory's pin or `[clang] gcc`, unless `--force`) |
| `cxvm prune [--dry-run]` | Delete the download cache and the logs of builds from source |
| `cxvm list [--available] [--json]` | Installed toolchains (marking active/global), or installable versions |
| `cxvm use <id> [--gcc <id>]` | Pin in the current project: write `.cxvmrc`, (re)generate `.cxvm/` and the opted-in integrations. With `--gcc`: pair the clang with that GCC's libstdc++ |
| `cxvm global <id>` | Set the default outside projects |
| `cxvm current [--json]` | Show the active toolchain and why |
| `cxvm exec -- <cmd…>` | Run a command with the active toolchain's environment |
| `cxvm env [--shell bash\|zsh\|fish]` | Print exports for `eval` / direnv |
| `cxvm link <prefix> [--as <id>]` / `cxvm unlink <id>` | Register an existing install (e.g. `/opt/gcc-11`) |
| `cxvm doctor` | Diagnose problems |
| `cxvm completions <shell>` | Shell completions |

Ids are `gcc@<version>` and `clang@<version>`, with a full (`13.4.0`) or partial (`13`)
version, or `system`. `-v` makes any command explain what it does.

## Compared with other tools

- **`update-alternatives`** switches `/usr/bin/gcc` between versions your distribution
  packages. That needs root and changes the compiler for every user and project at once. The
  packages get your distribution's security updates; cxvm's toolchains do not.
- **[xPack](https://xpack-dev-tools.github.io) with `xpm`** installs xPack's GCC and Clang
  builds per project, from a `package.json`, and needs Node.js. cxvm uses the same GCC builds,
  takes Clang from LLVM's own releases, builds GCC versions that xPack does not have, and
  generates CMake and editor files. It is a single binary.
- **[pixi](https://pixi.sh)** manages a whole conda-forge environment per project: compilers,
  libraries and tools, locked, on Linux, macOS and Windows. Its compilers come with their own
  sysroot and you work inside the environment. cxvm manages only compilers, builds against the
  host's C library, and leaves the rest of your system alone. Use pixi when you also need the
  libraries.
- **[Nix](https://nixos.org)** (shells, flakes, devenv) pins every tool and library of a
  project exactly, at the cost of the Nix store in `/nix`, which takes root to set up, and its
  own way of building. cxvm is the smaller step: compilers only, in your home directory, used
  by the build you already have.

## FAQ

**I ran `cxvm use` and my build still uses the old compiler.**
CMake keeps the compiler a build directory was first configured with, even when the toolchain
file changes or is passed again. Use one build directory per toolchain, as `cxvm use` suggests
(`build/gcc-13.4.0`). `cxvm doctor` lists build directories that no longer match.

**A program fails with `` version `GLIBCXX_3.4.32' not found ``.**
It was built with a GCC newer than the host's libstdc++ and without an rpath. The CMake
toolchain file adds one; elsewhere, link with `-Wl,-rpath,<store>/toolchains/gcc/<version>/lib64`.
`cxvm exec` and `cxvm env` deliberately do not set `LD_LIBRARY_PATH`.

**An older Clang fails inside the standard library headers.**
Clang uses the newest GCC installation on the host, and an old Clang may not compile a much
newer libstdc++. Pair it with an older GCC: `cxvm use clang@17 --gcc gcc@13`.

**Installing Clang 13.0.1 or 18.1.8 fails: `libtinfo.so.5` not found.**
Those LLVM builds need ncurses 5, which Ubuntu 24.04 and later no longer ship. Install
`libtinfo5` (older Debian/Ubuntu) or `ncurses-compat-libs` (Fedora/RHEL), or use another
version: Clang 19 and later do not need it.

**The xPack GCC 8.5.0 or 11.2.0 hello-world fails, mentioning `.relr.dyn`.**
These builds bundle a linker older than binutils 2.38, which cannot read the C library of
recent distributions. Pass `-B/usr/bin` to use the host's linker, or install a newer release of
the same major.

**Building GCC 10 from source fails in libsanitizer.**
Releases older than the host's C library often do. Run the same command with
`--configure-arg=--disable-libsanitizer`; the compiler then works, without `-fsanitize=`.

**A compiler release I need is not listed.**
The list of releases and their checksums is built into cxvm, so a new compiler release arrives
with a new cxvm release. Until then, `cxvm link` a toolchain you unpacked yourself, or point
`CXVM_CATALOG` at a catalog file generated with `cargo xtask catalog`.

**Downloads fail behind a proxy.**
cxvm honours `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY` and `NO_PROXY`. A proxy that intercepts
TLS with its own certificate fails, because cxvm trusts only the Mozilla root certificates
built into it. Download the archive another way and put it in `~/.cache/cxvm/downloads/` under
its file name; `cxvm install` checks its sha256 and uses it.

**Does cxvm need root, or change my shell configuration?**
No. It writes only to its store, its cache, the current project and temporary directories.

**What breaks if I uninstall cxvm?**
Nothing that is already configured: generated files name the compilers in the store. To remove
everything, remove the `cxvm` binary (`cargo uninstall cxvm`), and delete `~/.local/share/cxvm`
(or `$CXVM_HOME`), `~/.cache/cxvm` and the `.cxvm/` directories of your projects.

## Documentation

- [docs/integrations.md](docs/integrations.md): `.cxvmrc`, CMake and presets, Clang and
  libstdc++, `cxvm doctor`, the shell and direnv, VS Code and clangd.
- [docs/backends.md](docs/backends.md): where toolchains come from, how they are verified, and
  what was found on real hosts.
- [CHANGELOG.md](CHANGELOG.md).
- [CONTRIBUTING.md](CONTRIBUTING.md): how to build, test and change cxvm.
- [PLAN.md at v0.1.0](https://github.com/AndreaRicchi/cxvm/blob/v0.1.0/PLAN.md): the plan and
  decisions log that 0.1.0 was built from.

## Building from source

```sh
cargo build --release
cargo test --workspace                    # never touches the network or your store
cargo test --test network -- --ignored    # real downloads and a GCC build: slow
cargo xtask catalog                       # regenerate catalog/ from upstream releases
```

Requires Rust 1.89 or newer. CI builds the static `x86_64-unknown-linux-musl` binary that is
released.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
this work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without
any additional terms or conditions.
