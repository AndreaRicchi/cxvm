# cxvm — per-project C/C++ compiler versions for x86_64 Linux

> `cxvm` is a working name. This file is both the spec and the plan: read it fully before starting,
> and keep the **Status** section and **Decisions log** up to date as work progresses.

## Status

- [x] M0 — Skeleton & CI
- [x] M1 — Config, IDs & resolution
- [x] M2 — Project activation (first useful version)
- [x] M3 — Clang prebuilt backend
- [x] M4 — GCC prebuilt backend + runtime libs
- [x] M5 — GCC from source
- [x] M6 — Clang ↔ libstdc++ pairing & IDE integration
- [x] M7 — Doctor, safety & polish
- [x] M8 — Release

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
- 2026-09-30: `use` pins in the nearest existing `.cxvmrc` walking up, else in the current directory; `.cxvm/` and the `.gitignore` entry go next to it.
- 2026-09-30: `.cxvmrc` format is frozen as `compiler = "<exact id>"`; `use` rewrites the whole file (comments are lost). M6 brings format-preserving edits for `[clang]`/`[integrations]`.
- 2026-09-30: Link metadata (id, prefix, cc, cxx, first `--version` line) lives in the store's `links.toml`, never in the linked prefix.
- 2026-09-30: Linked ids always carry a suffix (`-local` by default); `--as` must be exact, suffixed, and match the detected kind and version. A prefix with both `bin/gcc` and `bin/clang` needs `--as`. Prefixes are stored absolute but not canonicalised.
- 2026-09-30: GCC's version comes from `-dumpfullversion` (fallback `-dumpversion`); clang's from `clang version X.Y.Z` in `--version`, since old releases answer `-dumpversion` with 4.2.1.
- 2026-09-30: rpath in M2 is GCC-only, from `g++ -print-file-name=libstdc++.so.6`; clang gets none until M6. For a distro GCC this yields a default library dir (harmless, redundant); M4 may skip default dirs.
- 2026-09-30: A `system` pin has no `.cxvm/current`; `toolchain.cmake`/`env` point at the `cc`/`c++` found on PATH when generated, and PATH is left alone.
- 2026-09-30: `.gitignore` gets `.cxvm/` unless `git check-ignore` (or the file itself) already ignores it; always on in M2, M6's `[integrations] gitignore` may make it opt-out.
- 2026-09-30: `exec` replaces the process and looks the command up on the new PATH; `env` prints nothing when the toolchain is missing. Neither sets `LD_LIBRARY_PATH`.
- 2026-09-30: `install` without an id regenerates `.cxvm/` from the project's `.cxvmrc` (ignoring `CXVM_COMPILER`) and points to `cxvm link` until downloads exist (M3).
- 2026-09-30: Spawning a compiler retries on ETXTBSY for about a second: a file just written is briefly "busy" while another thread's forked child still holds it open, which made parallel tests flaky.
- 2026-09-30: No store lock yet: `links.toml` and `config.toml` are written atomically, but concurrent `link`s can race until M3 adds the lock.
- 2026-10-03: The clang catalog (`catalog/clang.toml`, embedded) holds every LLVM release from 19.1.0 plus the newest patch of each major 11–18; `cargo xtask catalog` regenerates it, keeps known entries and never re-hashes them.
- 2026-10-03: Catalog sha256s come from the GitHub asset digest, the SLSA attestation subject or a `.sha256` file, cross-checked when several exist; seven pre-19 releases were hashed by downloading them (`source = "hashed-at-generation"`).
- 2026-10-03: At run time cxvm trusts only the embedded sha256; provenance is checked when the catalog is generated, not by the binary (no Sigstore dependency).
- 2026-10-03: Asset choice per release: `LLVM-*-Linux-X64`, then the lowest Ubuntu version, then RHEL, then SLES; assets named with another version are skipped (11.0.0 ships `11.0.1-rc2`).
- 2026-10-03: Dependencies: `ureq` (rustls + ring, bundled Mozilla roots: no system certificates, so no corporate CA yet), `sha2`, `tar` (no xattr), `liblzma` (static, no bindgen), `indicatif`. No `anyhow`.
- 2026-10-03: The store lock covers `install` (download included), `remove`, `link` and `unlink`, and taking it deletes whatever `staging/` holds; this replaces the 2026-09-30 "no store lock yet" entry.
- 2026-10-03: Downloads go to `<asset>.part`, resume with a Range request, and are renamed only once size and sha256 match; cached archives are re-verified before reuse and kept until M7's `prune`.
- 2026-10-03: Post-install checks: a library `ldd` cannot find or a version other than the catalog's refuses the install; a C++ hello-world that does not build or run only warns.
- 2026-10-03: The `Backend` trait is `name` + `prepare` (archive → prefix in staging); listing and resolving live in the catalog, and download, checks, metadata and the rename are shared.
- 2026-10-03: `.cxvm-meta.toml` records id, backend, url, sha256, install time (RFC 3339 UTC), cc, cxx and the first line of `--version`, like `links.toml`, rather than the whole output.
- 2026-10-03: `install` resolves partial ids against the catalog (highest known), even when a lower match is installed; an installed id is a no-op; suffixed ids and `system` are refused. Without an id it now downloads a missing pin (updates the 2026-09-30 entry).
- 2026-10-03: `remove` takes an exact id; it refuses linked toolchains and, without `--force`, a global default that nothing else would satisfy. It moves the prefix into `staging/` before deleting it.
- 2026-10-03: `CXVM_CATALOG=<file>` replaces the embedded catalog; the tests serve it from a local HTTP server.
- 2026-10-03: Found while testing: 18.1.8 and 13.0.1 need `libtinfo.so.5`, which Ubuntu 24.04+ lacks (install refused, hint names packages), and clang 17 fails on GCC 16's libstdc++ headers until given `--gcc-install-dir` (M6). See docs/backends.md.
- 2026-10-03: CI runs clippy and tests with `--workspace`, so the xtask is checked too.
- 2026-10-03: GCC prebuilts come from xPack only (17 versions: 8.5.0, 11.2–15.2); no conda-forge, whose GCC is several packages with their own sysroot. GCC 9, 10, 16 and ≤7 wait for M5.
- 2026-10-03: The GCC catalog (`catalog/gcc.toml`, embedded next to `clang.toml`) holds every GCC version from its highest xPack build; the build number shows only in the asset name, the id stays `gcc@13.3.0`. `cargo xtask catalog [clang|gcc]` regenerates one or both.
- 2026-10-03: GCC sha256s come from xPack's `<asset>.sha`, which must equal the GitHub digest when there is one; nothing is hashed at generation.
- 2026-10-03: New dependency `flate2` (pure-Rust backend) for xPack's `.tar.gz`; the decoder is chosen from the archive's name.
- 2026-10-03: `.cxvm-meta.toml` gains optional `libstdcxx_dir` and `gcc_install_dir` (GCC only), detected in staging with `g++ -print-file-name=libstdc++.so.6` and `gcc -print-libgcc-file-name` and recorded as store paths. A GCC whose libstdc++ is not inside its own prefix is refused.
- 2026-10-03: The toolchain file's rpath still comes from probing the driver when `.cxvm/` is generated, so linked and installed GCCs share one code path; the meta fields are the record for M6 and M7.
- 2026-10-03: GCC smoke test: the hello-world is linked with `-Wl,-rpath,<lib dir>` and `ldd` must resolve its `libstdc++.so.6` there. A "recent library feature" proves nothing on a host whose libstdc++ is newer than the toolchain's (this one), so the M4 "done when" was shown in an `ubuntu:24.04` container instead.
- 2026-10-03: Severity as in M3: a hello-world that does not build, run or load the right libstdc++ only warns. That is what happens with xPack builds bundling binutils older than 2.38 (11.2.0, 8.5.0) on hosts whose glibc uses `DT_RELR` (Ubuntu 26.04); `-B/usr/bin` works around it. See docs/backends.md.
- 2026-10-03: The toolchain file also sets `CMAKE_MODULE_LINKER_FLAGS_INIT` (§4 updated), and gets no rpath when libstdc++ is in a default loader directory, as for a linked distro GCC (closes the 2026-09-30 note).
- 2026-10-03: The test sandbox passes an empty `CXVM_CATALOG` by default: with GCC in the embedded catalog, a test pinning `gcc@13.3.0` downloaded the real release.
- 2026-10-03: `list --available` prints a table per kind with one row per major (versions newest first, `*` on installed ones, `download` = archive size of the row's newest version), wrapped at a fixed 80 columns; 28 lines instead of 64. `--json` still lists every release.
- 2026-10-05: GCC source releases are a third embedded catalog (`catalog/gcc-source.toml`): every GNU release from 7.1.0 plus 6.5.0, 5.5.0, 4.9.4 and 4.8.5 (48 tarballs), not literally "any release". Being listed means cxvm can fetch and verify it, not that it builds on every host.
- 2026-10-05: Source tarballs are verified like prebuilt ones, by the embedded sha256. `cargo xtask catalog gcc-source` records a tarball only after `gpgv` accepts its `.sig` against the GNU keyring and its sha512 equals `sha512.sum` on gcc.gnu.org; cxvm never runs GnuPG. Catalog URLs go through `ftpmirror.gnu.org`.
- 2026-10-05: GMP, MPFR, MPC and ISL are fetched by cxvm, not by `contrib/download_prerequisites` (ftp URLs up to GCC 9, no checks up to GCC 6, needs wget). The script is only read for the versions; the 20 tarballs of `gcc.gnu.org/pub/gcc/infrastructure/` are in the catalog. A missing ISL is skipped, CLooG and gettext always.
- 2026-10-05: Without `--from-source` a partial id still resolves among prebuilt archives (`gcc@13` is xPack 13.4.0); a version that only exists as source is built after a `[y/N]` question on a terminal, else refused with the `--from-source` command. With `--from-source` the highest source release wins (`gcc@13` is 13.5.0).
- 2026-10-05: Build options beyond §8: `-j/--jobs` and a repeatable `--configure-arg`; all build options require `--from-source`. `--host-compiler` takes any installed or linked GCC, never Clang.
- 2026-10-05: The build is `configure --prefix=<store path> --enable-languages=c,c++ --disable-multilib --disable-bootstrap`, `make`, `make install-strip DESTDIR=<staging>` (the stripped prefix is 176–282 MB), in an environment of its own (`HOME`, `TMPDIR`, `LC_ALL=C`, `CC`, `CXX`, and `PATH` without cxvm's directories).
- 2026-10-05: Before any download the host is checked: `make`, `m4`, `awk`, and a host compiler that builds and runs a C and a C++ program; the error names the distro packages.
- 2026-10-05: The build log is `~/.cache/cxvm/logs/gcc-<version>-<UTC time>.log`, outside staging so that it survives a failure; errors quote the first `error:` and the last 12 lines.
- 2026-10-05: The build runs in its own process group; SIGINT, SIGTERM or SIGHUP stops the group, deletes staging and exits 130, a second signal ends cxvm at once. Outside the build phase an interrupt still relies on the next lock to clean staging (M3 behaviour).
- 2026-10-05: The `Backend` trait is now `prepare(release, archive, work dir) -> prefix` plus `describe(meta)`, and returns complete errors; `backends::install` takes the backend. `.cxvm-meta.toml` gains optional `host_compiler` and `configure_args`; the source backend is `gnu-source`.
- 2026-10-05: New dependencies: `bzip2` (pure-Rust backend; the prerequisites are `.tar.bz2`), `signal-hook`, and `libc` for one `kill(2)` on the build's process group, the crate's only `unsafe`.
- 2026-10-05: `list --available` shows the releases that can only be built under a title of their own; `--json` gains `from_source`.
- 2026-10-05: A from-source GCC has the layout of an xPack one (`lib64/libstdc++.so.6`, triple `x86_64-pc-linux-gnu`) but no binutils: it runs the `as` and `ld` on `PATH`. Its `cc1` links the host compiler's libstdc++ statically.
- 2026-10-05: Found while testing on Ubuntu 26.04: GCC 16.2.0 builds as is (7m19s on 22 CPUs); GCC 10.5.0 needs `--configure-arg=--disable-libsanitizer` because glibc 2.42 dropped `<termio.h>`, with either host compiler. cxvm does not add the argument itself; the failure names it. See docs/backends.md.
- 2026-10-05: The time estimate is `4 + 70 / jobs` minutes (from the 16.2.0 build), times three for `--bootstrap`, which was not measured.
- 2026-10-05: `--gcc-install-dir` from clang 16 on, `--gcc-toolchain=<prefix>` before: 15.0.6 rejects the former, 16.0.4 accepts it (the releases in between were not tried). A clang before 16 is refused a GCC whose prefix holds other GCCs, since it would take the newest. See docs/backends.md.
- 2026-10-05: The pairing option is a second element of `CMAKE_<LANG>_COMPILER`, not `CMAKE_<LANG>_FLAGS_INIT`, which `-DCMAKE_CXX_FLAGS=…` replaces without a word. `env` and `exec` carry it in `CC`/`CXX` (`CC="clang --gcc-install-dir=…"`); a bare `clang++` on `PATH` gets nothing, as cxvm does not wrap.
- 2026-10-05: New option `cxvm use <clang> --gcc <id|system>` writes the exact id to `[clang] gcc`; a hand-written partial id resolves to the highest installed GCC. `gcc = "system"` or no table means clang's own detection. The table is kept, and ignored, while a GCC or `system` is pinned.
- 2026-10-05: `[clang]` lives in `.cxvmrc` only, not in `config.toml`: the pairing applies to whichever clang is active in the project (also one chosen by `CXVM_COMPILER`) and to no clang outside a project.
- 2026-10-05: `cxvm install` (no id) also installs a missing paired GCC; `current` prints `with <gcc> (…)` and `--json` gains `clang_gcc` (null without a pairing); a pairing that is not installed fails `current`, `env`, `exec`, `use` and `install` with the install command.
- 2026-10-05: `use` edits `.cxvmrc` in place (`toml_edit`): comments and tables survive, and a file that would still be invalid is reported, not replaced (updates the 2026-09-30 "rewrites the whole file" entry). `use` and `install` work out every file before writing the first, so a failure leaves the project as it was.
- 2026-10-05: `[integrations]`: `gitignore` defaults to true (M2 behaviour, now opt-out), `cmake_presets` and `vscode` to false.
- 2026-10-05: Presets are named `cxvm-<kind>-<version>`, build in `build/<kind>-<version>` and each has its own toolchain file, `.cxvm/toolchains/<kind>-<version>.cmake`, because `.cxvm/toolchain.cmake` only describes the current pin. Compiler settings inline as `cacheVariables` were tried and dropped: a one-language project then warns that `CMAKE_C_COMPILER` was not used. Needs presets version 3 (CMake 3.21); an older file is reported, not upgraded.
- 2026-10-05: cxvm only adds or rewrites the preset of the current pin. Presets of removed toolchains are not dropped (the opening plan said they would be): a user preset may inherit from one, and a dangling reference makes CMake reject the whole file. Left to M7's `doctor`.
- 2026-10-05: VS Code: clangd gets `--query-driver=<bin>/*` for a GCC pin only. On a clang driver it makes clangd 21 query the driver without the pairing option and fall back to the host's GCC; clangd follows a clang, paired or not, from `compile_commands.json` alone.
- 2026-10-05: Beyond the two keys of §8, the VS Code integration sets clangd's `--compile-commands-dir` and `cmake.buildDirectory` to `build/<kind>-<version>`, so that the editor uses the per-toolchain build directory; a value cxvm did not write is left alone with a note. `--query-driver` globs of other compilers are kept; cxvm's are recognised by the store path or a known toolchain's bin directory.
- 2026-10-05: New dependencies: `toml_edit` (shares `toml`'s parser crates) and `jsonc-parser` (no dependencies; `.vscode/settings.json` is JSON with comments). Both files are edited through a syntax tree, never re-serialised.
- 2026-10-05: The "done when" was shown with a scripted LSP session against clangd 21.1.8 started with the generated `clangd.arguments`: `std::vector` resolves into the pinned GCC's headers, and into the paired GCC's for clang 15 and 17. VS Code itself was not driven.
- 2026-10-05: For M7: a compiler set in a toolchain file is not in `CMakeCache.txt` (`CMAKE_<LANG>_COMPILER` is then a normal variable); the doctor check has to read `CMakeFiles/<version>/CMake<LANG>Compiler.cmake`, or follow the cache's `CMAKE_TOOLCHAIN_FILE`.
- 2026-10-12: No `cxvm catalog update`: a fetched catalog would be a second trust root (its sha256s decide what runs), the catalog has no format version (`deny_unknown_fields`), and new releases come with cxvm releases; `CXVM_CATALOG=<file>` stays the way to use a newer one. See docs/backends.md.
- 2026-10-12: Labels (`error:`, `warning:`, `note:`, `hint:`, doctor's `ok:`) are coloured through `anstream`/`anstyle`, already in Cargo.lock through clap, so colour follows clap's rules: `NO_COLOR`, `CLICOLOR`, `CLICOLOR_FORCE`, `TERM=dumb`, terminals only. One `output` module prints them.
- 2026-10-12: `-v` is a process-wide flag set once in `cli::run` (logic stays free of process state otherwise); `debug:` lines cover resolution, store/cache/catalog, every process run (`files::run_captured`) with its exit status, downloads, files written and skipped build dirs. No `log` crate.
- 2026-10-12: `prune` deletes all of `downloads/` (`.part` files included) and `logs/` under the store lock, so never an archive an install is fetching; `--dry-run` lists. Install messages point at it (closes the 2026-10-03 "kept until `prune`" note). Sizes below 1 MiB print in KiB.
- 2026-10-12: `doctor` prints sections Store, Active toolchain, Project (when a `.cxvmrc` is found) and Cache; findings are errors, warnings or notes, each with a hint; it exits 1 only with an error (`Error::Diagnosed`). It takes no lock and creates nothing. A toolchain whose files are gone is one store error; where it is selected (global, pin) a note points there.
- 2026-10-12: Doctor store checks: strays (`Store::scan`, which `installed()` now filters), `.cxvm-meta.toml`, drivers, the version probe, `ldd` on both drivers and GCC's `cc1plus` (`-print-prog-name`), links that are gone (error), hidden by a store entry or report another version (warnings), the global default, `staging/` (a note while the lock is held).
- 2026-10-12: Doctor project checks: `.cxvm/current`, `toolchain.cmake` and `env` are compared with what `cxvm install` would write now (a dangling `current` is an error, a stale file a warning), the `[clang] gcc` pairing, and `cxvm-…` presets of toolchains no longer installed or without their toolchain file (closes the 2026-10-05 "left to M7's doctor" entry). Without the pin installed, the Active section reports it and `.cxvm/` is not checked.
- 2026-10-12: Found with CMake 4.2.3: a build dir keeps its first compiler when the toolchain file changes, even when `-DCMAKE_TOOLCHAIN_FILE` is passed again, without a message; the pairing option is recorded as `CMAKE_<LANG>_COMPILER_ARG1`; a relative toolchain file is stored as given (resolved against the build tree, then the source tree). §4's "CMake notices the change" was wrong and is corrected.
- 2026-10-12: Build dirs are searched up to three levels below the project root (hidden dirs, symlinks and nested projects skipped). A build dir configured with a cxvm toolchain file must match what that file names now (error), unless it is named after the toolchain it builds with, as `cxvm use` suggests (`build/gcc-13.3.0`); one without a toolchain file is compared with the pin (warning, as it may be on purpose: the opening plan said error); foreign toolchain files are skipped.
- 2026-10-12: `remove` also refuses, without `--force`, the current directory's pin (`.cxvmrc` or `CXVM_COMPILER`) and the `[clang] gcc` of the nearest `.cxvmrc` while a clang is active, and names the project's build dirs that build with the toolchain (updates the 2026-10-03 entry). Other projects cannot be known.
- 2026-10-12: `Error::NotImplemented` went with the last stub; a unit test renders every `Error` variant through a match without catch-all and requires a `hint:` line. `--json` for `list`/`current` was already done in M1; `doctor` has none.
- 2026-10-12: The name `cxvm` is free: crates.io has no crate of that name or a similar one, and the GitHub repositories called `cxvm` are empty ones with no stars. No rename.
- 2026-10-12: Tags are `v<version>` (§8 said `0.1.0`). Pushing one runs `release.yml`: CI through `workflow_call`, then a **draft** GitHub release built from CI's musl archive, which the maintainer publishes. `ci.yml` now runs on branch pushes only, so that a tag runs CI once. The `v0.1.0` tag is made after the M8 review.
- 2026-10-12: The release archive is `cxvm-x86_64-unknown-linux-musl.tar.gz`, without a version, so that `releases/latest/download/` is a stable URL. It holds a directory of that name with the binary, the licences, README and CHANGELOG, packed reproducibly (sorted, owner 0, commit time, `gzip -n`), and has a `.sha256` in `sha256sum` format.
- 2026-10-12: The release job refuses a tag whose binary reports another version, or whose version has no `## [x.y.z]` section in CHANGELOG.md (Keep a Changelog); that section is the release notes, so its links are absolute.
- 2026-10-12: The archive gets a build provenance attestation (`actions/attest-build-provenance@v4`, `gh attestation verify`). The step is skipped while the repository is private: GitHub attests private repositories only on Enterprise Cloud.
- 2026-10-12: cxvm is published on crates.io, by hand (`cargo publish`), not by the workflow. The package `include`s `src`, `catalog` (embedded with `include_str!`), `tests` (otherwise cargo warns about every test target), README, CHANGELOG and the licences.
- 2026-10-12: Rust 1.89 had never been tried; it builds, and a CI job keeps it so with `cargo +1.89 check --locked` (`+1.89` because `rust-toolchain.toml` selects stable).
- 2026-10-12: `install.sh` is POSIX sh, uses curl or wget, refuses an archive that does not match its `.sha256`, and installs into `$XDG_BIN_HOME` or `~/.local/bin` through a temporary file and a rename. No sudo, no startup files edited; it prints the `PATH` line instead. `CXVM_VERSION`, `CXVM_BIN_DIR` and `CXVM_RELEASES_URL` (for mirrors and tests) mirror its options. It is a release asset, so the script matches the release it installs. Tested with dash and bash against a local server, with busybox's wget, and in `ubuntu:24.04`.
- 2026-10-12: Downloads honour `ALL_PROXY`/`HTTPS_PROXY`/`HTTP_PROXY`/`NO_PROXY` (ureq's default, checked with a dead proxy). A TLS-intercepting proxy still fails (bundled roots, 2026-10-03); an archive put in `~/.cache/cxvm/downloads/` by hand is verified and used instead. Both are in the README FAQ.
- 2026-10-12: `v0.1.0` was pushed while the repository was still private, so its release has no attestation (the step was skipped as designed, and a re-run keeps the original event). It was left so: re-releasing would rebuild an archive whose sha256 is already published. Releases after 0.1.0 are attested; the README says so.

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
~/.cache/cxvm/logs/        logs of builds from source
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

[integrations]       # like FVM's updateVscodeSettings
cmake_presets = true # default: false
vscode = true        # default: false
gitignore = true     # default: true
```

```
.cxvm/                 (gitignored)
  current -> ~/.local/share/cxvm/toolchains/gcc/13.3.0
  toolchain.cmake      generated
  env                  generated, source-able
  toolchains/          with cmake_presets: one toolchain file per preset
```

### Resolution order

1. `CXVM_COMPILER` environment variable
2. nearest `.cxvmrc`, walking up from the current directory
3. global default in `config.toml`
4. `system`

### Generated CMake toolchain file

It must use the **resolved absolute store path**, not the `.cxvm/current` symlink. CMake caches
the compiler path; if it went through the symlink, re-pinning would silently swap compilers under
an existing build dir. With real paths, a build dir keeps the compiler it was configured with:
CMake does not notice the change (checked with 4.2.3 in M7), but each build dir stays
consistent, and `cxvm doctor` reports one that no longer matches its toolchain file. The
symlink is for humans, IDEs and PATH.

Example (GCC):

```cmake
# Generated by cxvm — do not edit. Toolchain: gcc@13.3.0
set(CXVM_TOOLCHAIN "gcc@13.3.0")
set(CMAKE_C_COMPILER   "/home/me/.local/share/cxvm/toolchains/gcc/13.3.0/bin/gcc")
set(CMAKE_CXX_COMPILER "/home/me/.local/share/cxvm/toolchains/gcc/13.3.0/bin/g++")
# Programs must load this GCC's libstdc++ at runtime, not the system one
set(CMAKE_EXE_LINKER_FLAGS_INIT    "-Wl,-rpath,/home/me/.local/share/cxvm/toolchains/gcc/13.3.0/lib64")
set(CMAKE_SHARED_LINKER_FLAGS_INIT "-Wl,-rpath,/home/me/.local/share/cxvm/toolchains/gcc/13.3.0/lib64")
set(CMAKE_MODULE_LINKER_FLAGS_INIT "-Wl,-rpath,/home/me/.local/share/cxvm/toolchains/gcc/13.3.0/lib64")
```

Intended use: `cmake -S . -B build/gcc-13.3.0 -DCMAKE_TOOLCHAIN_FILE=.cxvm/toolchain.cmake`
(one build dir per toolchain).

For a clang paired with a GCC (`[clang] gcc`), the option that selects that GCC is passed with
the compiler, and the rpath is that GCC's:

```cmake
set(CMAKE_C_COMPILER   "…/toolchains/clang/17.0.6/bin/clang"   "--gcc-install-dir=…/toolchains/gcc/13.3.0/lib/gcc/x86_64-pc-linux-gnu/13.3.0")
set(CMAKE_CXX_COMPILER "…/toolchains/clang/17.0.6/bin/clang++" "--gcc-install-dir=…/toolchains/gcc/13.3.0/lib/gcc/x86_64-pc-linux-gnu/13.3.0")
```

`exec` / `env` prepend the toolchain's `bin/` to PATH and set `CC` / `CXX` (followed by the
pairing option for a paired clang). They do **not** set `LD_LIBRARY_PATH`; runtime libraries
are handled with rpath.

## 5. CLI (end state)

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
