//! `cxvm link` / `cxvm unlink`: registering toolchains that already exist on disk.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;

use common::{Sandbox, fake_toolchain};
use predicates::prelude::*;

fn links_toml(s: &Sandbox) -> String {
    fs::read_to_string(s.store().join("links.toml")).unwrap_or_default()
}

#[test]
fn links_a_prefix_under_a_detected_local_id() {
    let s = Sandbox::new();
    let prefix = s.path("opt/gcc-11");
    fake_toolchain(&prefix, "gcc", "11.4.0");
    s.cxvm("")
        .arg("link")
        .arg(&prefix)
        .assert()
        .success()
        .stdout(predicate::str::contains("gcc@11.4.0-local"))
        .stdout(predicate::str::contains(prefix.display().to_string()));
    s.cxvm("")
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("gcc@11.4.0-local  linked"));
    let links = links_toml(&s);
    assert!(
        links.contains(&format!("prefix = \"{}\"", prefix.display())),
        "{links}"
    );
    assert!(links.contains("version = \"gcc (Fake) 11.4.0\""), "{links}");
}

#[test]
fn relative_prefixes_are_stored_absolute() {
    let s = Sandbox::new();
    let prefix = s.path("opt/llvm-18");
    fake_toolchain(&prefix, "clang", "18.1.8");
    s.cxvm("a")
        .args(["link", "../../opt/./llvm-18/"])
        .assert()
        .success()
        .stdout(predicate::str::contains("clang@18.1.8-local"));
    let links = links_toml(&s);
    assert!(
        links.contains(&format!("prefix = \"{}\"", prefix.display())),
        "{links}"
    );
}

#[test]
fn symlinked_prefixes_are_kept_as_given() {
    let s = Sandbox::new();
    let real = s.path("opt/gcc-11.4.0");
    fake_toolchain(&real, "gcc", "11.4.0");
    let alias = s.path("opt/gcc-11");
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    s.cxvm("").arg("link").arg(&alias).assert().success();
    assert!(links_toml(&s).contains(&format!("prefix = \"{}\"", alias.display())));
}

#[test]
fn as_overrides_the_suffix() {
    let s = Sandbox::new();
    let prefix = s.path("opt/llvm-18");
    fake_toolchain(&prefix, "clang", "18.1.8");
    s.cxvm("")
        .arg("link")
        .arg(&prefix)
        .args(["--as", "clang@18.1.8-mine"])
        .assert()
        .success();
    s.cxvm("")
        .arg("list")
        .assert()
        .stdout(predicate::str::contains("clang@18.1.8-mine"));
}

#[test]
fn as_must_be_exact_suffixed_and_match_the_prefix() {
    let s = Sandbox::new();
    let prefix = s.path("opt/llvm-18");
    fake_toolchain(&prefix, "clang", "18.1.8");
    for (id, expected) in [
        ("clang@18", "exact"),
        ("clang@18.1.8", "suffix"),
        ("gcc@18.1.8-local", "bin/gcc"),
        ("clang@17.0.6-local", "18.1.8"),
    ] {
        s.cxvm("")
            .arg("link")
            .arg(&prefix)
            .args(["--as", id])
            .assert()
            .code(1)
            .stderr(predicate::str::contains(expected))
            .stderr(predicate::str::contains("hint:"));
    }
    assert_eq!(links_toml(&s), "");
}

#[test]
fn a_prefix_with_both_kinds_needs_as() {
    let s = Sandbox::new();
    let prefix = s.path("usr");
    fake_toolchain(&prefix, "gcc", "13.3.0");
    fake_toolchain(&prefix, "clang", "18.1.8");
    s.cxvm("")
        .arg("link")
        .arg(&prefix)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("both"))
        .stderr(predicate::str::contains("--as"));
    s.cxvm("")
        .arg("link")
        .arg(&prefix)
        .args(["--as", "gcc@13.3.0-sys"])
        .assert()
        .success();
}

#[test]
fn undetectable_version_is_accepted_with_as_and_a_warning() {
    let s = Sandbox::new();
    let prefix = s.path("opt/odd");
    for name in ["clang", "clang++"] {
        let path = prefix.join("bin").join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "#!/bin/sh\necho 'odd compiler'\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    s.cxvm("")
        .arg("link")
        .arg(&prefix)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("--as"));
    s.cxvm("")
        .arg("link")
        .arg(&prefix)
        .args(["--as", "clang@9.0.1-odd"])
        .assert()
        .success()
        .stderr(predicate::str::contains("warning:"));
}

#[test]
fn prefixes_without_compilers_are_rejected() {
    let s = Sandbox::new();
    fs::create_dir_all(s.path("empty")).unwrap();
    let gcc_only = s.path("gcc-only");
    fake_toolchain(&gcc_only, "gcc", "13.3.0");
    fs::remove_file(gcc_only.join("bin/g++")).unwrap();
    for (prefix, expected) in [
        (s.path("empty"), "bin/gcc"),
        (s.path("missing"), "missing"),
        (gcc_only, "bin/g++"),
    ] {
        s.cxvm("")
            .arg("link")
            .arg(&prefix)
            .assert()
            .code(1)
            .stderr(predicate::str::contains(expected))
            .stderr(predicate::str::contains("hint:"));
    }
}

#[test]
fn prefixes_inside_the_store_are_rejected() {
    let s = Sandbox::new();
    let prefix = s.install("gcc", "13.3.0");
    s.cxvm("")
        .arg("link")
        .arg(&prefix)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("store"));
}

#[test]
fn relinking_the_same_prefix_is_a_no_op() {
    let s = Sandbox::new();
    let prefix = s.path("opt/gcc-11");
    fake_toolchain(&prefix, "gcc", "11.4.0");
    s.cxvm("").arg("link").arg(&prefix).assert().success();
    let before = links_toml(&s);
    s.cxvm("")
        .arg("link")
        .arg(&prefix)
        .assert()
        .success()
        .stdout(predicate::str::contains("already linked"));
    assert_eq!(links_toml(&s), before);
}

#[test]
fn an_id_linked_elsewhere_is_rejected() {
    let s = Sandbox::new();
    let first = s.path("opt/gcc-11");
    let second = s.path("home/gcc-11");
    fake_toolchain(&first, "gcc", "11.4.0");
    fake_toolchain(&second, "gcc", "11.4.0");
    s.cxvm("").arg("link").arg(&first).assert().success();
    s.cxvm("")
        .arg("link")
        .arg(&second)
        .assert()
        .code(1)
        .stderr(predicate::str::contains(first.display().to_string()))
        .stderr(predicate::str::contains("--as"));
    s.cxvm("")
        .arg("link")
        .arg(&second)
        .args(["--as", "gcc@11.4.0-home"])
        .assert()
        .success();
}

#[test]
fn an_id_in_the_store_is_rejected() {
    let s = Sandbox::new();
    s.install("gcc", "11.4.0-local");
    let prefix = s.path("opt/gcc-11");
    fake_toolchain(&prefix, "gcc", "11.4.0");
    s.cxvm("")
        .arg("link")
        .arg(&prefix)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("store"))
        .stderr(predicate::str::contains("--as"));
}

#[test]
fn unlink_forgets_the_toolchain_but_keeps_its_files() {
    let s = Sandbox::new();
    let prefix = s.path("opt/gcc-11");
    fake_toolchain(&prefix, "gcc", "11.4.0");
    s.cxvm("").arg("link").arg(&prefix).assert().success();
    s.cxvm("")
        .args(["unlink", "gcc@11.4.0-local"])
        .assert()
        .success()
        .stdout(predicate::str::contains("gcc@11.4.0-local"));
    s.cxvm("")
        .arg("list")
        .assert()
        .stdout(predicate::str::contains("gcc@11.4.0-local").not());
    assert!(prefix.join("bin/gcc").exists());
}

#[test]
fn unlink_warns_when_the_toolchain_is_in_use() {
    let s = Sandbox::new();
    let prefix = s.path("opt/gcc-11");
    fake_toolchain(&prefix, "gcc", "11.4.0");
    s.cxvm("").arg("link").arg(&prefix).assert().success();
    s.global("gcc@11.4.0-local");
    let rc = s.pin("", "gcc@11.4.0-local");
    s.cxvm("a")
        .args(["unlink", "gcc@11.4.0-local"])
        .assert()
        .success()
        .stderr(predicate::str::contains("global default"))
        .stderr(predicate::str::contains(rc.display().to_string()));
}

#[test]
fn unlink_rejects_ids_that_are_not_linked() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    for (id, expected) in [
        ("gcc@13.3.0", "cxvm remove"),
        ("gcc@11.4.0-local", "not linked"),
        ("gcc@11-local", "exact"),
    ] {
        s.cxvm("")
            .args(["unlink", id])
            .assert()
            .code(1)
            .stderr(predicate::str::contains(expected))
            .stderr(predicate::str::contains("hint:"));
    }
}
