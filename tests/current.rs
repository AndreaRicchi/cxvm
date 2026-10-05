//! `cxvm current`: the active toolchain and why, from any directory of a project.

mod common;

use common::{Sandbox, stdout};
use predicates::prelude::*;
use serde_json::json;

#[test]
fn defaults_to_the_system_compiler_on_path() {
    let s = Sandbox::new();
    s.cxvm("a")
        .arg("current")
        .assert()
        .success()
        .stdout(format!(
            "system (no .cxvmrc or global default)\n{}\n",
            s.bin().join("cc").display()
        ));
}

#[test]
fn system_without_cc_on_path_says_so() {
    let s = Sandbox::new();
    s.cxvm("")
        .arg("current")
        .env("PATH", s.path("nothing"))
        .assert()
        .success()
        .stdout("system (no .cxvmrc or global default)\nno `cc` found on PATH\n");
}

#[test]
fn system_skips_the_store_on_path() {
    let s = Sandbox::new();
    let prefix = s.install("gcc", "13.3.0");
    s.fake_compiler(&prefix.join("bin"), "cc");
    let path = std::env::join_paths([prefix.join("bin"), s.bin()]).unwrap();
    s.cxvm("")
        .arg("current")
        .env("PATH", path)
        .assert()
        .success()
        .stdout(predicate::str::ends_with(format!(
            "{}\n",
            s.bin().join("cc").display()
        )));
}

#[test]
fn project_pin_applies_from_every_nested_directory() {
    let s = Sandbox::new();
    let prefix = s.install("gcc", "13.3.0");
    let rc = s.pin("", "gcc@13.3.0");
    for dir in ["", "a", "a/b", "a/b/c"] {
        s.cxvm(dir)
            .arg("current")
            .assert()
            .success()
            .stdout(format!(
                "gcc@13.3.0 (set by {})\n{}\n",
                rc.display(),
                prefix.display()
            ));
    }
}

#[test]
fn nearest_pin_wins_in_nested_projects() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    s.install("clang", "18.1.8");
    s.pin("", "gcc@13.3.0");
    let inner = s.pin("a/b", "clang@18.1.8");
    s.cxvm("a/b/c")
        .arg("current")
        .assert()
        .success()
        .stdout(predicate::str::starts_with(format!(
            "clang@18.1.8 (set by {})\n",
            inner.display()
        )));
    s.cxvm("a")
        .arg("current")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("gcc@13.3.0 "));
}

#[test]
fn partial_pin_shows_what_it_resolved_from() {
    let s = Sandbox::new();
    s.install("gcc", "13.2.0");
    let best = s.install("gcc", "13.3.0");
    s.install("gcc", "14.2.0");
    let rc = s.pin("", "gcc@13");
    s.cxvm("a")
        .arg("current")
        .assert()
        .success()
        .stdout(format!(
            "gcc@13.3.0 (gcc@13, set by {})\n{}\n",
            rc.display(),
            best.display()
        ));
}

#[test]
fn global_default_applies_outside_projects() {
    let s = Sandbox::new();
    s.install("gcc", "14.2.0");
    let config = s.global("gcc@14");
    s.cxvm("a")
        .arg("current")
        .assert()
        .success()
        .stdout(predicate::str::starts_with(format!(
            "gcc@14.2.0 (gcc@14, global default in {})\n",
            config.display()
        )));
}

#[test]
fn env_var_overrides_the_project_pin() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    s.install("clang", "18.1.8");
    s.pin("", "gcc@13.3.0");
    s.cxvm("a")
        .arg("current")
        .env("CXVM_COMPILER", "clang@18")
        .assert()
        .success()
        .stdout(predicate::str::starts_with(
            "clang@18.1.8 (clang@18, set by CXVM_COMPILER)\n",
        ));
}

#[test]
fn pin_that_is_not_installed_fails_with_a_next_step() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    s.install("gcc", "14.2.0");
    s.install("clang", "18.1.8");
    let rc = s.pin("", "gcc@15.1.0");
    s.cxvm("a/b")
        .arg("current")
        .assert()
        .code(1)
        .stdout(format!("gcc@15.1.0 (set by {})\n", rc.display()))
        .stderr(predicate::str::contains("gcc@15.1.0 is not installed"))
        .stderr(predicate::str::contains("cxvm install gcc@15.1.0"))
        .stderr(predicate::str::contains("gcc@13.3.0, gcc@14.2.0"))
        .stderr(predicate::str::contains("clang@18.1.8").not());
}

#[test]
fn partial_pin_without_a_match_fails() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    s.pin("", "gcc@14");
    s.cxvm("")
        .arg("current")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("gcc@14 is not installed"))
        .stderr(predicate::str::contains("cxvm install gcc@14"));
}

#[test]
fn missing_linked_toolchain_points_to_link() {
    let s = Sandbox::new();
    s.pin("", "gcc@11.4.0-local");
    s.cxvm("")
        .arg("current")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("cxvm link"))
        .stderr(predicate::str::contains("cxvm install").not());
}

#[test]
fn invalid_pin_fails_naming_the_file() {
    let s = Sandbox::new();
    let rc = s.pin("", "gcc13");
    s.cxvm("a/b/c")
        .arg("current")
        .assert()
        .code(1)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(rc.display().to_string()))
        .stderr(predicate::str::contains("hint:"));
}

#[test]
fn json_for_an_installed_toolchain() {
    let s = Sandbox::new();
    let prefix = s.install("gcc", "13.3.0");
    let rc = s.pin("", "gcc@13");
    let out = s.cxvm("a").args(["current", "--json"]).output().unwrap();
    assert!(out.status.success(), "{}", stdout(&out));
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        value,
        json!({
            "id": "gcc@13.3.0",
            "requested": "gcc@13",
            "source": "cxvmrc",
            "source_path": rc,
            "installed": true,
            "path": prefix,
            "clang_gcc": null,
        })
    );
}

#[test]
fn json_for_system_and_env() {
    let s = Sandbox::new();
    let out = s
        .cxvm("")
        .args(["current", "--json"])
        .env("CXVM_COMPILER", "system")
        .output()
        .unwrap();
    assert!(out.status.success());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        value,
        json!({
            "id": "system",
            "requested": "system",
            "source": "env",
            "source_path": null,
            "installed": true,
            "path": s.bin().join("cc"),
            "clang_gcc": null,
        })
    );
}

#[test]
fn json_for_a_missing_toolchain_still_prints_and_fails() {
    let s = Sandbox::new();
    let config = s.global("clang@19");
    let out = s.cxvm("").args(["current", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        value,
        json!({
            "id": null,
            "requested": "clang@19",
            "source": "global",
            "source_path": config,
            "installed": false,
            "path": null,
            "clang_gcc": null,
        })
    );
}
