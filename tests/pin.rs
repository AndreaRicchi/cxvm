//! `cxvm use` and `cxvm install` without arguments: pinning a toolchain in a project and
//! (re)generating `.cxvm/`. (Not `tests/use.rs`: `use` is a keyword.)

mod common;

use std::fs;
use std::path::Path;

use common::{Sandbox, fake_toolchain};
use predicates::prelude::*;

fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path.as_ref()).unwrap_or_else(|e| panic!("{}: {e}", path.as_ref().display()))
}

#[test]
fn use_pins_the_exact_id_and_generates_the_project_folder() {
    let s = Sandbox::new();
    let prefix = s.install("gcc", "13.3.0");
    let root = s.project();
    s.cxvm("")
        .args(["use", "gcc@13.3.0"])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "Pinned gcc@13.3.0 in {}",
            root.join(".cxvmrc").display()
        )))
        .stdout(predicate::str::contains(
            "cmake -S . -B build/gcc-13.3.0 -DCMAKE_TOOLCHAIN_FILE=.cxvm/toolchain.cmake",
        ));
    assert_eq!(read(root.join(".cxvmrc")), "compiler = \"gcc@13.3.0\"\n");
    assert_eq!(fs::read_link(root.join(".cxvm/current")).unwrap(), prefix);
    let cmake = read(root.join(".cxvm/toolchain.cmake"));
    assert!(
        cmake.contains(&format!(
            "set(CMAKE_C_COMPILER   \"{}\")",
            prefix.join("bin/gcc").display()
        )),
        "{cmake}"
    );
    assert!(
        cmake.contains(&format!(
            "set(CMAKE_CXX_COMPILER \"{}\")",
            prefix.join("bin/g++").display()
        )),
        "{cmake}"
    );
    let env = read(root.join(".cxvm/env"));
    assert!(
        env.contains(&format!("export CC='{}'", prefix.join("bin/gcc").display())),
        "{env}"
    );
    assert_eq!(read(root.join(".gitignore")), ".cxvm/\n");
    s.cxvm("a/b")
        .arg("current")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("gcc@13.3.0 (set by"));
}

#[test]
fn use_resolves_partial_versions_to_the_highest_installed() {
    let s = Sandbox::new();
    s.install("gcc", "13.2.0");
    s.install("gcc", "13.3.0");
    s.install("gcc", "14.2.0");
    s.cxvm("")
        .args(["use", "gcc@13"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Pinned gcc@13.3.0"))
        .stdout(predicate::str::contains("gcc@13"));
    assert_eq!(
        read(s.project().join(".cxvmrc")),
        "compiler = \"gcc@13.3.0\"\n"
    );
}

#[test]
fn use_updates_the_nearest_pin_from_a_subdirectory() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    s.install("clang", "18.1.8");
    s.pin("", "gcc@13.3.0");
    s.cxvm("a/b")
        .args(["use", "clang@18"])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "cmake -S {root} -B {root}/build/clang-18.1.8 \
             -DCMAKE_TOOLCHAIN_FILE={root}/.cxvm/toolchain.cmake",
            root = s.project().display()
        )));
    assert_eq!(
        read(s.project().join(".cxvmrc")),
        "compiler = \"clang@18.1.8\"\n"
    );
    assert!(s.project().join(".cxvm/toolchain.cmake").exists());
    assert!(!s.project().join("a/b/.cxvmrc").exists());
    assert!(!s.project().join("a/b/.cxvm").exists());
}

#[test]
fn use_without_a_pin_makes_the_current_directory_the_project() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    s.cxvm("a/b").args(["use", "gcc@13.3.0"]).assert().success();
    assert!(s.project().join("a/b/.cxvmrc").exists());
    assert!(s.project().join("a/b/.cxvm/current").exists());
    assert!(!s.project().join(".cxvmrc").exists());
}

#[test]
fn switching_toolchains_regenerates_everything() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    let clang = s.install("clang", "18.1.8");
    s.cxvm("").args(["use", "gcc@13.3.0"]).assert().success();
    s.cxvm("").args(["use", "clang@18.1.8"]).assert().success();
    let root = s.project();
    assert_eq!(fs::read_link(root.join(".cxvm/current")).unwrap(), clang);
    let cmake = read(root.join(".cxvm/toolchain.cmake"));
    assert!(
        cmake.contains("clang@18.1.8") && !cmake.contains("gcc@13.3.0"),
        "{cmake}"
    );
    assert_eq!(read(root.join(".gitignore")), ".cxvm/\n", "added once");
}

#[test]
fn use_of_a_toolchain_that_is_not_installed_changes_nothing() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    s.cxvm("")
        .args(["use", "gcc@15"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("gcc@15 is not installed"))
        .stderr(predicate::str::contains("cxvm link"))
        .stderr(predicate::str::contains("gcc@13.3.0"));
    assert!(!s.project().join(".cxvmrc").exists());
    assert!(!s.project().join(".cxvm").exists());
}

#[test]
fn use_never_picks_a_linked_toolchain_for_an_unsuffixed_id() {
    let s = Sandbox::new();
    let prefix = s.path("opt/gcc-11");
    fake_toolchain(&prefix, "gcc", "11.4.0");
    s.cxvm("").arg("link").arg(&prefix).assert().success();
    s.cxvm("")
        .args(["use", "gcc@11"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("gcc@11.4.0-local"))
        .stderr(predicate::str::contains("`gcc@11-local`"));
    s.cxvm("").args(["use", "gcc@11-local"]).assert().success();
    assert_eq!(
        read(s.project().join(".cxvmrc")),
        "compiler = \"gcc@11.4.0-local\"\n"
    );
    assert_eq!(
        fs::read_link(s.project().join(".cxvm/current")).unwrap(),
        prefix
    );
}

#[test]
fn use_system_points_at_the_compilers_on_path() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    s.cxvm("").args(["use", "gcc@13.3.0"]).assert().success();
    s.cxvm("")
        .args(["use", "system"])
        .assert()
        .success()
        .stdout(predicate::str::contains("build/system"));
    let root = s.project();
    assert_eq!(read(root.join(".cxvmrc")), "compiler = \"system\"\n");
    assert!(root.join(".cxvm/current").symlink_metadata().is_err());
    let cmake = read(root.join(".cxvm/toolchain.cmake"));
    assert!(
        cmake.contains(&format!("\"{}\"", s.bin().join("cc").display())),
        "{cmake}"
    );
    assert!(
        cmake.contains(&format!("\"{}\"", s.bin().join("c++").display())),
        "{cmake}"
    );
    let env = read(root.join(".cxvm/env"));
    assert!(!env.contains("PATH"), "{env}");
}

#[test]
fn gcc_toolchain_files_carry_the_libstdcxx_rpath() {
    let s = Sandbox::new();
    let prefix = s.install("gcc", "14.2.0");
    fs::create_dir_all(prefix.join("lib64")).unwrap();
    fs::write(prefix.join("lib64/libstdc++.so.6"), "").unwrap();
    s.cxvm("").args(["use", "gcc@14.2.0"]).assert().success();
    let cmake = read(s.project().join(".cxvm/toolchain.cmake"));
    let lib64 = prefix.join("lib64").canonicalize().unwrap();
    assert!(
        cmake.contains(&format!(
            "set(CMAKE_EXE_LINKER_FLAGS_INIT    \"-Wl,-rpath,{}\")",
            lib64.display()
        )),
        "{cmake}"
    );
}

#[test]
fn clang_toolchain_files_have_no_rpath_yet() {
    let s = Sandbox::new();
    s.install("clang", "18.1.8");
    s.cxvm("").args(["use", "clang@18.1.8"]).assert().success();
    let cmake = read(s.project().join(".cxvm/toolchain.cmake"));
    assert!(!cmake.contains("rpath"), "{cmake}");
}

#[test]
fn incomplete_toolchains_are_refused() {
    let s = Sandbox::new();
    let prefix = s.store().join("toolchains/gcc/13.3.0");
    fs::create_dir_all(&prefix).unwrap();
    s.cxvm("")
        .args(["use", "gcc@13.3.0"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            prefix.join("bin/gcc").display().to_string(),
        ))
        .stderr(predicate::str::contains("hint:"));
    assert!(!s.project().join(".cxvmrc").exists());
}

#[test]
fn use_notes_when_the_env_var_overrides_the_pin() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    s.cxvm("")
        .args(["use", "gcc@13.3.0"])
        .env("CXVM_COMPILER", "clang@18")
        .assert()
        .success()
        .stderr(predicate::str::contains("CXVM_COMPILER"));
}

#[test]
fn use_rejects_invalid_ids() {
    let s = Sandbox::new();
    s.cxvm("")
        .args(["use", "gcc13"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("invalid toolchain id"));
}

#[test]
fn install_without_arguments_regenerates_the_project_folder() {
    let s = Sandbox::new();
    let prefix = s.install("gcc", "13.3.0");
    s.pin("", "gcc@13");
    s.cxvm("a/b")
        .arg("install")
        .assert()
        .success()
        .stdout(predicate::str::contains("gcc@13.3.0"));
    let root = s.project();
    assert_eq!(fs::read_link(root.join(".cxvm/current")).unwrap(), prefix);
    assert!(root.join(".cxvm/toolchain.cmake").exists());
    assert_eq!(
        read(root.join(".cxvmrc")),
        "compiler = \"gcc@13\"\n",
        "left as written"
    );
}

#[test]
fn install_without_arguments_ignores_the_env_var() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    s.pin("", "gcc@13.3.0");
    s.cxvm("")
        .arg("install")
        .env("CXVM_COMPILER", "clang@18")
        .assert()
        .success();
    assert!(read(s.project().join(".cxvm/toolchain.cmake")).contains("gcc@13.3.0"));
}

#[test]
fn install_without_a_pin_says_how_to_make_one() {
    let s = Sandbox::new();
    s.cxvm("a")
        .arg("install")
        .assert()
        .code(1)
        .stderr(predicate::str::contains(".cxvmrc"))
        .stderr(predicate::str::contains("cxvm use"));
}

#[test]
fn install_of_a_missing_pin_points_to_link() {
    let s = Sandbox::new();
    let rc = s.pin("", "gcc@13.3.0");
    s.cxvm("")
        .arg("install")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("gcc@13.3.0 is not installed"))
        .stderr(predicate::str::contains(rc.display().to_string()))
        .stderr(predicate::str::contains("cxvm link"));
}
