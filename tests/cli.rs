//! End-to-end checks of the CLI surface: every command exists and has help text, and messages
//! follow `-v` and the colour settings.

mod common;

use assert_cmd::Command;
use common::Sandbox;
use predicates::prelude::*;

/// Every command, in the order of the README's command table.
const COMMANDS: &[&str] = &[
    "install",
    "remove",
    "prune",
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

#[test]
fn labels_are_coloured_only_when_asked_and_never_with_no_color() {
    let sandbox = Sandbox::new();
    let run = |vars: &[(&str, &str)]| {
        let mut cmd = sandbox.cxvm("");
        cmd.args(["use", "gcc13"]);
        for (name, value) in vars {
            cmd.env(name, value);
        }
        let output = cmd.assert().code(1).get_output().clone();
        String::from_utf8(output.stderr).unwrap()
    };
    let escape = "\u{1b}[";

    let piped = run(&[]);
    assert!(piped.starts_with("error: invalid toolchain id"), "{piped}");
    assert!(piped.contains("\n  hint: "), "{piped}");
    assert!(!piped.contains(escape), "{piped:?}");

    let forced = run(&[("CLICOLOR_FORCE", "1")]);
    assert!(forced.contains(escape), "{forced:?}");
    assert_eq!(anstream::adapter::strip_str(&forced).to_string(), piped);

    let no_color = run(&[("CLICOLOR_FORCE", "1"), ("NO_COLOR", "1")]);
    assert_eq!(no_color, piped);
}

#[test]
fn verbose_says_how_the_toolchain_was_found() {
    let sandbox = Sandbox::new();
    sandbox.install("gcc", "13.3.0");
    let rc = sandbox.pin("", "gcc@13.3.0");
    sandbox
        .cxvm("a/b")
        .arg("current")
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    sandbox
        .cxvm("a/b")
        .args(["-v", "current"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("gcc@13.3.0 (set by"))
        .stderr(predicate::str::contains(format!(
            "debug: {} pins gcc@13.3.0",
            rc.display()
        )))
        .stderr(predicate::str::contains("debug: store "));
    sandbox
        .cxvm("")
        .args(["exec", "--verbose", "--", "true"])
        .env("PATH", sandbox.path_with_host())
        .assert()
        .success()
        .stderr(predicate::str::contains("debug: running true with "))
        .stderr(predicate::str::contains("CXVM_TOOLCHAIN=gcc@13.3.0"));
}
