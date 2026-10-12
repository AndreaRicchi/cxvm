# Build and editor integration

What cxvm writes in a project, and how CMake, the shell and editors pick it up. `cxvm use <id>`
and `cxvm install` (no id) regenerate all of it from `.cxvmrc`.

```toml
# .cxvmrc
compiler = "clang@17.0.6"

[clang]
gcc = "gcc@13.3.0"     # the GCC whose libstdc++ clang uses; default: "system"

[integrations]
cmake_presets = true   # default: false
vscode = true          # default: false
gitignore = true       # default: true
```

`cxvm use` only changes `compiler` (and `[clang] gcc` with `--gcc`); comments and the other
tables stay as written. After editing the file by hand, run `cxvm install`.

## CMake

`.cxvm/toolchain.cmake` describes the pinned toolchain with real store paths:

```sh
cmake -S . -B build/gcc-13.3.0 -DCMAKE_TOOLCHAIN_FILE=.cxvm/toolchain.cmake
```

Use one build directory per toolchain. `cxvm use` prints the command with the right directory.
CMake keeps the compiler a build directory was first configured with: after `cxvm use <other>`,
a build directory configured for the old pin goes on building with the old compiler, without a
word, even when the toolchain file is passed again. `cxvm doctor` reports such directories.

### Presets

With `cmake_presets = true`, `CMakeUserPresets.json` gets one configure preset per toolchain
the project has been pinned to:

```sh
cmake --preset cxvm-gcc-13.3.0      # configures build/gcc-13.3.0
```

- A preset is named `cxvm-<kind>-<version>`, builds in `build/<kind>-<version>` and has its own
  toolchain file, `.cxvm/toolchains/<kind>-<version>.cmake`.
- cxvm adds or rewrites only the preset of the current pin. Your own presets, comments and
  the presets of earlier pins stay; delete a preset you no longer want by hand.
- The file needs `"version": 3` or later (CMake 3.21). An existing file with an older version
  is reported, not changed.
- After deleting `.cxvm/`, the presets of earlier pins have no toolchain file until that
  toolchain is pinned again.
- CMake's documentation asks that `CMakeUserPresets.json` is not committed; cxvm does not add
  it to `.gitignore`.

## Clang and libstdc++

Clang has no C++ standard library of its own on Linux: it uses the newest GCC it finds on the
host, and an older clang may not compile a much newer libstdc++. Pair it with a GCC that cxvm
knows:

```sh
cxvm use clang@17 --gcc gcc@13     # writes [clang] gcc = "gcc@13.3.0"
cxvm use clang@17 --gcc system     # back to clang's own choice
```

- The toolchain file passes `--gcc-install-dir=…` with the compiler (`--gcc-toolchain=…` for
  clang before 16) and adds the rpath of that GCC's libstdc++.
- `cxvm env` and `cxvm exec` put the option in `CC` and `CXX` (`CC="…/clang --gcc-install-dir=…"`),
  which make, autotools, Meson and CMake accept. A `clang++` typed by hand gets no option:
  cxvm does not wrap compilers.
- The pairing belongs to the project. It applies to any clang that is active there, also one
  chosen with `CXVM_COMPILER`, and to no clang outside a project.
- `cxvm install` also installs a paired GCC that is missing; `cxvm current` shows the pairing.
- To pair with one of several distro GCCs, link it first: `cxvm link /usr --as
  gcc@15.2.0-local`. This needs clang 16 or later; an older clang always takes the newest GCC
  of a prefix.

## Checking a project

`cxvm doctor` checks the store, the active toolchain, the project of the current directory and
the download cache, and prints a hint under each problem. It only reads: nothing is fixed for
you. It exits with status 1 when it finds an error (something fails, or silently uses the wrong
compiler), and 0 when there are only warnings (something to clean up) or notes.

In the project it checks:

- that the pin, and the GCC of `[clang] gcc`, are installed;
- that `.cxvm/current`, `toolchain.cmake` and `env` are what `cxvm install` would write now,
  e.g. after a `git pull` changed `.cxvmrc`;
- that the `cxvm-…` presets in `CMakeUserPresets.json` are for toolchains that are still
  installed, and have their toolchain files;
- the CMake build directories up to three levels below the project root (hidden directories
  and projects with a `.cxvmrc` of their own are skipped). One configured with a toolchain file
  of cxvm must build with the compiler that file names now, unless it is named after the
  toolchain it builds with (`build/gcc-13.3.0`); a mismatch is an error. One configured without
  a toolchain file is compared with the pin, and a mismatch is a warning, since that may be on
  purpose. Build directories with a toolchain file of their own, e.g. for cross compiling, are
  left alone.

The compiler of a build directory is read from `CMakeFiles/<version>/CMake<LANG>Compiler.cmake`:
a compiler set by a toolchain file is not in `CMakeCache.txt`.

`cxvm remove` refuses, unless given `--force`, a toolchain that the global default or the
current directory uses: its pin, or the GCC its clang is paired with. It cannot know about
other projects. `cxvm prune` deletes the download cache and the logs of builds from source;
`--dry-run` lists them.

### Messages

Errors, warnings and notes go to stderr, each problem with a `hint:` line saying what to do.
Their labels are coloured on a terminal, the way clap colours its help: `NO_COLOR` turns colour
off, `CLICOLOR_FORCE` turns it on in a pipe, `CLICOLOR=0` and `TERM=dumb` turn it off. `-v` or
`--verbose` adds `debug:` lines: how the toolchain was resolved, which processes cxvm runs and
how they end, what it downloads and which files it writes.

## Shell and direnv

```sh
eval "$(cxvm env)"                 # bash, zsh; fish: cxvm env | source
cxvm exec -- make
```

Both put the toolchain's `bin/` first on `PATH` and set `CC` and `CXX`. They do not set
`LD_LIBRARY_PATH`: a program built this way with a GCC newer than the host's needs
`-Wl,-rpath,<toolchain>/lib64` itself, which the CMake toolchain file adds for you.

With [direnv](https://direnv.net), an `.envrc` next to `.cxvmrc`:

```sh
watch_file .cxvmrc
eval "$(cxvm env --shell bash)"
```

`.cxvm/env` holds the same exports for the current pin, for shells without cxvm on `PATH`:
`. .cxvm/env`.

## Shell completions

```sh
cxvm completions bash > ~/.local/share/bash-completion/completions/cxvm
cxvm completions zsh  > ~/.zfunc/_cxvm      # with ~/.zfunc in $fpath before compinit
cxvm completions fish > ~/.config/fish/completions/cxvm.fish
```

## VS Code and clangd

With `vscode = true`, cxvm keeps these keys of `.vscode/settings.json` in step with the pin
and leaves the rest of the file, comments included, as it is:

| Key | Value |
|---|---|
| `clangd.arguments`: `--query-driver=` | `<toolchain>/bin/*` for a GCC, so that clangd asks the pinned GCC for its built-in include directories. Globs of your own (a cross compiler) stay in the list; the glob of the previous pin goes. Nothing for clang or `system`. |
| `clangd.arguments`: `--compile-commands-dir=` | `${workspaceFolder}/build/<kind>-<version>` |
| `cmake.buildDirectory` | `${workspaceFolder}/build/<kind>-<version>` |
| `cmake.configureSettings`: `CMAKE_TOOLCHAIN_FILE` | `${workspaceFolder}/.cxvm/toolchain.cmake` |

- A `--compile-commands-dir` or `cmake.buildDirectory` that cxvm did not write is left alone,
  with a note.
- The `--query-driver` glob is an absolute path below your home directory. If the project
  commits `.vscode/settings.json`, leave `vscode` off and copy the settings into your user or
  workspace-local settings instead.
- The project root (where `.cxvmrc` is) must be the workspace folder.
- clangd needs `compile_commands.json` in the build directory: CMake Tools exports it by
  default; from a terminal, configure with `-DCMAKE_EXPORT_COMPILE_COMMANDS=ON`.
- In CMake Tools, select the `[Unspecified]` kit so that the toolchain file chooses the
  compiler, or a `cxvm-…` preset if `cmake_presets` is on (presets replace
  `cmake.buildDirectory` and `cmake.configureSettings`).
- Restart clangd after `cxvm use` (`clangd: Restart language server`): its arguments are read
  at start.
- For a paired clang, clangd itself must know `--gcc-install-dir`, so presumably clangd 16 or
  later (not tried with an older one).

What was checked, with clangd 21.1.8 started with the generated arguments (see
[backends.md](backends.md)): go-to-definition on `std::vector` opens
`<store>/toolchains/gcc/13.3.0/include/c++/13.3.0/bits/stl_vector.h` for a `gcc@13.3.0` pin,
and the paired GCC's header for a clang pin. VS Code itself was not driven: that the clangd
extension expands `${workspaceFolder}` in `clangd.arguments`, and what is said above about
CMake Tools kits and presets, comes from their documentation.
