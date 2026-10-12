//! `cxvm remove`: deleting installed toolchains, with the global default and what the
//! current directory uses protected.

mod common;

use std::fs;

use common::Sandbox;
use predicates::prelude::*;

#[test]
fn removes_an_installed_toolchain() {
    let s = Sandbox::new();
    let prefix = s.install("clang", "19.1.0");
    s.install("clang", "18.1.8");
    s.cxvm("")
        .args(["remove", "clang@19.1.0"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Removed clang@19.1.0"))
        .stderr(predicate::str::is_empty());
    assert!(!prefix.exists());
    assert!(s.store().join("toolchains/clang/18.1.8").is_dir());
    assert_eq!(fs::read_dir(s.store().join("staging")).unwrap().count(), 0);
}

#[test]
fn needs_an_exact_installed_id() {
    let s = Sandbox::new();
    s.install("clang", "19.1.0");
    s.cxvm("")
        .args(["remove", "clang@19"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("exact"));
    s.cxvm("")
        .args(["remove", "clang@19.1.7"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("clang@19.1.7 is not installed"))
        .stderr(predicate::str::contains("installed: clang@19.1.0"));
    assert!(s.store().join("toolchains/clang/19.1.0").is_dir());
}

#[test]
fn refuses_linked_toolchains() {
    let s = Sandbox::new();
    let prefix = s.path("opt/gcc-11");
    common::fake_toolchain(&prefix, "gcc", "11.4.0");
    s.cxvm("").args(["link"]).arg(&prefix).assert().success();
    s.cxvm("")
        .args(["remove", "gcc@11.4.0-local"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("cxvm unlink gcc@11.4.0-local"));
    assert!(prefix.join("bin/gcc").is_file());
}

#[test]
fn the_global_default_needs_force() {
    let s = Sandbox::new();
    let prefix = s.install("clang", "19.1.0");
    s.global("clang@19.1.0");
    s.cxvm("")
        .args(["remove", "clang@19.1.0"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("is the global default"))
        .stderr(predicate::str::contains("--force"));
    assert!(prefix.is_dir());
    s.cxvm("")
        .args(["remove", "clang@19.1.0", "--force"])
        .assert()
        .success()
        .stderr(predicate::str::contains("was the global default"));
    assert!(!prefix.exists());
}

#[test]
fn a_partial_global_with_another_match_is_not_in_the_way() {
    let s = Sandbox::new();
    s.install("clang", "19.1.0");
    s.install("clang", "19.1.7");
    s.global("clang@19");
    s.cxvm("")
        .args(["remove", "clang@19.1.7"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
}

#[test]
fn a_toolchain_pinned_here_needs_force() {
    let s = Sandbox::new();
    let prefix = s.install("clang", "19.1.0");
    let rc = s.pin("", "clang@19.1.0");
    s.cxvm("a")
        .args(["remove", "clang@19.1.0"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(format!(
            "error: clang@19.1.0 is pinned here (set by {})\n  hint: pin another with `cxvm use \
             <id>` first, or run `cxvm remove clang@19.1.0 --force`",
            rc.display()
        )));
    assert!(prefix.is_dir());
    s.cxvm("a")
        .args(["remove", "clang@19.1.0", "--force"])
        .assert()
        .success()
        .stderr(predicate::str::contains("still pinned here"))
        .stderr(predicate::str::contains("cxvm install"));
    assert!(!prefix.exists());
}

#[test]
fn a_toolchain_cxvm_compiler_selects_needs_force() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    s.cxvm("")
        .args(["remove", "gcc@13.3.0"])
        .env("CXVM_COMPILER", "gcc@13")
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "gcc@13.3.0 is pinned here (set by CXVM_COMPILER)",
        ));
}

#[test]
fn the_gcc_a_clang_is_paired_with_here_needs_force() {
    let s = Sandbox::new();
    s.install("clang", "19.1.0");
    let gcc = s.install("gcc", "13.3.0");
    let rc = s.project().join(".cxvmrc");
    fs::write(
        &rc,
        "compiler = \"clang@19.1.0\"\n\n[clang]\ngcc = \"gcc@13.3.0\"\n",
    )
    .unwrap();
    s.cxvm("")
        .args(["remove", "gcc@13.3.0"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(format!(
            "error: clang uses the headers and libstdc++ of gcc@13.3.0 here (`[clang] gcc` in \
             {})\n",
            rc.display()
        )));
    assert!(gcc.is_dir());
    s.cxvm("")
        .args(["remove", "gcc@13.3.0", "--force"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "clang is still paired with gcc@13.3.0 here",
        ));
    // A pairing that a GCC pin leaves unused is no reason to keep it.
    s.install("gcc", "13.3.0");
    s.install("gcc", "14.2.0");
    fs::write(
        &rc,
        "compiler = \"gcc@14.2.0\"\n\n[clang]\ngcc = \"gcc@13.3.0\"\n",
    )
    .unwrap();
    s.cxvm("")
        .args(["remove", "gcc@13.3.0"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
}

#[test]
fn build_dirs_configured_with_it_are_named() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    let prefix = s.install("gcc", "14.2.0");
    s.pin("", "gcc@13.3.0");
    let build = s.project().join("build/gcc-14.2.0");
    fs::create_dir_all(&build).unwrap();
    fs::write(
        build.join("CMakeCache.txt"),
        format!(
            "CMAKE_CXX_COMPILER:FILEPATH={}\n",
            prefix.join("bin/g++").display()
        ),
    )
    .unwrap();
    s.cxvm("")
        .args(["remove", "gcc@14.2.0"])
        .assert()
        .success()
        .stderr(predicate::str::contains(format!(
            "warning: {} was configured with gcc@14.2.0 and cannot build any more",
            build.display()
        )));
}
