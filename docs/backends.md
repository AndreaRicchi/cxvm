# Backends: where toolchains come from

Findings about upstream release assets, checked against the real release pages. Update this
file when upstream changes.

## Clang: official LLVM releases

Source: GitHub releases of [`llvm/llvm-project`](https://github.com/llvm/llvm-project/releases),
tags `llvmorg-<version>`. Data below was read from the GitHub releases API on 2026-10-03.

### Asset names

The x86_64 Linux asset changed name at 19.1.0:

| Releases | Asset |
|---|---|
| 19.1.0 and later | `LLVM-<version>-Linux-X64.tar.xz` |
| 23.1.0 and later | also `LLVM-<version>-Linux-X64.tar.zst` (cxvm uses the `.tar.xz`) |
| 18.x and earlier | `clang+llvm-<version>-x86_64-<platform>.tar.xz` |

The `<platform>` part of the old names is the distro the release manager built on, and varies
from release to release: `linux-gnu-ubuntu-16.04`, `linux-gnu-ubuntu-18.04`,
`linux-gnu-ubuntu-20.04`, `linux-gnu-ubuntu-22.04`, `linux-gnu-rhel-8.4`,
`unknown-linux-gnu-rhel86`, `unknown-linux-gnu-sles15`, `linux-sles12.4`, `linux-sles11.3`, …
Some releases carry several, some none.

Releases with **no** x86_64 Linux asset include 18.1.1–18.1.3, 18.1.5, 18.1.6, 17.0.1, 17.0.3,
16.0.1, 16.0.5, 16.0.6, 15.0.1, 15.0.3 and 14.0.1–14.0.5. `llvmorg-11.0.0` ships an asset named `clang+llvm-11.0.1-rc2-…`, i.e. the wrong version.

When a release has several x86_64 Linux assets, the catalog generator picks, in order:

1. `LLVM-<version>-Linux-X64.tar.xz`;
2. the Ubuntu build with the lowest Ubuntu version (oldest glibc, so it runs on most hosts);
3. a RHEL build;
4. a SLES build.

Assets whose name does not contain the tag's exact version are skipped.

### Archive layout

Every archive holds a single top-level directory named after the asset (without `.tar.xz`),
containing a normal install prefix: `bin/clang`, `bin/clang++` (symlinks to `clang-<major>`),
`lib/`, `include/`, `share/`. cxvm strips that top-level directory.

### Sizes

Compressed: about 0.4 GB for 11.x, 0.5–1.0 GB for 13–18, 1.6 GB for 19.x, 1.9 GB for 20 and
later. Installed, 19.1.7 takes 8.2 GB; downloading and extracting it took about 1m40s here.

### Host requirements

The `LLVM-*-Linux-X64` builds (19.1.0 and later) need only glibc, libstdc++, libgcc and zlib
from the host, with `RUNPATH $ORIGIN/../lib` for their own libraries.

Some older builds also need `libtinfo.so.5`, with the versioned symbol
`NCURSES_TINFO_5.0.19991023`. Recent distros no longer ship it: Ubuntu 24.04 and later have only
`libtinfo6`, whose symbols are versioned `NCURSES6_TINFO_*`, so a `libtinfo.so.5 ->
libtinfo.so.6` symlink does not help. cxvm finds this with `ldd` before the toolchain enters
the store and refuses the install, naming the library.

`readelf -d` on `bin/clang-<major>` of the catalog's older releases (2026-10-03):

| Release | Build | libtinfo |
|---|---|---|
| 18.1.8 | ubuntu-18.04 | `libtinfo.so.5` (install refused on Ubuntu 26.04) |
| 17.0.6, 16.0.4 | ubuntu-22.04 | `libtinfo.so.6` |
| 15.0.6 | ubuntu-18.04 | none found (single pass, not double-checked) |
| 14.0.6 | rhel-8.4 | none found (single pass, not double-checked) |
| 13.0.1 | ubuntu-18.04 | `libtinfo.so.5` |
| 12.0.1 | ubuntu-16.04 | none found (single pass, not double-checked) |
| 11.1.0 | ubuntu-16.04 | none |

Clang builds C++ with the newest GCC installation it finds on the host (headers, libstdc++,
crt files). When the post-install C++ hello-world does not build, cxvm still installs clang
and warns. That happens when there is no GCC (no `g++` or libstdc++ development package), or
when the newest one is too new for an old clang: on Ubuntu 26.04, clang 17.0.6 fails on GCC
16's `<iostream>` (`member access into incomplete type '__normal_iterator'`) but works with
`--gcc-install-dir=/usr/lib/gcc/x86_64-linux-gnu/15`. `[clang] gcc` in `.cxvmrc` pairs clang
with a chosen GCC; see "Pairing clang with a GCC" below.

### Pairing clang with a GCC

Two driver options select the GCC whose headers, libstdc++ and crt files clang uses. Tried on
Ubuntu 26.04 (2026-10-05) with the catalog's clang 15.0.6, 16.0.4 and 17.0.6 and the distro's
clang 21.1.8, against xPack GCC 11.5.0 and 13.3.0 in the store and the distro's GCC 15 and 16
in `/usr`:

| Option | Meaning | 15.0.6 | 16.0.4, 17.0.6, 21.1.8 |
|---|---|---|---|
| `--gcc-install-dir=<prefix>/lib/gcc/<triple>/<version>` | exactly this GCC | `unsupported option` | works |
| `--gcc-toolchain=<prefix>` | the newest GCC below `<prefix>/lib/gcc/<triple>/` | works | works |

The releases between 15.0.6 and 16.0.4 were not tried; cxvm passes `--gcc-install-dir` from
major 16 on and `--gcc-toolchain` before, and the catalog's only clang 16 is 16.0.4.
`--gcc-toolchain=/usr` selects GCC 16 here even when 15 is wanted, so cxvm refuses to pair a
clang before 16 with a GCC whose prefix holds others. A store GCC has a prefix of its own, and
its triple (`x86_64-pc-linux-gnu` for xPack and from-source builds) is one clang knows.

How the option reaches the compiler matters:

- In `toolchain.cmake` it is a second element of `CMAKE_<LANG>_COMPILER`
  (`set(CMAKE_CXX_COMPILER "<clang++>" "--gcc-install-dir=…")`). In `CMAKE_CXX_FLAGS_INIT` it
  works too, until someone configures with `-DCMAKE_CXX_FLAGS=-O1`: the option is then gone and
  the build silently uses the host's newest GCC. Checked with CMake 4.2.3.
- The option appears in `compile_commands.json`, and clangd (21.1.8 here) honours it: with no
  `--query-driver`, go-to-definition on `std::vector` opens the paired GCC's `stl_vector.h`. A
  `--query-driver` glob that matches the clang driver breaks this: clangd asks the driver for
  its include directories without the option and gets the host's GCC 16. cxvm therefore writes
  a `--query-driver` glob for GCC pins only.
- Programs also need the paired GCC's `libstdc++.so.6` at run time, so the toolchain file adds
  the same rpath as for that GCC itself.

### Integrity data

| Data | Available for | Used by cxvm |
|---|---|---|
| GitHub asset `digest` (`sha256:…`, computed by GitHub on upload) | assets uploaded after mid-2025: 20.1.7 and later | catalog generation |
| `<asset>.jsonl`: Sigstore bundle with SLSA v1 provenance; its in-toto subject holds the asset's sha256, built by `.github/workflows/release-tasks.yml` at the tag | 19.1.0 and later | catalog generation |
| `<asset>.sha256` (`sha256sum` format) | 11.1.0, 12.0.0, 13.0.0 only | catalog generation |
| `<asset>.sig` (GPG detached signature) | some releases only | not used |
| nothing | most of 11.x–18.x | hashed once at catalog generation |

cxvm trusts the sha256 in the catalog embedded in its binary and checks every download against
it. Provenance is checked when the catalog is generated (`cargo xtask catalog`): for 20.1.7 and
later the attestation subject must equal the GitHub digest. Verifying Sigstore bundles inside
cxvm would need a heavy dependency for little gain over an embedded checksum.

### Catalog coverage

Every release from 19.1.0 on, plus the newest patch release of each major from 11 to 18 that
has an x86_64 Linux asset: 18.1.8, 17.0.6, 16.0.4, 15.0.6, 14.0.6, 13.0.1, 12.0.1, 11.1.0.
Older patch releases can be added with the xtask's `--hash-missing` allowlist; each one costs a
full download to hash.

## GCC: xPack prebuilt archives

Source: GitHub releases of
[`xpack-dev-tools/gcc-xpack`](https://github.com/xpack-dev-tools/gcc-xpack/releases), tags
`v<gcc version>-<build>` (`v13.3.0-2` is the second xPack build of GCC 13.3.0). Data below was
read from the GitHub releases API on 2026-10-03; archives were unpacked and run on Ubuntu 26.04
(glibc 2.43, binutils 2.46).

### Coverage

26 releases, 17 GCC versions: 8.5.0, 11.2.0–11.5.0, 12.1.0–12.5.0, 13.2.0–13.4.0,
14.1.0–14.3.0 and 15.2.0. There is nothing for GCC 9, 10, 7 and older, and nothing for GCC 16
yet (newest release: 2025-10-14). Those need `cxvm install --from-source` or `cxvm link`.

The catalog holds every GCC version, from the highest xPack build of each. The build number is
visible in the asset name and URL only: the id stays `gcc@13.3.0`.

### Asset names and integrity data

| | |
|---|---|
| Asset | `xpack-gcc-<gcc version>-<build>-linux-x64.tar.gz` (gzip; 115–172 MB) |
| Checksum | `<asset>.sha`, in `sha256sum` format, on every release |
| GitHub asset `digest` | releases uploaded after mid-2025 (12.5.0-1, 13.4.0-1, 14.3.0-1, 15.2.0-1) |

`cargo xtask catalog gcc` takes the sha256 from the `.sha` file and, when GitHub has a digest,
requires both to agree. Nothing has to be hashed at generation time. There are no signatures or
attestations.

### Archive layout

One top-level directory, `xpack-gcc-<gcc version>-<build>/`, which cxvm strips. No hard links.
Checked on 15.2.0-1, 12.1.0-1, 11.3.0-1, 11.2.0-3, 11.2.0-1 and 8.5.0-1:

| | 11.2.0 and later | 8.5.0-1 |
|---|---|---|
| Drivers | `bin/gcc`, `bin/g++` (also `c++`, `cpp`, `gfortran`) | same |
| Target triple (`-dumpmachine`) | `x86_64-pc-linux-gnu` | `x86_64-unknown-linux-gnu` |
| GCC install dir | `lib/gcc/x86_64-pc-linux-gnu/<version>` | `lib/gcc/x86_64-unknown-linux-gnu/8.5.0` |
| C++ runtime | `lib64/libstdc++.so.6`, `lib64/libgcc_s.so.1` | same, but no `lib64/libstdc++.so`: `g++` links `libstdc++.a` statically |
| Installed size | 320–460 MB | 300 MB |

Also in every prefix: a bundled binutils (`bin/ld`, `as`, `ar`, `objdump`, … and the copies GCC
itself runs in `<triple>/bin/`), `gdb`, `README.md` and `distro-info/`. 15.2.0-1 adds `lib32/`
(multilib). Because `cxvm env`/`exec` put `bin/` first on `PATH`, these binutils and `gdb`
shadow the host's while the toolchain is active.

`libexec/` (8.5.0: `bin/`) holds private copies of the libraries the xPack programs themselves
need, including a second `libstdc++.so.6`; they are found through `RPATH $ORIGIN…` and are not
what compiled programs link against.

What the drivers answer, which cxvm relies on:

- `gcc -dumpfullversion` prints `x.y.z` (so does `-dumpversion`).
- `g++ -print-file-name=libstdc++.so.6` prints
  `<prefix>/bin/../lib/gcc/<triple>/<version>/../../../../lib64/libstdc++.so.6`: absolute but
  not normalised. cxvm canonicalises it and records the directory.
- `gcc -print-libgcc-file-name` prints `<prefix>/bin/../lib/gcc/<triple>/<version>/libgcc.a`;
  its directory is the GCC install dir.
- The toolchain is relocatable: all of the above follow the prefix when it is moved.

### Host requirements

- The drivers, `cc1plus` and `ld` of 15.2.0-1 need at most `GLIBC_2.27` and nothing else from
  the host (`ldd` finds every library); those of 11.2.0-1 and 8.5.0-1 need `GLIBC_2.14`.
- There is no sysroot: programs are built against the host's glibc headers and `crt1.o`
  (`libc6-dev`, `glibc-devel`). The builds are multiarch-aware (`-print-multiarch` answers
  `x86_64-linux-gnu`) and search `/usr/include/x86_64-linux-gnu` and
  `/usr/lib/x86_64-linux-gnu`, so Debian/Ubuntu work.
- GCC always uses its **bundled** linker, and an old one cannot read a new host's glibc.
  Ubuntu's glibc is linked with packed relative relocations (`DT_RELR`, section `.relr.dyn`),
  which binutils understands from 2.38:

  | Build | Bundled binutils | Linking on Ubuntu 26.04 |
  |---|---|---|
  | 15.2.0-1 | 2.45 | works |
  | 12.1.0-1, 11.3.0-1 | 2.38 | works |
  | 11.2.0-3, 11.2.0-1 | 2.36.1 | fails: `libmvec.so.1: unknown type [0x13] section '.relr.dyn'` |
  | 8.5.0-1 | 2.34 | fails the same way, C programs included |

  The other builds were not unpacked; by release date, everything after 11.2.0-3 (2022-02)
  should carry binutils 2.38 or newer (not verified). cxvm installs such a toolchain and warns
  that the hello-world did not build. Passing `-B/usr/bin` makes GCC use the host's `as` and
  `ld` instead, which worked for 8.5.0-1 here.

### Runtime libraries

Nothing in an xPack GCC makes the programs it builds find its `libstdc++.so.6`: a plain
`g++ hello.cpp` on this host records only `NEEDED libstdc++.so.6` and loads
`/usr/lib/x86_64-linux-gnu/libstdc++.so.6`. That works while the host's libstdc++ is at least
as new as the toolchain's, and fails with `GLIBCXX_… not found` otherwise. (xPack's own advice
is `-static-libgcc -static-libstdc++`.)

With `-Wl,-rpath,<prefix>/lib64`, as in the generated `toolchain.cmake`, `ldd` shows
`libstdc++.so.6` and `libgcc_s.so.1` resolved from the prefix. The bundled `ld` writes the
path as `DT_RPATH`; Ubuntu's own `ld` (used with `-B/usr/bin`) writes `DT_RUNPATH`. Both cover
the libraries the program links directly.

cxvm's post-install check builds a C++ hello-world with that rpath, runs it, and checks with
`ldd` that `libstdc++.so.6`, when the program needs one, is the toolchain's.

Checked where it matters, on a host older than the toolchain: in an `ubuntu:24.04` container
(libstdc++ 6.0.33 from GCC 14, glibc 2.39, `libc6-dev` installed, no system GCC),
`cxvm install gcc@15` passes that check, and a program needing `GLIBCXX_3.4.34` (a localised
`std::format` of a `std::chrono` value) runs when linked with the flag from `toolchain.cmake`.
Without the flag it stops with ``version `GLIBCXX_3.4.34' not found``. `LD_LIBRARY_PATH` was
unset in both runs.

## GCC: GNU source releases

Source: GNU's release tarballs. Read on 2026-10-05; the builds below ran on Ubuntu 26.04 (glibc
2.43, binutils 2.46, system GCC 15.2.0, 22 CPUs).

### What GNU publishes

| Where | Holds |
|---|---|
| `https://ftp.gnu.org/gnu/gcc/gcc-<version>/` | `gcc-<version>.tar.gz`, a second tarball (`.tar.xz` or `.tar.bz2`), a detached GPG signature `<tarball>.sig` for each, diffs to the previous release. No checksum file |
| `https://ftpmirror.gnu.org/gnu/gcc/…` | the same tree, as a redirect (302) to a nearby mirror, often plain `http://` |
| `https://gcc.gnu.org/pub/gcc/releases/gcc-<version>/` | the same files on the GCC project's own server, plus `sha512.sum` |
| `https://ftp.gnu.org/gnu/gnu-keyring.gpg` | the GNU keyring, with the keys of the GCC release managers |
| `https://gcc.gnu.org/pub/gcc/infrastructure/` | the GMP, MPFR, MPC and ISL tarballs GCC is built with, and `sha512.sum` |

The index lists every release from 2.95 to 16.2.0. The second tarball is a `.tar.xz` for 5.5.0,
6.4.0, 6.5.0 and everything from 7.2.0; before that it is a `.tar.bz2`. cxvm takes the `.tar.xz`
when there is one, else the `.tar.gz`.

### Integrity

Same model as the prebuilt backends: cxvm trusts the sha256 embedded in its catalog and checks
every download against it, so a mirror, even over `http://`, cannot hand it another file.

`cargo xtask catalog gcc-source` records a tarball only after it

1. downloaded it from `ftp.gnu.org`,
2. verified its `.sig` with `gpgv` against the GNU keyring (`source = "gpg-signature"`), and
3. found its sha512 equal to the one in `sha512.sum` on `gcc.gnu.org`.

The library tarballs have no signatures next to them: they are recorded when their sha512
equals the one in `infrastructure/sha512.sum` (`source = "upstream-sha512"`). Entries already
in the catalog are never downloaded again. cxvm does not run GnuPG itself: the user would need
`gpg` and the keyring for a check the catalog already carries.

### Catalog coverage

Every release from 7.1.0, and the last release of each older series back to 4.8: 6.5.0, 5.5.0,
4.9.4 and 4.8.5. Being in the catalog means cxvm can fetch and verify the sources, not that they
build on every host: see "What was tried" below.

`cxvm install gcc@13` keeps meaning the prebuilt xPack archive. A source release is used with
`--from-source`, or, after asking on a terminal, when the version has no prebuilt archive.

### Prerequisites

GCC needs GMP, MPFR and MPC, and optionally ISL (for Graphite loop optimizations). cxvm unpacks
them into the source tree as `gmp/`, `mpfr/`, `mpc/` and `isl/`, where GCC's own build
configures and builds them statically, so nothing has to be installed on the host and nothing
of them is needed at run time.

Which versions: the ones the release's own `contrib/download_prerequisites` names, which cxvm
reads instead of running:

| GCC | Script | Names |
|---|---|---|
| 4.8.5 | `GMP=gmp-4.3.2` variables, fetched as `$GMP.tar.bz2` | gmp 4.3.2, mpfr 2.4.2, mpc 0.8.1; no ISL |
| 4.9.4 | same | the same, isl 0.12.2 (and CLooG 0.18.1, which cxvm skips) |
| 5.5.0, 6.5.0 | same | the same, isl 0.14 (5.5.0), isl 0.15 (6.5.0) |
| 7.1.0 | `gmp='gmp-6.1.0.tar.bz2'` | gmp 6.1.0, mpfr 3.1.4, mpc 1.0.3, isl 0.16.1 |
| 8.5.0, 9.5.0 | same | gmp 6.1.0, mpfr 3.1.4, mpc 1.0.3, isl 0.18 |
| 10.5.0, 11.5.0 | same | gmp 6.1.0, mpfr 3.1.6, mpc 1.0.3, isl 0.18 |
| 12.5.0 to 15.3.0 | same | gmp 6.2.1, mpfr 4.1.0, mpc 1.2.1, isl 0.24 (14 and later also gettext 0.22, which cxvm skips: glibc has `libintl`) |
| 16.2.0 | same | gmp 6.3.0, mpfr 4.2.2, mpc 1.3.1, isl 0.24 |

The script itself is not run because up to GCC 9 it fetches over `ftp://`, up to GCC 6 it
verifies nothing, and it needs `wget` or `curl`. All 20 GMP, MPFR, MPC and ISL tarballs of
`infrastructure/` are in the catalog; they go through the same cache and sha256 check as every
other download. A release naming an ISL the catalog lacks is built without ISL; a missing GMP,
MPFR or MPC stops the install.

### How cxvm builds

Everything happens in one directory of `staging/`, on the store's filesystem:

```
staging/gcc-10.5.0.<pid>/
  src/     the tarball, with gmp/ mpfr/ mpc/ isl/ added
  build/   the out-of-tree build
  dest/    DESTDIR of the install
```

1. `src/configure --prefix=<store>/toolchains/gcc/<version> --enable-languages=c,c++
   --disable-multilib --disable-bootstrap`, plus every `--configure-arg`; `--bootstrap` leaves
   `--disable-bootstrap` out.
2. `make -j<jobs>`, one job per CPU unless `-j` says otherwise.
3. `make install-strip DESTDIR=…/dest`. The prefix is then
   `dest/<store>/toolchains/gcc/<version>`; it gets the same checks as a prebuilt archive (`ldd`, version, runtime dirs, a C++ hello-world
   linked with `-Wl,-rpath`) and moves into the store in one rename. GCC finds its own files
   relative to the driver, so the checks run it from staging.

`install-strip` rather than `install`: the compiler's own programs are built with `-g`, and
stripped the prefix takes 176 MB (10.5.0) or 282 MB (16.2.0).

The build's environment is not the caller's: only `HOME` and `TMPDIR` are kept, `LC_ALL` is `C`,
`CC` and `CXX` are the host compiler's absolute paths, and `PATH` is the caller's without
cxvm's own directories (the store and any `.cxvm`). `CFLAGS`, `LIBRARY_PATH`, `CPATH`,
`MAKEFLAGS` and the like never reach it, and the binutils of an active cxvm toolchain are not
picked up by `configure`.

All output goes to `~/.cache/cxvm/logs/gcc-<version>-<UTC time>.log` (12 MB for 16.2.0), which
is kept whatever happens; staging is not. A failure quotes the first `error:` line and the last
12 lines, and names the log.

The build runs in its own process group. On SIGINT, SIGTERM or SIGHUP cxvm sends that group
SIGTERM (SIGKILL after 5 s), deletes the staging directory and exits with status 130; a second
signal ends cxvm at once. Before the build starts (download, unpacking) a signal ends cxvm as
usual, and the next cxvm command that takes the store lock deletes what was left in staging.

### Host requirements

Checked before anything is downloaded:

- `make`, `m4` (GMP's `configure` needs it) and `awk` on `PATH`;
- a host compiler whose C and C++ drivers build a program that runs: the system's `cc` and
  `c++`, or any GCC cxvm manages with `--host-compiler <id>` (installed or linked; Clang is
  refused).

Not needed, and absent on the machine used here: `bison`, `flex`, `makeinfo`, the gettext
tools and GNU awk (`mawk` 1.3.4 did the job). Release tarballs ship the files those generate.

### The toolchain it produces

Seen on the 10.5.0 and 16.2.0 builds:

- Triple `x86_64-pc-linux-gnu`; `lib64/libstdc++.so.6`; install dir
  `lib/gcc/x86_64-pc-linux-gnu/<version>`: the same layout as an xPack build from 11.2.0 on, so
  rpath and metadata work unchanged.
- No binutils: `gcc -print-prog-name=ld` answers `ld`, i.e. the assembler and linker are the
  ones on `PATH` when the compiler runs, normally the host's. This is the opposite of xPack,
  whose bundled linker can be too old for the host.
- `cc1` and `cc1plus` need only `libc`, `libm` and `libzstd.so.1` from the host. The host
  compiler's libstdc++ is linked in statically (`configure` adds `-static-libstdc++
  -static-libgcc` when the host compiler accepts them), so the result does not depend on the
  compiler it was built with. `libzstd` is there because its development package was
  installed here and `configure` picked it up.
- No sysroot: like xPack, it builds programs against the host's glibc headers.
- `.cxvm-meta.toml` records `backend = "gnu-source"`, the tarball's URL and sha256, the first
  `--version` line of the host compiler and the `configure` arguments.

### What was tried

On the machine described above, 22 jobs, non-bootstrap:

| Release | Host compiler | Extra arguments | Result | Wall time | CPU time |
|---|---|---|---|---|---|
| 16.2.0 | system GCC 15.2.0 | none | installed, checks pass | 7m19s | 74 min |
| 10.5.0 | xPack GCC 13.4.0 (linked) | none | `make` fails in libsanitizer after 4m23s | | |
| 10.5.0 | xPack GCC 13.4.0 (linked) | `--disable-libsanitizer` | installed, checks pass | 4m21s | 41 min |
| 10.5.0 | system GCC 15.2.0 | `--disable-libsanitizer` | installed, checks pass | 4m02s | 37 min |

The 16.2.0 build tree peaked at 6.2 GiB in staging. With the pinned 10.5.0, a CMake project
configured with the generated `toolchain.cmake` reports `CMAKE_CXX_COMPILER_VERSION` 10.5.0, and
its program loads `libstdc++.so.6` and `libgcc_s.so.1` from the store without `LD_LIBRARY_PATH`.

Not tried: `--bootstrap`, any release older than 10.5.0, and a host older than the release.
GCC 10.5.0 did not need an older host compiler here, although GCC 15 compiles C as C23 by
default; older releases may.

The time estimate cxvm prints is `4 + 70 / jobs` minutes, three times that for `--bootstrap`:
the 16.2.0 numbers split into a part that more jobs do not shorten and the rest. It is too
high for old releases (8 minutes announced for 10.5.0, 4 taken).

### libsanitizer and the host's glibc

The sanitizer runtime is the part of GCC that knows the most about glibc's internals, and old
releases stop building when glibc moves on. Here: glibc 2.43 has no `<termio.h>` any more (it
went in 2.42), and GCC 10.5.0 fails with

    sanitizer_platform_limits_posix.cpp:420:31: error: invalid application of 'sizeof' to
    incomplete type '__sanitizer::termio'

`--configure-arg=--disable-libsanitizer` builds a compiler without `-fsanitize=address` and its
siblings; everything else works. cxvm does not add it by itself: when `make` fails in
`libsanitizer` the error says so and names the argument, at the cost of a failed build (four
minutes here).

Which releases still use `struct termio`, read from the sources of the ones named (only
10.5.0 was built): 4.9.4, 6.5.0, the last release of each major from 7 to 12, 13.1.0, 13.3.0,
13.4.0, 14.1.0 to 14.3.0 and 15.1.0. It is gone in 13.5.0, 14.4.0, 15.2.0, 15.3.0, 16.1.0 and
16.2.0. So on a host with glibc 2.42 or later, expect to need the argument for everything
before 13.5.0, for 14.1.0 to 14.3.0 and for 15.1.0.

## Catalog updates

The catalogs are embedded in the binary, so a release of LLVM, xPack or GNU that came out after
a cxvm release is not installable until the next one. A `cxvm catalog update` command,
fetching a newer catalog from the project repository, was considered and not built, for these
reasons:

- **A second trust root.** The catalog's sha256s decide which binaries cxvm unpacks and runs.
  Today they come with the cxvm binary, from a tagged release. A catalog fetched later would
  be trusted on its own, at any time, from wherever it is published. A checksum fetched from
  the same place only shows that the download is complete, not who wrote it; that needs a
  signature, and so a signing key to keep and a verifying dependency.
- **No format version.** The catalog files reject unknown keys (`deny_unknown_fields`), so a
  newer catalog that records anything new would break every older binary that fetched it.
- **Little gain.** New catalogs come with new cxvm releases, which the release workflow makes
  cheap. Until then, `CXVM_CATALOG=<file>` uses any catalog file instead of the embedded one,
  e.g. one regenerated from a checkout with `cargo xtask catalog`, which checks provenance as
  described above.

What would make it worth building: a `format` field in the catalog that old binaries can
check, and catalogs signed by a release key whose public half is embedded in cxvm.
