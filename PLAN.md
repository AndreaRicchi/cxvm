# cxvm — per-project C/C++ compiler versions for x86_64 Linux

> `cxvm` is a working name. This file is both the spec and the plan: read it fully before starting,
> and keep the **Status** section and **Decisions log** up to date as work progresses.

## Status

- [x] M0 — Skeleton & CI
- [x] M1 — Config, IDs & resolution
- [ ] M2 — Project activation (first useful version)
- [ ] M3 — Clang prebuilt backend
- [ ] M4 — GCC prebuilt backend + runtime libs
- [ ] M5 — GCC from source
- [ ] M6 — Clang ↔ libstdc++ pairing & IDE integration
- [ ] M7 — Doctor, safety & polish
- [ ] M8 — Release

### Decisions log

_(Append dated one-line entries: what was decided and why.)_

- 2026-09-28: License is dual `MIT OR Apache-2.0` (Rust ecosystem convention).
- 2026-09-28: Edition 2024, MSRV 1.89 (`std::fs::File::lock` needs it); CI runs on `stable`.
- 2026-09-28: Stubbed commands exit 1 with `not implemented yet (planned for M<n>)` on stderr, so scripts cannot mistake a stub for success.
- 2026-09-28: `completions` is implemented in M0 (free with `clap_complete`) rather than stubbed.
- 2026-09-28: Dependencies are added in the milestone that uses them, not all of §6 up front; M0 has only `clap`, `clap_complete` and the test crates.
- 2026-09-28: M0 stubs the §5 table only; `prune` and `catalog update` (M7) are added when M7 starts.
- 2026-09-28: CI installs `musl-tools` from the start so the musl job does not change when C dependencies (rustls/ring, liblzma) arrive in M3.
- 2026-09-28: §7 "small, focused commits" is standing authorisation to commit per logical step during a milestone; nothing is ever pushed by Claude.
- 2026-09-30: A version is exact with three numeric components (`13.3.0`); one or two make it partial. Suffix syntax is `-[a-z0-9][a-z0-9._-]*`; leading zeros are rejected so ids and store directory names round-trip.
- 2026-09-30: The suffix is part of the identity: `gcc@11` never matches `gcc@11.4.0-local`, only `gcc@11-local` does. Linked toolchains are never picked implicitly.
- 2026-09-30: Resolution stops at the first source that is set, even if it names a toolchain that is not installed (pyenv/rbenv behaviour); a `.cxvmrc` that cannot be read is an error, never skipped.
- 2026-09-30: `current` for a pin that is not installed still prints it (text or `--json` with `"installed": false`) and exits 1 with the install or link command.
- 2026-09-30: `--json` for `list` and `current` landed in M1 (the flags already existed); `list --available` stays a stub until M3.
- 2026-09-30: One library error enum (`thiserror`), every variant ending in a `hint:` line; no `anyhow` so far.
- 2026-09-30: `.cxvmrc` and `config.toml` reject unknown keys; `config.toml` uses the same `compiler = "<id>"` key as `.cxvmrc`. M6 adds `[clang]` and `[integrations]` explicitly.
- 2026-09-30: Empty `CXVM_COMPILER`/`CXVM_HOME`/XDG variables count as unset; relative XDG variables are ignored (XDG spec); a relative `CXVM_HOME` is an error.
- 2026-09-30: Logic takes the environment as parameters (`Paths::from_env` closure, `Context`), since `std::env::set_var` is `unsafe` in edition 2024 and tests must not touch process state.
- 2026-09-30: `system` is the first executable `cc` on PATH, skipping relative entries, the store and any `.cxvm` directory; no version probing yet.

---

## 1. Problem & goal

I'm an embedded software engineer working on Linux. Different projects need different native
C/C++ compilers: e.g. a host GCC matching the major version of an older cross-toolchain, or a newer
Clang for experiments. Distros ship one or a few versions, installing others is painful, and
switching the global `gcc` with `update-alternatives` is error-prone.

`cxvm` is to C/C++ compilers what FVM (fvm.app) is to Flutter:

- a shared **store** of installed toolchains in my home directory
- a committed per-project **pin file** (`.cxvmrc`)
- a gitignored per-project **`.cxvm/` folder** with a `current` symlink into the store and
  generated build/IDE integration files
- a **global default** for use outside projects

## 2. Scope

**In scope**
- Host: x86_64 Linux only.
- Toolchains: native GCC (C, C++) and Clang/LLVM.
- Install from prebuilt archives; build GCC from source as a fallback.
- CMake integration, clangd / VS Code integration, shell environment.

**Out of scope for now** (don't build abstractions for these, but don't make them impossible)
- Cross toolchains, other architectures, macOS/Windows, MSVC, other languages.
- Managing libraries or packages.

## 3. Design principles

1. **Never needs root.** Never write outside the store, the current project and temp dirs.
   Never edit shell rc files automatically.
2. **Point, don't wrap.** Like FVM, cxvm does not proxy compiler invocations. Builds must keep
   working even when cxvm isn't on PATH, because generated files reference real paths.
3. **Reproducible pins.** `.cxvmrc` always stores an exact version (`gcc@13.3.0`), even if the
   user typed `gcc@13`.
4. **Atomic, verified installs.** Download → verify → extract into staging → atomic rename into
   the store. A failed or interrupted install leaves nothing half-installed.
5. **Explainable.** `cxvm current` says which toolchain is active *and why*.
6. **Verify, don't assume.** Upstream asset names, URLs and checksum formats change across
   versions. Fetch real release pages to confirm them and record findings in `docs/backends.md`.

## 4. Concepts

### Toolchain IDs

`<kind>@<version>`, with kind ∈ `gcc`, `clang`.

- Partial versions (`gcc@13`, `clang@18`) resolve to the highest matching **installed** version
  for `use`, and the highest **known** version for `install`.
- `system` = whatever `cc` / `c++` resolve to on PATH, with cxvm-managed dirs excluded.
  It can be pinned but not installed or removed.
- Linked toolchains (see M2) get a detected id with a suffix, e.g. `gcc@11.4.0-local`;
  `--as <id>` overrides it.

### Store layout

Root: `$CXVM_HOME`, else `$XDG_DATA_HOME/cxvm`, else `~/.local/share/cxvm`.
Download cache: `$XDG_CACHE_HOME/cxvm`, else `~/.cache/cxvm`.

```
~/.local/share/cxvm/
  toolchains/
    gcc/13.3.0/            bin/ lib64/ lib/gcc/... + .cxvm-meta.toml
    clang/18.1.8/
  links.toml               toolchains registered with `cxvm link`
  config.toml              global default + settings
  staging/                 in-progress installs (same filesystem → atomic rename)
  lock                     lock file for store mutations
~/.cache/cxvm/downloads/   reusable archives
```

`.cxvm-meta.toml` (per toolchain): id, backend, source URL, checksum, install date, detected
paths (C compiler, C++ compiler, lib dir containing `libstdc++.so`, GCC install dir) and the
compiler's `--version` output.

### Project files

```toml
# .cxvmrc  (committed)
compiler = "gcc@13.3.0"

# optional, from M6
[clang]
gcc = "gcc@13.3.0"   # which GCC's libstdc++ clang should use; default: system

[integrations]       # opt-in, like FVM's updateVscodeSettings
cmake_presets = true
vscode = true
gitignore = true
```

```
.cxvm/                 (gitignored)
  current -> ~/.local/share/cxvm/toolchains/gcc/13.3.0
  toolchain.cmake      generated
  env                  generated, source-able
```

### Resolution order

1. `CXVM_COMPILER` environment variable
2. nearest `.cxvmrc`, walking up from the current directory
3. global default in `config.toml`
4. `system`

### Generated CMake toolchain file

It must use the **resolved absolute store path**, not the `.cxvm/current` symlink. CMake caches
the compiler path; if it went through the symlink, re-pinning would silently swap compilers under
an existing build dir. With real paths, CMake notices the change. The symlink is for humans,
IDEs and PATH.

Example (GCC):

```cmake
# Generated by cxvm — do not edit. Toolchain: gcc@13.3.0
set(CXVM_TOOLCHAIN "gcc@13.3.0")
set(CMAKE_C_COMPILER   "/home/me/.local/share/cxvm/toolchains/gcc/13.3.0/bin/gcc")
set(CMAKE_CXX_COMPILER "/home/me/.local/share/cxvm/toolchains/gcc/13.3.0/bin/g++")
# Programs must load this GCC's libstdc++ at runtime, not the system one
set(CMAKE_EXE_LINKER_FLAGS_INIT    "-Wl,-rpath,/home/me/.local/share/cxvm/toolchains/gcc/13.3.0/lib64")
set(CMAKE_SHARED_LINKER_FLAGS_INIT "-Wl,-rpath,/home/me/.local/share/cxvm/toolchains/gcc/13.3.0/lib64")
```

Intended use: `cmake -S . -B build/gcc-13.3.0 -DCMAKE_TOOLCHAIN_FILE=.cxvm/toolchain.cmake`
(one build dir per toolchain).

`exec` / `env` prepend the toolchain's `bin/` to PATH and set `CC` / `CXX`. They do **not** set
`LD_LIBRARY_PATH`; runtime libraries are handled with rpath.

## 5. CLI (end state)

| Command | Purpose |
|---|---|
| `cxvm install [<id>] [--from-source]` | Install a toolchain. With no id: install what `.cxvmrc` needs and regenerate `.cxvm/` |
| `cxvm remove <id> [--force]` | Remove from the store (refuse if it's the global default unless `--force`) |
| `cxvm list [--available] [--json]` | Installed toolchains (marking active/global), or installable versions |
| `cxvm use <id>` | Pin in the current project: write `.cxvmrc`, (re)generate `.cxvm/` |
| `cxvm global <id>` | Set the default outside projects |
| `cxvm current [--json]` | Show the active toolchain and why |
| `cxvm exec -- <cmd…>` | Run a command with the active toolchain's environment |
| `cxvm env [--shell bash\|zsh\|fish]` | Print exports for `eval` / direnv |
| `cxvm link <prefix> [--as <id>]` / `cxvm unlink <id>` | Register an existing install (e.g. `/opt/gcc-11`) |
| `cxvm doctor` | Diagnose problems |
| `cxvm completions <shell>` | Shell completions |

## 6. Tech stack

- Rust stable, edition 2024. Library crate + thin `main.rs`, so logic is testable.
- Suggested crates (check current versions; ask before adding anything heavy):
  `clap` (derive) + `clap_complete`, `serde` + `toml` + `serde_json`, `anyhow` (binary) +
  `thiserror` (library), `ureq` (blocking HTTP, rustls), `sha2`, `tar`, `flate2`,
  xz decompression (`xz2`/`liblzma`, or a pure-Rust option if static linking gets awkward),
  `indicatif`, `tempfile`, file locking via std `File::lock` or `fs4`.
- Tests: `assert_cmd`, `predicates`, `tempfile`; optionally `insta` for generated files.
- Release target: static `x86_64-unknown-linux-musl` binary.

## 7. Working agreements (for Claude Code)

- **One milestone at a time.** At the start, post a short plan for the milestone and flag
  anything in this document you disagree with or find unclear.
- **At the end of each milestone:** `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and
  `cargo test` all clean; update **Status** and the **Decisions log**; summarize what changed and
  how I can try it by hand; then **stop and wait for my review**.
- Small, focused commits with clear messages.
- Default tests never touch the network. Tests that download are `#[ignore]`d and run explicitly.
- Tests use **fake toolchains**: temp dirs with tiny shell scripts named `gcc`, `g++`, `clang`,
  `clang++` that print a version string.
- Tests never touch my real store: always point `CXVM_HOME` (and XDG vars) at temp dirs.
- Every user-facing error says what went wrong and what to do next.
- Ask me before: adding a heavy dependency, changing the `.cxvmrc` format after M2, or anything
  that would need root.

## 8. Milestones

### M0 — Skeleton & CI

Goal: a compiling project with the full CLI surface stubbed.

- `cargo new`, lib + bin layout. Suggested modules: `cli`, `paths`, `config`, `toolchain`
  (ids, versions, metadata), `store`, `project`, `backends`, `integrations`, `doctor`.
- Every command from §5 exists in clap with help text and returns "not implemented yet".
- GitHub Actions: fmt, clippy, test on ubuntu-latest; build a musl release artifact.
- README stub, `.gitignore`, LICENSE (ask me: MIT, Apache-2.0 or dual).

**Done when:** `cxvm --help` lists every command and CI is green.

### M1 — Config, IDs & resolution

Goal: cxvm knows which toolchain *should* be active, without installing anything.

- Toolchain id parsing (kind, exact/partial version, `system`, `-local` suffix) with clear errors.
- Store paths (`CXVM_HOME`, XDG); global `config.toml` read/write.
- `.cxvmrc` discovery (walk up) and parsing; resolution order from §4.
- `list` shows toolchains present in the store (placed there by hand for now) and marks
  global/active.
- `current` prints the active id and the reason (env var / which `.cxvmrc` / global / system).

**Done when:** unit tests cover id parsing, partial-version matching and every resolution branch,
and `cxvm current` is correct from nested subdirectories.

### M2 — Project activation (first useful version)

Goal: switching works end-to-end with compilers I already have.

- `link` / `unlink`: register an existing prefix (e.g. a manually extracted LLVM tarball or
  `/opt/gcc-11`); detect kind and version from `bin/gcc`+`bin/g++` or `bin/clang`+`bin/clang++`;
  record metadata.
- `use <id>`: write the exact version to `.cxvmrc`, create `.cxvm/current`, generate
  `toolchain.cmake` (per §4) and `env`, add `.cxvm/` to `.gitignore` (idempotently).
- `global <id>`, `exec -- <cmd>`, `env --shell …`.
- `install` with no args regenerates `.cxvm/` when the pinned toolchain is already present
  (downloading comes later).

**Done when:** integration tests with fake toolchains pass, and by hand: a CMake hello-world
builds with a linked compiler; after `cxvm use <other>`, a fresh build dir uses the other compiler
(check `CMAKE_CXX_COMPILER_VERSION`).

### M3 — Clang prebuilt backend

Goal: `cxvm install clang@X.Y.Z` installs official LLVM x86_64 Linux releases.

- **Research first:** how LLVM names its Linux x86_64 release assets across versions (this has
  changed), which versions have them, and what integrity data is published (checksums,
  signatures, attestations). Write findings to `docs/backends.md`.
- A `Backend` trait (list available, resolve, fetch, install) designed so GCC backends slot in.
- A catalog of known versions (URL + checksum), generated by an `xtask` from upstream release
  data and embedded in the binary. Where upstream publishes no checksum, hash at generation time.
- Download with progress and cache reuse → verify → extract into `staging/` → atomic rename →
  write `.cxvm-meta.toml`. Lock the store during mutations.
- `remove`, `list --available`.
- Post-install smoke test: `clang++ --version`, then compile and run a one-line program. If it
  fails (e.g. a missing shared library such as an old `libtinfo`), report clearly; check `ldd`
  output for "not found".

**Done when:** installing, using and removing two clang versions works, and an interrupted
download leaves no broken toolchain behind.

### M4 — GCC prebuilt backend + runtime libs

Goal: `cxvm install gcc@X.Y.Z` from prebuilt archives.

- **Research first:** xPack GCC x86_64 Linux archives (GitHub releases of
  `xpack-dev-tools/gcc-xpack`): naming, available versions, checksum files, layout quirks.
  If coverage is too thin, propose conda-forge as a second source, but ask me first.
- Detect and record: the lib dir containing `libstdc++.so` (`lib64` vs `lib`) and the GCC
  install dir (`lib/gcc/<triple>/<version>`; the triple varies between builds).
- The toolchain file's rpath uses the detected lib dir.
- Smoke test: compile and **run** a C++ program using a recent standard-library feature, to prove
  the rpath works (no `GLIBCXX_… not found`).

**Done when:** a GCC newer than the system one builds and runs a program without
`LD_LIBRARY_PATH`.

### M5 — GCC from source

Goal: any GCC release, when no prebuilt exists (`--from-source`, or an automatic fallback after
asking).

- Download the release tarball from GNU mirrors and verify it (**research** what GNU publishes:
  `.sig` files, checksum lists; confirm, don't assume).
- Prerequisites (GMP, MPFR, MPC, ISL): use `contrib/download_prerequisites` or fetch them
  directly; decide and document.
- Out-of-tree build under `staging/`: configure with the final store path as `--prefix`,
  `--enable-languages=c,c++ --disable-multilib`; default to a non-bootstrap build for speed, with
  `--bootstrap` for a full 3-stage build. `make -j$(nproc)`, then `make install DESTDIR=<staging>`
  and atomically move the result into the store.
- Full build log to a file; errors point to the log. Check host prerequisites up front and name
  the distro packages to install.
- `--host-compiler <id>`: build with a cxvm-managed GCC, because old GCC releases often fail to
  compile with a modern host compiler.
- Show a rough time estimate before starting. Ctrl-C leaves no partial install.

**Done when:** a from-source GCC (pick a version xPack doesn't ship) installs, passes the M4
smoke test and works with `use`.

### M6 — Clang ↔ libstdc++ pairing & IDE integration

Goal: clang behaves predictably, and editors see the right compiler.

- **Clang pairing:** by default clang uses the newest GCC installation it finds on the system,
  and an old clang can fail on a much newer libstdc++. Support `[clang] gcc = "gcc@…"` in
  `.cxvmrc`: the generated toolchain file passes `--gcc-install-dir=<detected dir>`, or
  `--gcc-toolchain=<prefix>` for clang versions that lack it (**research** which versions support
  which). Add the matching rpath.
- `CMakeUserPresets.json` (opt-in): one configure preset per pinned toolchain, `binaryDir` =
  `build/<id>`. Merge with an existing file, never overwrite it.
- VS Code (opt-in): merge into `.vscode/settings.json` — clangd `--query-driver` for the
  toolchain's bin dir, and the CMake toolchain setting. Never clobber unrelated keys.
- Docs: direnv snippet (`eval "$(cxvm env)"`); shell completions.

**Done when:** in VS Code with clangd, go-to-definition on a `std::` symbol opens the pinned
toolchain's headers.

### M7 — Doctor, safety & polish

- `doctor` detects: broken `current` symlink, pinned toolchain not installed, missing shared
  libraries (`ldd`), store entries without metadata, leftover staging dirs, and **build dirs whose
  `CMakeCache.txt` compiler differs from the pinned toolchain**.
- `remove` safety checks; `prune` for the download cache.
- `-v/--verbose` logging, `--json` for `list` / `current`, `NO_COLOR` support.
- Error-message pass: every error has a next step.
- Catalog refresh: `cxvm catalog update` fetching a newer, checksummed catalog from the project
  repo — or document why it isn't worth it yet.

### M8 — Release

- README: motivation, quickstart (install → use → cmake), comparison with alternatives
  (update-alternatives, xPack, pixi, Nix), FAQ covering the gotchas in this plan.
- Check the name is free on crates.io and GitHub; rename if needed.
- GitHub Release workflow: static musl binary + sha256; `cargo install` support; install script.
- CHANGELOG, tag 0.1.0.
