//! `cxvm list`: installed toolchains with the active one and the global default marked.

mod common;

use common::{Sandbox, stdout};
use predicates::prelude::*;

#[test]
fn empty_store_says_so_with_a_hint() {
    let s = Sandbox::new();
    s.cxvm("")
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("No toolchains installed"))
        .stdout(predicate::str::contains("hint:"))
        .stdout(predicate::str::contains("active: system"));
}

#[test]
fn lists_installed_toolchains_marking_active_and_global() {
    let s = Sandbox::new();
    s.install("clang", "18.1.8");
    s.install("gcc", "14.2.0");
    s.install("gcc", "13.3.0");
    s.global("gcc@14");
    s.pin("", "clang@18.1.8");
    let out = s.cxvm("a/b").arg("list").output().unwrap();
    assert!(out.status.success());
    let lines: Vec<String> = stdout(&out)
        .lines()
        .map(str::trim_end)
        .map(String::from)
        .collect();
    assert_eq!(
        lines,
        [
            "  gcc@13.3.0",
            "  gcc@14.2.0    global",
            "* clang@18.1.8  active",
        ]
    );
}

#[test]
fn one_toolchain_can_be_active_and_global() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    s.global("gcc@13.3.0");
    s.cxvm("")
        .arg("list")
        .assert()
        .success()
        .stdout("* gcc@13.3.0  active, global\n");
}

#[test]
fn notes_active_and_global_toolchains_that_are_not_in_the_list() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    let config = s.global("clang@19");
    let rc = s.pin("", "gcc@15.1.0");
    let out = s.cxvm("a").arg("list").output().unwrap();
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(
        text.contains(&format!(
            "active: gcc@15.1.0 (set by {}), not installed",
            rc.display()
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "global: clang@19 (in {}), not installed",
            config.display()
        )),
        "{text}"
    );
}

#[test]
fn system_global_is_noted() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    s.global("system");
    s.cxvm("")
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("active: system"))
        .stdout(predicate::str::contains("global: system"));
}

#[test]
fn json_lists_every_toolchain_with_flags() {
    let s = Sandbox::new();
    let gcc = s.install("gcc", "13.3.0");
    s.install("clang", "18.1.8");
    s.global("clang@18");
    s.pin("", "gcc@13");
    let out = s.cxvm("").args(["list", "--json"]).output().unwrap();
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        json,
        serde_json::json!([
            {"id": "gcc@13.3.0", "path": gcc, "active": true, "global": false},
            {"id": "clang@18.1.8", "path": s.store().join("toolchains/clang/18.1.8"),
             "active": false, "global": true},
        ])
    );
}

#[test]
fn json_of_an_empty_store_is_an_empty_array() {
    let s = Sandbox::new();
    s.cxvm("")
        .args(["list", "--json"])
        .assert()
        .success()
        .stdout("[]\n");
}

#[test]
fn invalid_pin_is_an_error_naming_the_file() {
    let s = Sandbox::new();
    let rc = s.pin("", "gcc13");
    s.cxvm("a")
        .arg("list")
        .assert()
        .code(1)
        .stderr(predicate::str::contains(rc.display().to_string()));
}
