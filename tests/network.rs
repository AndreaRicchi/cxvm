//! Tests against real upstream releases. They download hundreds of MB, so they are ignored by
//! default; run them with `cargo test --test network -- --ignored`.

mod common;

use common::Sandbox;
use predicates::prelude::*;

#[test]
#[ignore = "downloads clang 11.1.0 (0.4 GB) from GitHub"]
fn installs_runs_and_removes_a_real_clang() {
    let s = Sandbox::new();
    let host_path = std::env::var_os("PATH").unwrap_or_default();
    let path =
        std::env::join_paths(std::iter::once(s.bin()).chain(std::env::split_paths(&host_path)))
            .unwrap();
    let cxvm = |args: &[&str]| {
        let mut cmd = s.cxvm("");
        cmd.env("PATH", &path).args(args);
        cmd
    };
    cxvm(&["install", "clang@11.1.0"])
        .timeout(std::time::Duration::from_secs(1800))
        .assert()
        .success()
        .stdout(predicate::str::contains("Installed clang@11.1.0"));
    let clang = s.store().join("toolchains/clang/11.1.0/bin/clang++");
    let output = std::process::Command::new(&clang)
        .arg("--version")
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("clang version 11.1.0"),
        "{output:?}"
    );
    cxvm(&["remove", "clang@11.1.0"]).assert().success();
    assert!(!clang.exists());
}
