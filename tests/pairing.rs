//! `[clang] gcc`: pairing a clang with the headers and libstdc++ of a chosen GCC.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{FakeGcc, Sandbox, Server, asset_name, stdout, write_catalog};
use predicates::prelude::*;

fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path).unwrap()
}

/// Installs a fake GCC laid out like a real one and returns its canonical prefix, which is
/// what the drivers' answers resolve to.
fn gcc(s: &Sandbox, version: &str) -> PathBuf {
    let prefix = s.store().join("toolchains/gcc").join(version);
    FakeGcc::new(version).install_into(&prefix);
    prefix.canonicalize().unwrap()
}

fn install_dir(prefix: &Path, version: &str) -> String {
    prefix
        .join("lib/gcc/x86_64-pc-linux-gnu")
        .join(version)
        .display()
        .to_string()
}

#[test]
fn use_with_gcc_pairs_the_clang_in_every_generated_file() {
    let s = Sandbox::new();
    let clang = s.install("clang", "17.0.6");
    let gcc = gcc(&s, "13.3.0");
    s.install("gcc", "12.4.0");
    let flag = format!("--gcc-install-dir={}", install_dir(&gcc, "13.3.0"));

    s.cxvm("")
        .args(["use", "clang@17", "--gcc", "gcc@13"])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "Clang uses the headers and libstdc++ of gcc@13.3.0 ({flag})"
        )));

    let root = s.project();
    assert_eq!(
        read(root.join(".cxvmrc")),
        "compiler = \"clang@17.0.6\"\n\n[clang]\ngcc = \"gcc@13.3.0\"\n"
    );
    let cmake = read(root.join(".cxvm/toolchain.cmake"));
    for (variable, driver) in [
        ("CMAKE_C_COMPILER  ", "clang"),
        ("CMAKE_CXX_COMPILER", "clang++"),
    ] {
        let line = format!(
            "set({variable} \"{}\" \"{flag}\")\n",
            clang.join("bin").join(driver).display()
        );
        assert!(cmake.contains(&line), "{cmake}");
    }
    let rpath = format!(
        "set(CMAKE_EXE_LINKER_FLAGS_INIT    \"-Wl,-rpath,{}\")\n",
        gcc.join("lib64").display()
    );
    assert!(cmake.contains(&rpath), "{cmake}");
    assert!(cmake.contains("libstdc++ of gcc@13.3.0"), "{cmake}");

    let env = read(root.join(".cxvm/env"));
    let cxx = format!(
        "export CXX='{} {flag}'\n",
        clang.join("bin/clang++").display()
    );
    assert!(env.contains(&cxx), "{env}");
}

#[test]
fn a_clang_before_16_is_given_the_prefix() {
    let s = Sandbox::new();
    s.install("clang", "15.0.6");
    let gcc = gcc(&s, "13.3.0");
    s.cxvm("")
        .args(["use", "clang@15.0.6", "--gcc", "gcc@13.3.0"])
        .assert()
        .success();
    let cmake = read(s.project().join(".cxvm/toolchain.cmake"));
    assert!(
        cmake.contains(&format!("\"--gcc-toolchain={}\")\n", gcc.display())),
        "{cmake}"
    );
    assert!(!cmake.contains("--gcc-install-dir"), "{cmake}");
}

#[test]
fn a_clang_before_16_cannot_choose_among_the_gccs_of_one_prefix() {
    let s = Sandbox::new();
    s.install("clang", "15.0.6");
    let gcc = gcc(&s, "13.3.0");
    let other = gcc.join("lib/gcc/x86_64-pc-linux-gnu/14");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("crtbegin.o"), "").unwrap();

    s.cxvm("")
        .args(["use", "clang@15.0.6", "--gcc", "gcc@13.3.0"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "cannot pair clang@15.0.6 with gcc@13.3.0",
        ))
        .stderr(predicate::str::contains("other GCCs too (14)"))
        .stderr(predicate::str::contains("hint:"));
    assert!(!s.project().join(".cxvmrc").exists());
    assert!(!s.project().join(".cxvm").exists());

    // A clang with `--gcc-install-dir` names the directory itself.
    s.install("clang", "16.0.4");
    s.cxvm("")
        .args(["use", "clang@16", "--gcc", "gcc@13.3.0"])
        .assert()
        .success();
}

#[test]
fn a_gcc_that_does_not_name_its_install_dir_cannot_be_paired() {
    let s = Sandbox::new();
    s.install("clang", "17.0.6");
    s.install("gcc", "13.3.0");
    s.cxvm("")
        .args(["use", "clang@17.0.6", "--gcc", "gcc@13.3.0"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("-print-libgcc-file-name"))
        .stderr(predicate::str::contains("hint:"));
    assert!(!s.project().join(".cxvmrc").exists());
}

#[test]
fn gcc_option_is_refused_for_anything_but_a_clang_and_a_gcc() {
    let s = Sandbox::new();
    s.install("clang", "17.0.6");
    gcc(&s, "13.3.0");
    for (args, needle) in [
        (
            ["use", "gcc@13.3.0", "--gcc", "gcc@13.3.0"],
            "gcc@13.3.0 is not a clang",
        ),
        (
            ["use", "system", "--gcc", "gcc@13.3.0"],
            "system is not a clang",
        ),
        (
            ["use", "clang@17", "--gcc", "clang@17"],
            "does not name a GCC",
        ),
        (
            ["use", "clang@17", "--gcc", "gcc13"],
            "invalid toolchain id",
        ),
        (
            ["use", "clang@17", "--gcc", "gcc@12"],
            "gcc@12 is not installed (--gcc)",
        ),
    ] {
        s.cxvm("")
            .args(args)
            .assert()
            .code(1)
            .stderr(predicate::str::contains(needle))
            .stderr(predicate::str::contains("hint:"));
        assert!(!s.project().join(".cxvmrc").exists(), "{args:?}");
    }
}

#[test]
fn the_pairing_stays_until_it_is_changed() {
    let s = Sandbox::new();
    s.install("clang", "17.0.6");
    s.install("clang", "19.1.0");
    let gcc = gcc(&s, "13.3.0");
    let rc = s.project().join(".cxvmrc");
    let cmake = s.project().join(".cxvm/toolchain.cmake");
    let flag = format!("--gcc-install-dir={}", install_dir(&gcc, "13.3.0"));

    s.cxvm("")
        .args(["use", "clang@17", "--gcc", "gcc@13"])
        .assert()
        .success();
    // Another clang keeps it.
    s.cxvm("").args(["use", "clang@19"]).assert().success();
    assert_eq!(
        read(&rc),
        "compiler = \"clang@19.1.0\"\n\n[clang]\ngcc = \"gcc@13.3.0\"\n"
    );
    assert!(read(&cmake).contains(&flag));
    // A GCC ignores it, without forgetting it.
    s.cxvm("").args(["use", "gcc@13"]).assert().success();
    assert!(read(&rc).contains("[clang]\ngcc = \"gcc@13.3.0\"\n"));
    assert!(!read(&cmake).contains("--gcc-install-dir"));
    assert!(!read(&cmake).contains("Clang uses"));
    // `system` hands the choice back to clang.
    s.cxvm("")
        .args(["use", "clang@19", "--gcc", "system"])
        .assert()
        .success();
    assert_eq!(
        read(&rc),
        "compiler = \"clang@19.1.0\"\n\n[clang]\ngcc = \"system\"\n"
    );
    let text = read(&cmake);
    assert!(!text.contains("--gcc-install-dir"), "{text}");
    assert!(!text.contains("rpath"), "{text}");
}

#[test]
fn a_hand_written_pairing_is_applied_by_install() {
    let s = Sandbox::new();
    s.install("clang", "17.0.6");
    gcc(&s, "13.2.0");
    let newest = gcc(&s, "13.3.0");
    fs::write(
        s.project().join(".cxvmrc"),
        "compiler = \"clang@17.0.6\"\n\n[clang]\n# what the target ships\ngcc = \"gcc@13\"\n",
    )
    .unwrap();
    s.cxvm("a")
        .arg("install")
        .assert()
        .success()
        .stdout(predicate::str::contains("clang@17.0.6 is available"))
        .stdout(predicate::str::contains("gcc@13.3.0 is available"));
    let cmake = read(s.project().join(".cxvm/toolchain.cmake"));
    assert!(
        cmake.contains(&format!(
            "--gcc-install-dir={}",
            install_dir(&newest, "13.3.0")
        )),
        "{cmake}"
    );
    // `install` never rewrites the pin file.
    assert!(
        read(s.project().join(".cxvmrc")).contains("# what the target ships\ngcc = \"gcc@13\"")
    );
}

#[test]
fn a_pairing_that_is_not_installed_stops_use_and_install() {
    let s = Sandbox::new();
    s.install("clang", "17.0.6");
    let rc = s.project().join(".cxvmrc");
    let content = "compiler = \"clang@17.0.6\"\n\n[clang]\ngcc = \"gcc@13.3.0\"\n";
    fs::write(&rc, content).unwrap();
    for args in [vec!["install"], vec!["use", "clang@17"], vec!["env"]] {
        s.cxvm("")
            .args(&args)
            .assert()
            .code(1)
            .stderr(predicate::str::contains(format!(
                "gcc@13.3.0 is not installed (`[clang] gcc` in {})",
                rc.display()
            )))
            .stderr(predicate::str::contains("cxvm install gcc@13.3.0"));
        assert!(!s.project().join(".cxvm").exists(), "{args:?}");
    }
    assert_eq!(read(&rc), content);
}

#[test]
fn install_downloads_a_missing_paired_gcc() {
    let s = Sandbox::new();
    let server = Server::start();
    let archive = FakeGcc::new("13.3.0").archive();
    let url = server.add(&asset_name("gcc", "13.3.0"), archive.clone());
    let catalog = s.path("catalog.toml");
    write_catalog(
        &catalog,
        &[("gcc", "13.3.0", url.as_str(), archive.as_slice())],
    );
    s.install("clang", "17.0.6");
    fs::write(
        s.project().join(".cxvmrc"),
        "compiler = \"clang@17.0.6\"\n\n[clang]\ngcc = \"gcc@13.3.0\"\n",
    )
    .unwrap();

    s.cxvm("")
        .arg("install")
        .env("CXVM_CATALOG", &catalog)
        .assert()
        .success()
        .stdout(predicate::str::contains("Installed gcc@13.3.0"));
    let cmake = read(s.project().join(".cxvm/toolchain.cmake"));
    assert!(cmake.contains("--gcc-install-dir="), "{cmake}");
    assert!(cmake.contains("toolchains/gcc/13.3.0/lib64"), "{cmake}");
}

#[test]
fn env_and_exec_carry_the_pairing_in_cc_and_cxx() {
    let s = Sandbox::new();
    let clang = s.install("clang", "17.0.6");
    let gcc = gcc(&s, "13.3.0");
    s.cxvm("")
        .args(["use", "clang@17", "--gcc", "gcc@13"])
        .assert()
        .success();
    let flag = format!("--gcc-install-dir={}", install_dir(&gcc, "13.3.0"));
    let bin = clang.join("bin");

    s.cxvm("a/b")
        .args(["env", "--shell", "fish"])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "set -gx CC '{} {flag}'\n",
            bin.join("clang").display()
        )));
    let path = std::env::join_paths([s.bin(), "/usr/bin".into(), "/bin".into()]).unwrap();
    s.cxvm("a")
        .args(["exec", "--", "sh", "-c", "echo \"$CC|$CXX\""])
        .env("PATH", path)
        .assert()
        .success()
        .stdout(format!(
            "{} {flag}|{} {flag}\n",
            bin.join("clang").display(),
            bin.join("clang++").display()
        ));
}

#[test]
fn the_project_pairing_applies_to_a_clang_chosen_by_the_environment() {
    let s = Sandbox::new();
    s.install("clang", "19.1.0");
    let gcc = gcc(&s, "13.3.0");
    fs::write(
        s.project().join(".cxvmrc"),
        "compiler = \"gcc@13.3.0\"\n\n[clang]\ngcc = \"gcc@13.3.0\"\n",
    )
    .unwrap();
    let flag = format!("--gcc-install-dir={}", install_dir(&gcc, "13.3.0"));
    let out = s
        .cxvm("a")
        .arg("env")
        .env("CXVM_COMPILER", "clang@19")
        .output()
        .unwrap();
    assert!(stdout(&out).contains(&flag), "{}", stdout(&out));
    // The pinned GCC itself gets no option.
    let out = s.cxvm("a").arg("env").output().unwrap();
    assert!(
        !stdout(&out).contains("--gcc-install-dir"),
        "{}",
        stdout(&out)
    );
    // Outside the project there is no pairing.
    s.global("clang@19.1.0");
    let out = s.cxvm("..").arg("env").output().unwrap();
    assert!(out.status.success());
    assert!(
        !stdout(&out).contains("--gcc-install-dir"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn current_reports_the_pairing() {
    let s = Sandbox::new();
    let clang = s.install("clang", "17.0.6");
    let gcc_prefix = s.store().join("toolchains/gcc/13.3.0");
    gcc(&s, "13.3.0");
    let rc = s.project().join(".cxvmrc");
    fs::write(
        &rc,
        "compiler = \"clang@17.0.6\"\n\n[clang]\ngcc = \"gcc@13\"\n",
    )
    .unwrap();

    s.cxvm("a")
        .arg("current")
        .assert()
        .success()
        .stdout(format!(
            "clang@17.0.6 (set by {rc})\n{clang}\nwith gcc@13.3.0 (`[clang] gcc` in {rc})\n",
            rc = rc.display(),
            clang = clang.display()
        ));
    let out = s.cxvm("a").args(["current", "--json"]).output().unwrap();
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(
        report["clang_gcc"],
        serde_json::json!({
            "id": "gcc@13.3.0",
            "requested": "gcc@13",
            "source_path": rc.display().to_string(),
            "installed": true,
            "path": gcc_prefix.display().to_string(),
        })
    );

    fs::remove_dir_all(&gcc_prefix).unwrap();
    s.cxvm("a")
        .arg("current")
        .assert()
        .code(1)
        .stdout(predicate::str::contains(format!(
            "with gcc@13 (`[clang] gcc` in {}, not installed)\n",
            rc.display()
        )))
        .stderr(predicate::str::contains("gcc@13 is not installed"));
    let out = s.cxvm("a").args(["current", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(report["clang_gcc"]["installed"], false);
    assert_eq!(report["clang_gcc"]["id"], serde_json::Value::Null);
}

#[test]
fn current_has_no_pairing_for_a_gcc_or_an_unpaired_clang() {
    let s = Sandbox::new();
    s.install("clang", "17.0.6");
    gcc(&s, "13.3.0");
    let rc = s.project().join(".cxvmrc");
    for content in [
        "compiler = \"gcc@13.3.0\"\n\n[clang]\ngcc = \"gcc@13\"\n",
        "compiler = \"clang@17.0.6\"\n",
        "compiler = \"clang@17.0.6\"\n\n[clang]\ngcc = \"system\"\n",
    ] {
        fs::write(&rc, content).unwrap();
        let out = s.cxvm("").arg("current").output().unwrap();
        assert!(out.status.success(), "{content}");
        assert!(!stdout(&out).contains("with "), "{}", stdout(&out));
        let out = s.cxvm("").args(["current", "--json"]).output().unwrap();
        let report: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
        assert_eq!(report["clang_gcc"], serde_json::Value::Null, "{content}");
    }
}

#[test]
fn the_gitignore_entry_can_be_turned_off() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    fs::write(
        s.project().join(".cxvmrc"),
        "compiler = \"gcc@13.3.0\"\n\n[integrations]\ngitignore = false\n",
    )
    .unwrap();
    s.cxvm("").arg("install").assert().success();
    assert!(s.project().join(".cxvm/toolchain.cmake").exists());
    assert!(!s.project().join(".gitignore").exists());
    s.cxvm("").args(["use", "gcc@13"]).assert().success();
    assert!(!s.project().join(".gitignore").exists());
}
