//! `cxvm global`: the default toolchain outside projects.

mod common;

use std::fs;

use common::Sandbox;
use predicates::prelude::*;

#[test]
fn sets_the_exact_id_as_the_global_default() {
    let s = Sandbox::new();
    s.install("gcc", "13.2.0");
    let prefix = s.install("gcc", "13.3.0");
    let config = s.store().join("config.toml");
    s.cxvm("")
        .args(["global", "gcc@13"])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "Global default set to gcc@13.3.0 (from gcc@13) in {}",
            config.display()
        )));
    assert!(
        fs::read_to_string(&config)
            .unwrap()
            .contains("compiler = \"gcc@13.3.0\"")
    );
    s.cxvm("a")
        .arg("current")
        .assert()
        .success()
        .stdout(format!(
            "gcc@13.3.0 (global default in {})\n{}\n",
            config.display(),
            prefix.display()
        ));
}

#[test]
fn system_can_be_the_global_default() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    s.global("gcc@13.3.0");
    s.cxvm("").args(["global", "system"]).assert().success();
    s.cxvm("")
        .arg("current")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("system (global default in"));
}

#[test]
fn a_toolchain_that_is_not_installed_is_refused() {
    let s = Sandbox::new();
    let config = s.global("system");
    s.cxvm("")
        .args(["global", "clang@19"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("clang@19 is not installed"))
        .stderr(predicate::str::contains("hint:"));
    assert_eq!(
        fs::read_to_string(config).unwrap(),
        "compiler = \"system\"\n"
    );
}

#[test]
fn notes_when_a_project_pin_overrides_it_here() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    let rc = s.pin("", "system");
    s.cxvm("a")
        .args(["global", "gcc@13.3.0"])
        .assert()
        .success()
        .stderr(predicate::str::contains("note:"))
        .stderr(predicate::str::contains(rc.display().to_string()));
}

#[test]
fn a_broken_config_is_replaced() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    fs::create_dir_all(s.store()).unwrap();
    fs::write(s.store().join("config.toml"), "not toml").unwrap();
    s.cxvm("").args(["global", "gcc@13.3.0"]).assert().success();
    s.cxvm("").arg("current").assert().success();
}

#[test]
fn invalid_ids_are_rejected() {
    Sandbox::new()
        .cxvm("")
        .args(["global", "gcc13"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("invalid toolchain id"));
}
