//! `cxvm env` and `cxvm exec`: running things with the active toolchain.

mod common;

use common::{Sandbox, stdout};
use predicates::prelude::*;

/// The sandbox `PATH` plus the system dirs, so `sh` resolves.
fn with_system_path(s: &Sandbox) -> std::ffi::OsString {
    std::env::join_paths([s.bin(), "/usr/bin".into(), "/bin".into()]).unwrap()
}

#[test]
fn env_prints_posix_exports_by_default() {
    let s = Sandbox::new();
    let prefix = s.install("gcc", "13.3.0");
    s.pin("", "gcc@13");
    s.cxvm("a").arg("env").assert().success().stdout(format!(
        "export CXVM_TOOLCHAIN='gcc@13.3.0'\n\
         export PATH='{bin}':\"$PATH\"\n\
         export CC='{bin}/gcc'\n\
         export CXX='{bin}/g++'\n",
        bin = prefix.join("bin").display()
    ));
}

#[test]
fn env_follows_the_login_shell_unless_told_otherwise() {
    let s = Sandbox::new();
    s.install("clang", "18.1.8");
    s.global("clang@18.1.8");
    s.cxvm("")
        .arg("env")
        .env("SHELL", "/usr/bin/fish")
        .assert()
        .success()
        .stdout(predicate::str::starts_with(
            "set -gx CXVM_TOOLCHAIN 'clang@18.1.8'\n",
        ));
    s.cxvm("")
        .args(["env", "--shell", "zsh"])
        .env("SHELL", "/usr/bin/fish")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("export CXVM_TOOLCHAIN="));
    s.cxvm("")
        .args(["env", "--shell", "fish"])
        .env("SHELL", "/bin/bash")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("set -gx "));
}

#[test]
fn env_for_system_leaves_path_alone() {
    let s = Sandbox::new();
    s.cxvm("").arg("env").assert().success().stdout(format!(
        "export CXVM_TOOLCHAIN='system'\nexport CC='{bin}/cc'\nexport CXX='{bin}/c++'\n",
        bin = s.bin().display()
    ));
}

#[test]
fn env_prints_nothing_when_the_toolchain_is_missing() {
    let s = Sandbox::new();
    s.pin("", "gcc@15.1.0");
    s.cxvm("")
        .arg("env")
        .assert()
        .code(1)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("gcc@15.1.0 is not installed"));
}

#[test]
fn exec_runs_with_the_toolchain_environment() {
    let s = Sandbox::new();
    let prefix = s.install("gcc", "13.3.0");
    s.pin("", "gcc@13.3.0");
    let out = s
        .cxvm("a/b")
        .env("PATH", with_system_path(&s))
        .args([
            "exec",
            "--",
            "sh",
            "-c",
            "echo \"$CXVM_TOOLCHAIN|$CC|$CXX|$PATH\"",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let bin = prefix.join("bin");
    assert_eq!(
        stdout(&out),
        format!(
            "gcc@13.3.0|{}|{}|{}:{}:/usr/bin:/bin\n",
            bin.join("gcc").display(),
            bin.join("g++").display(),
            bin.display(),
            s.bin().display()
        )
    );
}

#[test]
fn exec_finds_the_command_on_the_new_path() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    s.pin("", "gcc@13.3.0");
    s.cxvm("")
        .args(["exec", "--", "gcc", "--version"])
        .assert()
        .success()
        .stdout(predicate::str::contains("(Fake) 13.3.0"));
}

#[test]
fn exec_passes_the_exit_status_through() {
    let s = Sandbox::new();
    s.cxvm("")
        .env("PATH", with_system_path(&s))
        .args(["exec", "--", "sh", "-c", "exit 7"])
        .assert()
        .code(7);
}

#[test]
fn exec_with_system_leaves_path_alone() {
    let s = Sandbox::new();
    let path = with_system_path(&s);
    s.cxvm("")
        .env("PATH", &path)
        .args(["exec", "--", "sh", "-c", "echo \"$CC|$PATH\""])
        .assert()
        .success()
        .stdout(format!(
            "{}|{}\n",
            s.bin().join("cc").display(),
            path.to_str().unwrap()
        ));
}

#[test]
fn exec_refuses_a_missing_toolchain_without_running_anything() {
    let s = Sandbox::new();
    s.pin("", "gcc@15.1.0");
    let marker = s.path("ran");
    s.cxvm("")
        .env("PATH", with_system_path(&s))
        .args(["exec", "--", "touch"])
        .arg(&marker)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("gcc@15.1.0 is not installed"));
    assert!(!marker.exists());
}

#[test]
fn exec_of_an_unknown_command_is_an_error_naming_it() {
    let s = Sandbox::new();
    s.cxvm("")
        .args(["exec", "--", "no-such-command-cxvm"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("no-such-command-cxvm"))
        .stderr(predicate::str::contains("hint:"));
}
