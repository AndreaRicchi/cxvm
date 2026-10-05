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
            {"id": "gcc@13.3.0", "path": gcc, "active": true, "global": false,
             "linked": false},
            {"id": "clang@18.1.8", "path": s.store().join("toolchains/clang/18.1.8"),
             "active": false, "global": true, "linked": false},
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

#[test]
fn linked_toolchains_are_listed_and_marked() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    let prefix = s.path("opt/gcc-11");
    common::fake_toolchain(&prefix, "gcc", "11.4.0");
    std::fs::write(
        s.store().join("links.toml"),
        format!(
            "[[toolchain]]\nid = \"gcc@11.4.0-local\"\nprefix = \"{p}\"\n\
             cc = \"{p}/bin/gcc\"\ncxx = \"{p}/bin/g++\"\nversion = \"gcc 11.4.0\"\n",
            p = prefix.display()
        ),
    )
    .unwrap();
    s.pin("", "gcc@11.4.0-local");
    s.cxvm("")
        .arg("list")
        .assert()
        .success()
        .stdout("* gcc@11.4.0-local  active, linked\n  gcc@13.3.0\n");
    let out = s.cxvm("").args(["list", "--json"]).output().unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["path"], serde_json::json!(prefix));
    assert_eq!(json[0]["linked"], serde_json::json!(true));
}

/// A catalog of `versions`, all pointing at an unreachable URL.
fn catalog(s: &Sandbox, versions: &[&str]) -> std::path::PathBuf {
    let path = s.path("catalog.toml");
    let entries: Vec<String> = versions
        .iter()
        .map(|v| {
            format!(
                "[[clang]]\nversion = \"{v}\"\nasset = \"LLVM-{v}-Linux-X64.tar.xz\"\n\
                 url = \"http://127.0.0.1:1/LLVM-{v}-Linux-X64.tar.xz\"\nsize = 1610612736\n\
                 sha256 = \"{}\"\nsource = \"test\"\n",
                "0".repeat(64)
            )
        })
        .collect();
    std::fs::write(&path, entries.join("\n")).unwrap();
    path
}

#[test]
fn available_lists_the_catalog_newest_first_marking_installed() {
    let s = Sandbox::new();
    let catalog = catalog(&s, &["18.1.8", "19.1.7", "19.1.0"]);
    s.install("clang", "19.1.0");
    let out = s
        .cxvm("")
        .args(["list", "--available"])
        .env("CXVM_CATALOG", &catalog)
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].contains("(* = installed)"), "{text}");
    assert!(lines[0].contains("CXVM_CATALOG"), "{text}");
    assert_eq!(
        lines[1..],
        [
            "",
            "  clang  download  versions",
            "  19      1.5 GiB  19.1.7  19.1.0*",
            "  18      1.5 GiB  18.1.8",
        ]
    );
}

#[test]
fn available_json_has_ids_sizes_and_urls() {
    let s = Sandbox::new();
    let catalog = catalog(&s, &["19.1.0"]);
    let out = s
        .cxvm("")
        .args(["list", "--available", "--json"])
        .env("CXVM_CATALOG", &catalog)
        .output()
        .unwrap();
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        json,
        serde_json::json!([{
            "id": "clang@19.1.0",
            "size": 1610612736u64,
            "installed": false,
            "url": "http://127.0.0.1:1/LLVM-19.1.0-Linux-X64.tar.xz",
            "from_source": false,
        }])
    );
}

#[test]
fn available_uses_the_embedded_catalog_by_default() {
    let s = Sandbox::new();
    s.cxvm("")
        .args(["list", "--available"])
        .env_remove("CXVM_CATALOG")
        .assert()
        .success()
        .stdout(predicate::str::contains("built into cxvm"))
        .stdout(predicate::str::contains("  gcc  "))
        .stdout(predicate::str::contains("15.2.0"))
        .stdout(predicate::str::contains("  clang  "))
        .stdout(predicate::str::contains("19.1.0"))
        .stdout(predicate::str::contains("11.1.0"));
}

#[test]
fn available_reports_a_broken_catalog_file() {
    let s = Sandbox::new();
    let path = s.path("catalog.toml");
    std::fs::write(&path, "[[clang]]\nversion = 1\n").unwrap();
    s.cxvm("")
        .args(["list", "--available"])
        .env("CXVM_CATALOG", &path)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("catalog.toml is invalid"))
        .stderr(predicate::str::contains("unset CXVM_CATALOG"));
}
