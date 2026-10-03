//! `cxvm install`: downloading, verifying and installing releases from a catalog, served by a
//! local HTTP server (default tests never touch the network).

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use common::{FakeGcc, Sandbox, Server, asset_name, fake_clang_archive, stdout, write_catalog};
use predicates::prelude::*;

/// A sandbox whose catalog (`catalog.toml`, passed as `CXVM_CATALOG`) lists fake release
/// archives served by a local HTTP server.
struct Fixture {
    sandbox: Sandbox,
    server: Server,
    catalog: PathBuf,
}

impl Fixture {
    /// Working fake LLVM archives for the clang `versions`.
    fn new(versions: &[&str]) -> Self {
        let archives: Vec<(&str, &str, Vec<u8>)> = versions
            .iter()
            .map(|v| ("clang", *v, fake_clang_archive(v, true)))
            .collect();
        Self::with_archives(&archives)
    }

    /// Working fake xPack archives for the GCC `versions`.
    fn gcc(versions: &[&str]) -> Self {
        let archives: Vec<(&str, &str, Vec<u8>)> = versions
            .iter()
            .map(|v| ("gcc", *v, FakeGcc::new(v).archive()))
            .collect();
        Self::with_archives(&archives)
    }

    /// One release per `(kind, version, archive)`.
    fn with_archives(archives: &[(&str, &str, Vec<u8>)]) -> Self {
        let sandbox = Sandbox::new();
        let server = Server::start();
        let catalog = sandbox.path("catalog.toml");
        let urls: Vec<String> = archives
            .iter()
            .map(|(kind, v, archive)| server.add(&asset_name(kind, v), archive.clone()))
            .collect();
        let entries: Vec<(&str, &str, &str, &[u8])> = archives
            .iter()
            .zip(&urls)
            .map(|((kind, v, archive), url)| (*kind, *v, url.as_str(), archive.as_slice()))
            .collect();
        write_catalog(&catalog, &entries);
        Self {
            sandbox,
            server,
            catalog,
        }
    }

    fn cxvm(&self, rel: &str) -> assert_cmd::Command {
        let mut cmd = self.sandbox.cxvm(rel);
        cmd.env("CXVM_CATALOG", &self.catalog);
        cmd
    }

    fn toolchain(&self, version: &str) -> PathBuf {
        self.sandbox.store().join("toolchains/clang").join(version)
    }

    fn gcc_toolchain(&self, version: &str) -> PathBuf {
        self.sandbox.store().join("toolchains/gcc").join(version)
    }

    /// Puts a fake `ldd` printing `output` on the sandbox's `PATH`.
    fn fake_ldd(&self, output: &str) {
        let ldd = self.sandbox.bin().join("ldd");
        fs::write(&ldd, format!("#!/bin/sh\nprintf '{output}'\n")).unwrap();
        fs::set_permissions(&ldd, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn downloads(&self) -> PathBuf {
        self.sandbox.path("cache/cxvm/downloads")
    }

    fn staging_is_empty(&self) -> bool {
        fs::read_dir(self.sandbox.store().join("staging")).map_or(true, |mut d| d.next().is_none())
    }
}

#[test]
fn installs_a_release_into_the_store() {
    let f = Fixture::new(&["19.1.0"]);
    let out = f
        .cxvm("")
        .args(["install", "clang@19.1.0"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("warning"), "{stderr}");
    let text = stdout(&out);
    assert!(text.contains("Installing clang@19.1.0"), "{text}");
    assert!(text.contains("Installed clang@19.1.0 in"), "{text}");
    assert!(text.contains("cxvm use clang@19.1.0"), "{text}");

    let prefix = f.toolchain("19.1.0");
    assert!(prefix.join("bin/clang").is_file());
    assert_eq!(
        fs::read_link(prefix.join("bin/clang++")).unwrap(),
        PathBuf::from("clang")
    );
    let meta = fs::read_to_string(prefix.join(".cxvm-meta.toml")).unwrap();
    assert!(meta.contains("id = \"clang@19.1.0\""), "{meta}");
    assert!(meta.contains("backend = \"llvm-prebuilt\""), "{meta}");
    assert!(
        meta.contains("version = \"clang version 19.1.0 (fake)\""),
        "{meta}"
    );
    assert!(
        meta.contains(&format!("cc = \"{}\"", prefix.join("bin/clang").display())),
        "{meta}"
    );
    assert!(f.downloads().join("LLVM-19.1.0-Linux-X64.tar.xz").is_file());
    assert!(f.staging_is_empty());

    f.cxvm("")
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("clang@19.1.0"));
}

#[test]
fn partial_ids_install_the_highest_known_release() {
    let f = Fixture::new(&["19.1.0", "19.1.7", "18.1.8"]);
    f.cxvm("")
        .args(["install", "clang@19"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Resolved clang@19 to clang@19.1.7",
        ));
    assert!(f.toolchain("19.1.7").is_dir());
    assert!(!f.toolchain("19.1.0").exists());
}

#[test]
fn installing_twice_is_a_no_op() {
    let f = Fixture::new(&["19.1.0"]);
    f.cxvm("")
        .args(["install", "clang@19.1.0"])
        .assert()
        .success();
    let requests = f.server.requests().len();
    f.cxvm("")
        .args(["install", "clang@19.1.0"])
        .assert()
        .success()
        .stdout(predicate::str::contains("already installed"));
    assert_eq!(f.server.requests().len(), requests);
}

#[test]
fn a_cached_archive_is_not_downloaded_again() {
    let f = Fixture::new(&["19.1.0"]);
    let archive = fake_clang_archive("19.1.0", true);
    fs::create_dir_all(f.downloads()).unwrap();
    fs::write(f.downloads().join("LLVM-19.1.0-Linux-X64.tar.xz"), &archive).unwrap();
    f.cxvm("")
        .args(["install", "clang@19.1.0"])
        .assert()
        .success();
    assert!(f.server.requests().is_empty(), "{:?}", f.server.requests());
}

#[test]
fn a_checksum_mismatch_installs_nothing_and_deletes_the_download() {
    let f = Fixture::new(&["19.1.0"]);
    let good = fake_clang_archive("19.1.0", true);
    let mut tampered = good.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 0xff;
    f.server.add("LLVM-19.1.0-Linux-X64.tar.xz", tampered);
    f.cxvm("")
        .args(["install", "clang@19.1.0"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("sha256 mismatch"))
        .stderr(predicate::str::contains("hint:"));
    assert!(!f.toolchain("19.1.0").exists());
    assert_eq!(fs::read_dir(f.downloads()).unwrap().count(), 0);
}

#[test]
fn an_interrupted_download_leaves_no_toolchain_and_resumes() {
    let f = Fixture::new(&["19.1.0"]);
    let size = fake_clang_archive("19.1.0", true).len();
    f.server.truncate_next_response(size / 2);
    f.cxvm("")
        .args(["install", "clang@19.1.0"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("resumes where it stopped"));
    assert!(!f.toolchain("19.1.0").exists());
    let part = f.downloads().join("LLVM-19.1.0-Linux-X64.tar.xz.part");
    assert_eq!(fs::metadata(&part).unwrap().len() as usize, size / 2);
    f.cxvm("")
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("No toolchains"));

    f.cxvm("")
        .args(["install", "clang@19.1.0"])
        .assert()
        .success();
    assert!(f.toolchain("19.1.0").join("bin/clang").is_file());
    let requests = f.server.requests();
    assert_eq!(
        requests.last().unwrap(),
        &format!("/LLVM-19.1.0-Linux-X64.tar.xz bytes={}-", size / 2)
    );
    assert!(!part.exists());
}

#[test]
fn leftovers_of_an_interrupted_install_are_cleaned_up() {
    let f = Fixture::new(&["19.1.0"]);
    let leftover = f.sandbox.store().join("staging/clang-19.1.0.4242/bin");
    fs::create_dir_all(&leftover).unwrap();
    f.cxvm("")
        .args(["install", "clang@19.1.0"])
        .assert()
        .success()
        .stderr(predicate::str::contains("left by an interrupted install"));
    assert!(f.staging_is_empty());
}

#[test]
fn a_compiler_that_cannot_build_cxx_is_installed_with_a_warning() {
    let f = Fixture::with_archives(&[("clang", "19.1.0", fake_clang_archive("19.1.0", false))]);
    f.cxvm("")
        .args(["install", "clang@19.1.0"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "cannot build and run a C++ program",
        ))
        .stderr(predicate::str::contains("iostream: file not found"))
        .stderr(predicate::str::contains("install g++"));
    assert!(f.toolchain("19.1.0").is_dir());
    assert!(f.staging_is_empty());
}

#[test]
fn missing_shared_libraries_abort_the_install() {
    let f = Fixture::new(&["18.1.8"]);
    f.fake_ldd("\\tlibtinfo.so.5 => not found\\n\\tlibc.so.6 => /lib/libc.so.6\\n");
    f.cxvm("")
        .args(["install", "clang@18.1.8"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "missing shared libraries libtinfo.so.5",
        ))
        .stderr(predicate::str::contains("libtinfo5"));
    assert!(!f.toolchain("18.1.8").exists());
    assert!(f.staging_is_empty());
    // The verified archive stays for the next attempt.
    assert!(f.downloads().join("LLVM-18.1.8-Linux-X64.tar.xz").is_file());
}

#[test]
fn an_archive_holding_another_version_is_refused() {
    let f = Fixture::with_archives(&[("clang", "19.1.0", fake_clang_archive("19.1.1", true))]);
    f.cxvm("")
        .args(["install", "clang@19.1.0"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("the archive holds clang@19.1.1"));
    assert!(!f.toolchain("19.1.0").exists());
    assert!(f.staging_is_empty());
}

#[test]
fn unknown_releases_list_what_the_major_offers() {
    let f = Fixture::new(&["19.1.0", "19.1.7"]);
    f.cxvm("")
        .args(["install", "clang@19.1.5"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("clang@19.1.5 is not in"))
        .stderr(predicate::str::contains(
            "available: clang@19.1.0, clang@19.1.7",
        ));
    f.cxvm("")
        .args(["install", "clang@30"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("cxvm list --available"));
}

#[test]
fn refuses_what_it_cannot_install() {
    let f = Fixture::new(&["19.1.0"]);
    for (args, expected) in [
        (
            &["install", "clang@19.1.0", "--from-source"][..],
            "planned for milestone M5",
        ),
        (&["install", "--from-source"], "planned for milestone M5"),
        (
            &["install", "clang@19.1.0-local"],
            "cxvm link <prefix> --as clang@19.1.0-local",
        ),
        (&["install", "system"], "does not install it"),
    ] {
        f.cxvm("")
            .args(args)
            .assert()
            .code(1)
            .stderr(predicate::str::contains(expected));
    }
    assert!(f.server.requests().is_empty());
}

#[test]
fn install_without_an_id_downloads_the_pinned_toolchain() {
    let f = Fixture::new(&["19.1.0"]);
    f.sandbox.pin("", "clang@19.1.0");
    f.cxvm("a/b")
        .arg("install")
        .assert()
        .success()
        .stdout(predicate::str::contains("Installed clang@19.1.0"))
        .stdout(predicate::str::contains("Generated"));
    let project = f.sandbox.project();
    assert_eq!(
        fs::read_link(project.join(".cxvm/current")).unwrap(),
        f.toolchain("19.1.0")
    );
    let cmake = fs::read_to_string(project.join(".cxvm/toolchain.cmake")).unwrap();
    assert!(
        cmake.contains(
            &f.toolchain("19.1.0")
                .join("bin/clang++")
                .display()
                .to_string()
        )
    );
}

#[test]
fn install_without_an_id_still_fails_for_what_the_catalog_lacks() {
    let f = Fixture::new(&["19.1.0"]);
    f.sandbox.pin("", "clang@17.0.6");
    f.cxvm("")
        .arg("install")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("clang@17.0.6 is not installed"));
}

#[test]
fn installs_a_gcc_release_and_records_its_runtime_dirs() {
    let f = Fixture::gcc(&["13.2.0", "13.3.0"]);
    let out = f.cxvm("").args(["install", "gcc@13"]).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(!stderr.contains("warning"), "{stderr}");
    let text = stdout(&out);
    assert!(text.contains("Resolved gcc@13 to gcc@13.3.0"), "{text}");
    assert!(text.contains("Installed gcc@13.3.0 in"), "{text}");

    let prefix = f.gcc_toolchain("13.3.0");
    assert!(prefix.join("bin/g++").is_file());
    assert!(!prefix.join("xpack-gcc-13.3.0-1").exists());
    let meta = fs::read_to_string(prefix.join(".cxvm-meta.toml")).unwrap();
    for line in [
        "id = \"gcc@13.3.0\"".to_owned(),
        "backend = \"xpack-prebuilt\"".to_owned(),
        "version = \"gcc (fake xPack GCC) 13.3.0\"".to_owned(),
        format!("cxx = \"{}\"", prefix.join("bin/g++").display()),
        format!("libstdcxx_dir = \"{}\"", prefix.join("lib64").display()),
        format!(
            "gcc_install_dir = \"{}\"",
            prefix.join("lib/gcc/x86_64-pc-linux-gnu/13.3.0").display()
        ),
    ] {
        assert!(meta.contains(&line), "{line} not in:\n{meta}");
    }
    assert!(!meta.contains("staging"), "{meta}");
    assert!(
        f.downloads()
            .join("xpack-gcc-13.3.0-1-linux-x64.tar.gz")
            .is_file()
    );
    assert!(f.staging_is_empty());

    f.cxvm("").args(["use", "gcc@13"]).assert().success();
    let cmake = fs::read_to_string(f.sandbox.project().join(".cxvm/toolchain.cmake")).unwrap();
    let rpath = format!(
        "\"-Wl,-rpath,{}\"",
        prefix.join("lib64").canonicalize().unwrap().display()
    );
    assert!(cmake.contains(&rpath), "{rpath} not in:\n{cmake}");
}

#[test]
fn install_without_an_id_downloads_a_pinned_gcc() {
    let f = Fixture::gcc(&["13.3.0"]);
    f.sandbox.pin("", "gcc@13.3.0");
    f.cxvm("a")
        .arg("install")
        .assert()
        .success()
        .stdout(predicate::str::contains("Installed gcc@13.3.0"));
    assert_eq!(
        fs::read_link(f.sandbox.project().join(".cxvm/current")).unwrap(),
        f.gcc_toolchain("13.3.0")
    );
}

#[test]
fn a_gcc_that_cannot_link_is_installed_with_a_warning() {
    let gcc = FakeGcc {
        compiles: false,
        ..FakeGcc::new("11.2.0")
    };
    let f = Fixture::with_archives(&[("gcc", "11.2.0", gcc.archive())]);
    f.cxvm("")
        .args(["install", "gcc@11.2.0"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "cannot build and run a C++ program",
        ))
        .stderr(predicate::str::contains(".relr.dyn"))
        .stderr(predicate::str::contains("libc6-dev"))
        .stderr(predicate::str::contains("-B/usr/bin"));
    assert!(f.gcc_toolchain("11.2.0").is_dir());
    assert!(f.staging_is_empty());
}

#[test]
fn a_program_loading_another_libstdcxx_is_reported() {
    let f = Fixture::gcc(&["13.3.0"]);
    f.fake_ldd(
        "\\tlibstdc++.so.6 => /usr/lib/x86_64-linux-gnu/libstdc++.so.6 (0x00007f0000000000)\\n",
    );
    f.cxvm("")
        .args(["install", "gcc@13.3.0"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "loads /usr/lib/x86_64-linux-gnu/libstdc++.so.6 instead of the libstdc++.so.6 in",
        ));
    assert!(f.gcc_toolchain("13.3.0").is_dir());
}

#[test]
fn a_gcc_without_its_own_libstdcxx_is_refused() {
    let gcc = FakeGcc {
        libstdcxx: false,
        ..FakeGcc::new("13.3.0")
    };
    let f = Fixture::with_archives(&[("gcc", "13.3.0", gcc.archive())]);
    f.cxvm("")
        .args(["install", "gcc@13.3.0"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "its g++ does not report a libstdc++.so.6",
        ))
        .stderr(predicate::str::contains("hint:"));
    assert!(!f.gcc_toolchain("13.3.0").exists());
    assert!(f.staging_is_empty());
}

#[test]
fn both_kinds_install_from_one_catalog() {
    let f = Fixture::with_archives(&[
        ("clang", "19.1.0", fake_clang_archive("19.1.0", true)),
        ("gcc", "13.3.0", FakeGcc::new("13.3.0").archive()),
    ]);
    let out = f.cxvm("").args(["list", "--available"]).output().unwrap();
    let text = stdout(&out);
    assert!(text.contains("gcc@13.3.0"), "{text}");
    assert!(text.contains("clang@19.1.0"), "{text}");
    for id in ["gcc@13.3.0", "clang@19.1.0"] {
        f.cxvm("").args(["install", id]).assert().success();
    }
    let out = f.cxvm("").arg("list").output().unwrap();
    let text = stdout(&out);
    assert!(text.contains("gcc@13.3.0"), "{text}");
    assert!(text.contains("clang@19.1.0"), "{text}");
}
