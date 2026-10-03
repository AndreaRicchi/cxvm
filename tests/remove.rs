//! `cxvm remove`: deleting installed toolchains, with the global default protected.

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
fn warns_when_the_project_still_pins_it() {
    let s = Sandbox::new();
    s.install("clang", "19.1.0");
    s.pin("", "clang@19.1.0");
    s.cxvm("a")
        .args(["remove", "clang@19.1.0"])
        .assert()
        .success()
        .stderr(predicate::str::contains("still pinned here"))
        .stderr(predicate::str::contains("cxvm install"));
}
