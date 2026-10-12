//! `cxvm prune`: emptying the download cache and the build logs.

mod common;

use std::fs;
use std::path::PathBuf;

use common::Sandbox;
use predicates::prelude::*;

/// A cache with an archive, the `.part` of an interrupted download and a build log.
fn fill_cache(s: &Sandbox) -> [PathBuf; 3] {
    let downloads = s.path("cache/cxvm/downloads");
    let logs = s.path("cache/cxvm/logs");
    fs::create_dir_all(&downloads).unwrap();
    fs::create_dir_all(&logs).unwrap();
    let files = [
        downloads.join("LLVM-19.1.0-Linux-X64.tar.xz"),
        downloads.join("gcc-13.3.0.tar.xz.part"),
        logs.join("gcc-10.5.0-20261005T101500Z.log"),
    ];
    fs::write(&files[0], vec![0; 3 << 20]).unwrap();
    fs::write(&files[1], vec![0; 1000]).unwrap();
    fs::write(&files[2], "configure: error\n").unwrap();
    files
}

#[test]
fn deletes_downloads_partial_downloads_and_logs() {
    let s = Sandbox::new();
    let files = fill_cache(&s);
    s.cxvm("")
        .arg("prune")
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "Deleted {} (3 MiB)",
            files[0].display()
        )))
        .stdout(predicate::str::contains(format!(
            "Deleted {} (1 KiB)",
            files[1].display()
        )))
        .stdout(predicate::str::contains(format!(
            "Deleted {}",
            files[2].display()
        )))
        .stdout(predicate::str::contains("Freed 4 MiB in"))
        .stderr(predicate::str::is_empty());
    for file in &files {
        assert!(!file.exists(), "{}", file.display());
    }
    assert!(s.path("cache/cxvm/downloads").is_dir());
}

#[test]
fn dry_run_lists_and_keeps_everything() {
    let s = Sandbox::new();
    let files = fill_cache(&s);
    s.cxvm("")
        .args(["prune", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "Would delete {} (3 MiB)",
            files[0].display()
        )))
        .stdout(predicate::str::contains("Would free 4 MiB"))
        .stdout(predicate::str::contains("run `cxvm prune`"));
    for file in &files {
        assert!(file.exists(), "{}", file.display());
    }
    assert!(!s.store().join("lock").exists(), "a dry run takes no lock");
}

#[test]
fn an_empty_cache_is_nothing_to_prune() {
    let s = Sandbox::new();
    s.cxvm("")
        .arg("prune")
        .assert()
        .success()
        .stdout(predicate::str::contains("Nothing to prune"));
}

#[test]
fn pruning_takes_the_store_lock_and_so_cleans_staging() {
    let s = Sandbox::new();
    let leftover = s.store().join("staging/clang-19.1.0.4242");
    fs::create_dir_all(&leftover).unwrap();
    s.cxvm("")
        .arg("prune")
        .assert()
        .success()
        .stderr(predicate::str::contains("left by an interrupted install"));
    assert!(!leftover.exists());
}
