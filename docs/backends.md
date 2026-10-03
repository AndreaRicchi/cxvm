# Backends: where toolchains come from

Findings about upstream release assets, checked against the real release pages (PLAN.md §3,
principle 6). Update this file when upstream changes.

## Clang: official LLVM releases (M3)

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
`--gcc-install-dir=/usr/lib/gcc/x86_64-linux-gnu/15`. M6 pairs clang with a chosen GCC.

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
