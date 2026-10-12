# Contributing to cxvm

## Checks

CI runs these on every push, and a change is ready when they pass locally:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo +1.89 check --locked --workspace --all-targets   # the MSRV
```

## Tests

- Default tests never use the network. Tests that download or build real toolchains are in
  `tests/network.rs`, are `#[ignore]`d, and run with `cargo test --test network -- --ignored`.
- Tests use fake toolchains: small shell scripts named `gcc`, `g++`, `clang` and `clang++`,
  made by the helpers in `tests/common/mod.rs`. Its `Sandbox` points `CXVM_HOME`,
  `XDG_CACHE_HOME` and `CXVM_CATALOG` at temporary directories, so a test never touches your
  store, your cache or the embedded catalog.
- Logic takes the environment as parameters (`Paths::from_env`, `Context`) and never calls
  `std::env::set_var`, so tests can run in parallel.

## Code

- Errors are one enum, `Error` in `src/error.rs`. Every message ends with a `hint:` line that
  says what to do next; `every_error_says_what_to_do_next` checks each variant.
- The catalogs in `catalog/` are generated: `cargo xtask catalog [clang|gcc|gcc-source]`.
  Record what you find about upstream releases in [docs/backends.md](docs/backends.md).
- Open an issue first before adding a heavy dependency, changing the `.cxvmrc` format, or
  anything that would need root.
- Add an entry under `## [Unreleased]` in [CHANGELOG.md](CHANGELOG.md) for changes users will
  notice.

## Releasing

Set the version in `Cargo.toml`, rename `[Unreleased]` in CHANGELOG.md to `[x.y.z] - <date>`,
and push a `vx.y.z` tag. `release.yml` refuses a tag whose binary or CHANGELOG section does not
match, and otherwise creates a draft GitHub release from CI's musl archive. The maintainer
publishes the draft and runs `cargo publish`.
