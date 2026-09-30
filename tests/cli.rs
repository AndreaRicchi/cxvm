//! End-to-end checks of the CLI surface: every §5 command exists, has help text, and the
//! stubs fail loudly instead of pretending to succeed.

mod common;

use assert_cmd::Command;
use common::Sandbox;
use predicates::prelude::*;

/// Every command from PLAN.md §5, in table order.
const COMMANDS: &[&str] = &[
    "install",
    "remove",
    "list",
    "use",
    "global",
    "current",
    "exec",
    "env",
    "link",
    "unlink",
    "doctor",
    "completions",
];

fn cxvm() -> Command {
    Command::cargo_bin("cxvm").expect("cxvm binary built by cargo")
}

#[test]
fn help_lists_every_command() {
    let assert = cxvm().arg("--help").assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    for name in COMMANDS {
        let listed = stdout.lines().any(|line| {
            let line = line.trim_start();
            line.starts_with(name) && line[name.len()..].starts_with(' ')
        });
        assert!(listed, "`{name}` missing from --help output:\n{stdout}");
    }
}

#[test]
fn every_command_has_help() {
    for name in COMMANDS {
        cxvm()
            .args([name, "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Usage:"));
    }
}

#[test]
fn version_flag_works() {
    cxvm()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("cxvm "));
}

#[test]
fn stubs_fail_with_not_implemented() {
    let invocations: &[&[&str]] = &[
        &["install"],
        &["install", "gcc@13", "--from-source"],
        &["remove", "gcc@13.3.0", "--force"],
        &["list", "--available", "--json"],
        &["use", "gcc@13"],
        &["global", "clang@18"],
        &["exec", "--", "make", "-j4"],
        &["env"],
        &["env", "--shell", "fish"],
        &["doctor"],
    ];
    // In a sandbox: a stub that becomes a real command must never touch the real store.
    let sandbox = Sandbox::new();
    for args in invocations {
        sandbox
            .cxvm("")
            .args(*args)
            .assert()
            .code(1)
            .stdout(predicate::str::is_empty())
            .stderr(predicate::str::contains("not implemented yet"))
            .stderr(predicate::str::contains(args[0]));
    }
}

#[test]
fn exec_requires_a_command() {
    cxvm()
        .arg("exec")
        .assert()
        .failure()
        .stderr(predicate::str::contains("required"));
}

#[test]
fn completions_print_a_script() {
    for shell in ["bash", "zsh", "fish"] {
        cxvm()
            .args(["completions", shell])
            .assert()
            .success()
            .stdout(predicate::str::contains("cxvm"))
            .stdout(predicate::str::contains("doctor"));
    }
}
