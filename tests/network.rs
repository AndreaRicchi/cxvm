//! Tests against real upstream releases. They download hundreds of MB, and one builds GCC, so
//! they are ignored by default; run them with `cargo test --test network -- --ignored`.

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

/// Checks the GCC `version` that `cxvm(args)` just installed in `s`: its metadata names its own
/// `lib64` and install dir, `cxvm use` writes that `lib64` as the rpath, and a program built
/// with it runs without `LD_LIBRARY_PATH` and loads the toolchain's `libstdc++.so.6`.
fn assert_builds_programs_loading_its_libstdcxx(
    s: &Sandbox,
    cxvm: impl Fn(&[&str]) -> assert_cmd::Command,
    version: &str,
) {
    let id = format!("gcc@{version}");
    let prefix = s.store().join("toolchains/gcc").join(version);
    let lib64 = prefix.join("lib64");
    let meta = std::fs::read_to_string(prefix.join(".cxvm-meta.toml")).unwrap();
    assert!(
        meta.contains(&format!("libstdcxx_dir = \"{}\"", lib64.display())),
        "{meta}"
    );
    assert!(
        meta.contains(&format!("lib/gcc/x86_64-pc-linux-gnu/{version}\"")),
        "{meta}"
    );

    cxvm(&["use", &id]).assert().success();
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
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), version);
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

    cxvm(&["remove", &id]).assert().success();
    assert!(!prefix.exists());
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
    assert_builds_programs_loading_its_libstdcxx(&s, cxvm, "15.2.0");
}

#[test]
#[ignore = "downloads the sources of GCC 16.2.0 (0.1 GB) and builds them with the system \
            compiler: about 10 minutes on 22 cores"]
fn a_real_gcc_builds_from_source() {
    let s = Sandbox::new();
    // The host's `PATH` only: the build needs the real `cc`, not the sandbox's fake one.
    let cxvm = |args: &[&str]| {
        let mut cmd = cxvm(&s, args);
        cmd.env("PATH", std::env::var_os("PATH").unwrap_or_default());
        cmd
    };
    let out = cxvm(&["install", "gcc@16.2.0", "--from-source"])
        .timeout(std::time::Duration::from_secs(4 * 3600))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(!stderr.contains("warning"), "{stderr}");
    let meta =
        std::fs::read_to_string(s.store().join("toolchains/gcc/16.2.0/.cxvm-meta.toml")).unwrap();
    assert!(meta.contains("backend = \"gnu-source\""), "{meta}");
    assert!(meta.contains("\"--disable-bootstrap\""), "{meta}");
    assert_builds_programs_loading_its_libstdcxx(&s, cxvm, "16.2.0");
}
