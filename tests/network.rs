//! Tests against real upstream releases. They download hundreds of MB, so they are ignored by
//! default; run them with `cargo test --test network -- --ignored`.

mod common;

use common::Sandbox;
use predicates::prelude::*;

/// `cxvm <args>` in `s` with the catalog built into cxvm and the host's `PATH` after the
/// sandbox's.
fn cxvm(s: &Sandbox, args: &[&str]) -> assert_cmd::Command {
    let host_path = std::env::var_os("PATH").unwrap_or_default();
    let path =
        std::env::join_paths(std::iter::once(s.bin()).chain(std::env::split_paths(&host_path)))
            .unwrap();
    let mut cmd = s.cxvm("");
    cmd.env("PATH", &path).env_remove("CXVM_CATALOG").args(args);
    cmd
}

#[test]
#[ignore = "downloads clang 11.1.0 (0.4 GB) from GitHub"]
fn installs_runs_and_removes_a_real_clang() {
    let s = Sandbox::new();
    let cxvm = |args: &[&str]| cxvm(&s, args);
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

#[test]
#[ignore = "downloads GCC 15.2.0 (165 MiB) from GitHub"]
fn a_real_gcc_builds_programs_that_load_its_libstdcxx() {
    let s = Sandbox::new();
    let cxvm = |args: &[&str]| cxvm(&s, args);
    let out = cxvm(&["install", "gcc@15.2.0"])
        .timeout(std::time::Duration::from_secs(1800))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    // The post-install check built a C++ program and found it loading this libstdc++.
    assert!(!stderr.contains("warning"), "{stderr}");

    let prefix = s.store().join("toolchains/gcc/15.2.0");
    let lib64 = prefix.join("lib64");
    let meta = std::fs::read_to_string(prefix.join(".cxvm-meta.toml")).unwrap();
    assert!(
        meta.contains(&format!("libstdcxx_dir = \"{}\"", lib64.display())),
        "{meta}"
    );
    assert!(
        meta.contains("lib/gcc/x86_64-pc-linux-gnu/15.2.0\""),
        "{meta}"
    );

    cxvm(&["use", "gcc@15.2.0"]).assert().success();
    let lib64 = lib64.canonicalize().unwrap();
    let rpath = format!("-Wl,-rpath,{}", lib64.display());
    let cmake = std::fs::read_to_string(s.project().join(".cxvm/toolchain.cmake")).unwrap();
    assert!(cmake.contains(&rpath), "{cmake}");

    let source = s.path("hello.cpp");
    let program = s.path("hello");
    std::fs::write(
        &source,
        "#include <iostream>\nint main() { std::cout << __VERSION__ << std::endl; }\n",
    )
    .unwrap();
    let build = std::process::Command::new(prefix.join("bin/g++"))
        .arg(&source)
        .arg("-o")
        .arg(&program)
        .arg(&rpath)
        .output()
        .unwrap();
    assert!(build.status.success(), "{build:?}");
    let run = std::process::Command::new(&program)
        .env_remove("LD_LIBRARY_PATH")
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "15.2.0");
    let ldd = std::process::Command::new("ldd")
        .arg(&program)
        .output()
        .unwrap();
    let ldd = String::from_utf8_lossy(&ldd.stdout);
    let expected = format!(
        "libstdc++.so.6 => {}",
        lib64.join("libstdc++.so.6").display()
    );
    assert!(ldd.contains(&expected), "{ldd}");

    cxvm(&["remove", "gcc@15.2.0"]).assert().success();
    assert!(!prefix.exists());
}
