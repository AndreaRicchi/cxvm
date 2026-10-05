//! `cxvm install --from-source`: building GCC from a source release. The "sources" are fake: a
//! `configure` that writes a tiny `Makefile`, built with the host's real `make`, served with
//! their library tarballs by a local HTTP server (default tests never touch the network).

mod common;

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{
    FakeGcc, FakeSource, Sandbox, Server, append_prerequisites, asset_name, fake_prerequisite,
    stdout, write_catalog,
};
use predicates::prelude::*;

const LIBRARIES: [&str; 4] = [
    "gmp-6.1.0.tar.bz2",
    "mpfr-3.1.6.tar.bz2",
    "mpc-1.0.3.tar.gz",
    "isl-0.18.tar.bz2",
];

/// A sandbox whose catalog lists fake GCC source releases and the libraries they name, with a
/// system compiler that "compiles" and fake `m4` and `awk` next to it.
struct Fixture {
    sandbox: Sandbox,
    server: Server,
    catalog: PathBuf,
}

impl Fixture {
    fn new(sources: &[FakeSource<'_>]) -> Self {
        Self::with_prebuilt(sources, &[])
    }

    /// `sources`, plus working fake xPack archives for the `prebuilt` GCC versions.
    fn with_prebuilt(sources: &[FakeSource<'_>], prebuilt: &[&str]) -> Self {
        let sandbox = Sandbox::new();
        let server = Server::start();
        let catalog = sandbox.path("catalog.toml");
        for name in ["cc", "c++"] {
            sandbox.compiling_compiler(&sandbox.bin(), name);
        }
        for name in ["m4", "awk"] {
            sandbox.fake_compiler(&sandbox.bin(), name);
        }

        let mut archives: Vec<(&str, &str, Vec<u8>)> = Vec::new();
        for source in sources {
            archives.push(("gcc-source", source.version, source.archive()));
        }
        for version in prebuilt {
            archives.push(("gcc", version, FakeGcc::new(version).archive()));
        }
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

        let libraries: Vec<(&str, String, Vec<u8>)> = LIBRARIES
            .iter()
            .map(|asset| {
                let archive = fake_prerequisite(asset);
                (*asset, server.add(asset, archive.clone()), archive)
            })
            .collect();
        let entries: Vec<(&str, &str, &[u8])> = libraries
            .iter()
            .map(|(asset, url, archive)| (*asset, url.as_str(), archive.as_slice()))
            .collect();
        append_prerequisites(&catalog, &entries);
        Self {
            sandbox,
            server,
            catalog,
        }
    }

    /// `cxvm` seeing the fixture's catalog, with the host's `PATH` (for `make`, `sh` and the
    /// core utilities) after the sandbox's.
    fn cxvm(&self, rel: &str) -> assert_cmd::Command {
        let mut cmd = self.sandbox.cxvm(rel);
        cmd.env("CXVM_CATALOG", &self.catalog)
            .env("PATH", self.sandbox.path_with_host());
        cmd
    }

    fn toolchain(&self, version: &str) -> PathBuf {
        self.sandbox.store().join("toolchains/gcc").join(version)
    }

    /// What the fake `configure` of the installed `version` saw.
    fn record(&self, version: &str) -> String {
        fs::read_to_string(self.toolchain(version).join("share/configure.record")).unwrap()
    }

    fn meta(&self, version: &str) -> String {
        fs::read_to_string(self.toolchain(version).join(".cxvm-meta.toml")).unwrap()
    }

    fn logs(&self) -> Vec<PathBuf> {
        fs::read_dir(self.sandbox.path("cache/cxvm/logs"))
            .map(|dir| dir.map(|entry| entry.unwrap().path()).collect())
            .unwrap_or_default()
    }

    fn staging_is_empty(&self) -> bool {
        fs::read_dir(self.sandbox.store().join("staging")).map_or(true, |mut d| d.next().is_none())
    }
}

#[test]
fn builds_and_installs_a_source_release() {
    let f = Fixture::new(&[FakeSource::new("10.5.0")]);
    let out = f
        .cxvm("")
        .args(["install", "gcc@10.5.0", "--from-source"])
        .env("CFLAGS", "-from-the-callers-shell")
        .assert()
        .success()
        .get_output()
        .clone();
    let text = stdout(&out);
    let prefix = f.toolchain("10.5.0");
    assert!(text.contains("Building gcc@10.5.0 from source"), "{text}");
    assert!(text.contains("host compiler: system"), "{text}");
    assert!(text.contains("Ctrl-C cancels"), "{text}");
    assert!(
        text.contains(&format!("Installed gcc@10.5.0 in {}", prefix.display())),
        "{text}"
    );
    assert!(text.contains("pin it with `cxvm use gcc@10.5.0`"), "{text}");

    // Configured for the final store path, built with the system compiler, in a clean
    // environment, with the libraries its own script names in the tree.
    let record = f.record("10.5.0");
    let args = format!(
        "args: --prefix={} --enable-languages=c,c++ --disable-multilib --disable-bootstrap\n",
        prefix.display()
    );
    assert!(record.contains(&args), "{record}");
    let bin = f.sandbox.bin();
    assert!(
        record.contains(&format!("CC={}\n", bin.join("cc").display())),
        "{record}"
    );
    assert!(
        record.contains(&format!("CXX={}\n", bin.join("c++").display())),
        "{record}"
    );
    assert!(record.contains("CFLAGS=unset\n"), "{record}");
    for library in [
        "with gmp: gmp-6.1.0",
        "with mpfr: mpfr-3.1.6",
        "with mpc: mpc-1.0.3",
        "with isl: isl-0.18",
    ] {
        assert!(record.contains(library), "{record}");
    }

    let meta = f.meta("10.5.0");
    assert!(meta.contains("backend = \"gnu-source\""), "{meta}");
    assert!(meta.contains("/gcc-10.5.0.tar.xz\""), "{meta}");
    assert!(
        meta.contains("host_compiler = \"clang version 1.0.0 (fake)\""),
        "{meta}"
    );
    assert!(
        meta.contains(&format!(
            "configure_args = [\"--prefix={}\", \"--enable-languages=c,c++\"",
            prefix.display()
        )),
        "{meta}"
    );
    assert!(
        meta.contains(&format!(
            "libstdcxx_dir = \"{}\"",
            prefix.join("lib64").display()
        )),
        "{meta}"
    );

    // The log has every step, and nothing is left in staging.
    let logs = f.logs();
    assert_eq!(logs.len(), 1, "{logs:?}");
    let name = logs[0].file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        name.starts_with("gcc-10.5.0-") && name.ends_with(".log"),
        "{name}"
    );
    assert!(text.contains(&format!("the build log is {}", logs[0].display())));
    let log = fs::read_to_string(&logs[0]).unwrap();
    for line in [
        "==> configure: ",
        "checking for a fake compiler... yes",
        "==> make: ",
        "building fake gcc",
        "==> make install: ",
        "install-strip",
    ] {
        assert!(log.contains(line), "expected {line:?} in {log}");
    }
    assert!(f.staging_is_empty());
    assert!(
        f.sandbox
            .path("cache/cxvm/downloads/gcc-10.5.0.tar.xz")
            .is_file()
    );

    // It is a toolchain like any other.
    f.cxvm("").args(["use", "gcc@10"]).assert().success();
    let cmake = fs::read_to_string(f.sandbox.project().join(".cxvm/toolchain.cmake")).unwrap();
    assert!(
        cmake.contains(&format!("{}/bin/g++", prefix.display())),
        "{cmake}"
    );
    f.cxvm("")
        .args(["install", "gcc@10.5.0", "--from-source"])
        .assert()
        .success()
        .stdout(predicate::str::contains("already installed"));
}

#[test]
fn build_options_reach_configure_and_make() {
    let f = Fixture::new(&[FakeSource::new("10.5.0")]);
    f.cxvm("")
        .args([
            "install",
            "gcc@10.5.0",
            "--from-source",
            "--bootstrap",
            "-j",
            "3",
            "--configure-arg=--disable-libsanitizer",
            "--configure-arg",
            "--with-system-zlib",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("jobs:          3"));
    let record = f.record("10.5.0");
    assert!(
        record.contains(
            "--enable-languages=c,c++ --disable-multilib --disable-libsanitizer \
             --with-system-zlib\n"
        ),
        "{record}"
    );
    assert!(!record.contains("--disable-bootstrap"), "{record}");
    let log = fs::read_to_string(&f.logs()[0]).unwrap();
    assert!(log.contains("\"-j3\""), "{log}");
    let meta = f.meta("10.5.0");
    assert!(meta.contains("\"--with-system-zlib\"]"), "{meta}");
}

#[test]
fn a_failing_build_installs_nothing_and_names_the_log() {
    let f = Fixture::new(&[FakeSource {
        build: "@echo 'x.c:1:1: error: boom' >&2; exit 1",
        ..FakeSource::new("10.5.0")
    }]);
    let out = f
        .cxvm("")
        .args(["install", "gcc@10.5.0", "--from-source"])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cannot build gcc@10.5.0: `make` exited with status 2"),
        "{stderr}"
    );
    assert!(stderr.contains("x.c:1:1: error: boom"), "{stderr}");
    assert!(stderr.contains("--host-compiler <id>"), "{stderr}");
    let logs = f.logs();
    assert_eq!(logs.len(), 1);
    assert!(
        stderr.contains(&format!("the full log is {}", logs[0].display())),
        "{stderr}"
    );
    assert!(
        fs::read_to_string(&logs[0])
            .unwrap()
            .contains("error: boom")
    );
    assert!(!f.toolchain("10.5.0").exists());
    assert!(f.staging_is_empty());
    assert!(
        f.sandbox
            .path("cache/cxvm/downloads/gcc-10.5.0.tar.xz")
            .is_file()
    );
}

#[test]
fn a_failure_in_libsanitizer_names_the_way_out() {
    let f = Fixture::new(&[FakeSource {
        build: "@echo 'make[1]: *** [Makefile:1: all-target-libsanitizer] Error 2'; exit 1",
        ..FakeSource::new("10.5.0")
    }]);
    f.cxvm("")
        .args(["install", "gcc@10.5.0", "--from-source"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "--configure-arg=--disable-libsanitizer",
        ));
}

#[test]
fn missing_tools_are_named_before_anything_is_downloaded() {
    let f = Fixture::new(&[FakeSource::new("10.5.0")]);
    // The sandbox's `bin/` alone: no `make`.
    f.cxvm("")
        .env("PATH", f.sandbox.bin())
        .args(["install", "gcc@10.5.0", "--from-source"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "cannot build gcc@10.5.0 from source here: there is no `make` on PATH",
        ))
        .stderr(predicate::str::contains("build-essential m4"));

    // A system compiler that cannot build a program.
    f.sandbox.fake_compiler(&f.sandbox.bin(), "cc");
    f.cxvm("")
        .args(["install", "gcc@10.5.0", "--from-source"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("bin/cc"))
        .stderr(predicate::str::contains("does not run"));

    // No compiler at all.
    fs::remove_file(f.sandbox.bin().join("c++")).unwrap();
    let no_cxx: Vec<PathBuf> = std::env::split_paths(&f.sandbox.path_with_host())
        .filter(|dir| !dir.join("c++").exists())
        .collect();
    f.cxvm("")
        .env("PATH", std::env::join_paths(no_cxx).unwrap())
        .args(["install", "gcc@10.5.0", "--from-source"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "there is no C and C++ compiler (`cc`, `c++`) on PATH",
        ));

    assert!(f.server.requests().is_empty());
    assert!(f.staging_is_empty());
    assert!(f.logs().is_empty());
}

#[test]
fn a_managed_gcc_can_be_the_host_compiler() {
    let f = Fixture::new(&[FakeSource::new("10.5.0")]);
    let host = f.toolchain("13.3.0");
    FakeGcc::new("13.3.0").install_into(&host);
    f.cxvm("")
        .args([
            "install",
            "gcc@10.5.0",
            "--from-source",
            "--host-compiler",
            "gcc@13",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "host compiler: gcc@13.3.0 (gcc (fake xPack GCC) 13.3.0)",
        ));
    let record = f.record("10.5.0");
    assert!(
        record.contains(&format!("CC={}\n", host.join("bin/gcc").display())),
        "{record}"
    );
    assert!(
        record.contains(&format!("CXX={}\n", host.join("bin/g++").display())),
        "{record}"
    );
    let meta = f.meta("10.5.0");
    assert!(
        meta.contains("host_compiler = \"gcc (fake xPack GCC) 13.3.0\""),
        "{meta}"
    );
}

#[test]
fn refuses_host_compilers_it_cannot_use() {
    let f = Fixture::new(&[FakeSource::new("10.5.0")]);
    f.sandbox.install("clang", "19.1.0");
    for (host, expected) in [
        ("clang@19", "cxvm builds GCC with GCC, not with Clang"),
        ("gcc@9", "gcc@9 is not installed (--host-compiler)"),
        ("gcc@", "invalid toolchain id"),
    ] {
        f.cxvm("")
            .args([
                "install",
                "gcc@10.5.0",
                "--from-source",
                "--host-compiler",
                host,
            ])
            .assert()
            .code(1)
            .stderr(predicate::str::contains(expected));
    }
    assert!(f.server.requests().is_empty());
}

#[test]
fn without_from_source_a_source_release_is_only_offered() {
    let f = Fixture::new(&[FakeSource::new("10.5.0")]);
    // Not a terminal: nobody to ask.
    f.cxvm("")
        .args(["install", "gcc@10"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "gcc@10.5.0 has no prebuilt archive; cxvm can build it from source",
        ))
        .stderr(predicate::str::contains(
            "run `cxvm install gcc@10 --from-source` (about ",
        ));
    assert!(f.server.requests().is_empty());

    f.sandbox.pin("", "gcc@10.5.0");
    f.cxvm("")
        .arg("install")
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "run `cxvm install --from-source` (about ",
        ));
    f.cxvm("")
        .args(["install", "--from-source"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Installed gcc@10.5.0"))
        .stdout(predicate::str::contains("Generated"));
    assert!(f.sandbox.project().join(".cxvm/toolchain.cmake").is_file());
}

#[test]
fn partial_ids_prefer_prebuilt_unless_asked_to_build() {
    let f = Fixture::with_prebuilt(
        &[FakeSource::new("13.4.0"), FakeSource::new("13.5.0")],
        &["13.4.0"],
    );
    f.cxvm("")
        .args(["install", "gcc@13"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Resolved gcc@13 to gcc@13.4.0"))
        .stdout(predicate::str::contains("Installing gcc@13.4.0"));
    assert!(f.meta("13.4.0").contains("backend = \"xpack-prebuilt\""));

    f.cxvm("")
        .args(["install", "gcc@13", "--from-source"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Resolved gcc@13 to gcc@13.5.0"))
        .stdout(predicate::str::contains("Building gcc@13.5.0 from source"));
    assert!(f.meta("13.5.0").contains("backend = \"gnu-source\""));

    f.cxvm("")
        .args(["install", "gcc@9", "--from-source"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("gcc@9 is not a source release in"))
        .stderr(predicate::str::contains("cxvm list --available"));
    f.cxvm("")
        .args(["install", "gcc@13.9.0"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("gcc@13.9.0 is not in"))
        .stderr(predicate::str::contains(
            "available: gcc@13.4.0; with `--from-source`: gcc@13.5.0",
        ));
}

#[test]
fn a_library_the_catalog_lacks_stops_the_build() {
    let f = Fixture::new(&[
        FakeSource {
            prerequisites: "gmp='gmp-9.9.9.tar.bz2'\nmpfr='mpfr-3.1.6.tar.bz2'\n\
                            mpc='mpc-1.0.3.tar.gz'\n",
            ..FakeSource::new("10.5.0")
        },
        FakeSource {
            prerequisites: "gmp='gmp-6.1.0.tar.bz2'\nmpfr='mpfr-3.1.6.tar.bz2'\n\
                            mpc='mpc-1.0.3.tar.gz'\nisl='isl-9.9.tar.bz2'\n",
            ..FakeSource::new("11.5.0")
        },
    ]);
    f.cxvm("")
        .args(["install", "gcc@10.5.0", "--from-source"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "cannot install gcc@10.5.0: its sources need gmp-9.9.9.tar.bz2, which is not in",
        ));
    assert!(!f.toolchain("10.5.0").exists());
    assert!(f.staging_is_empty());

    // ISL is optional: GCC builds without it.
    f.cxvm("")
        .args(["install", "gcc@11.5.0", "--from-source"])
        .assert()
        .success()
        .stderr(predicate::str::contains("building without ISL"));
    let record = f.record("11.5.0");
    assert!(
        record.contains("with mpc: mpc-1.0.3") && !record.contains("with isl"),
        "{record}"
    );
}

#[test]
fn available_lists_what_can_only_be_built() {
    let f = Fixture::with_prebuilt(
        &[FakeSource::new("13.4.0"), FakeSource::new("10.5.0")],
        &["13.4.0"],
    );
    let out = f
        .cxvm("")
        .args(["list", "--available"])
        .assert()
        .success()
        .get_output()
        .clone();
    let text = stdout(&out);
    let (prebuilt, from_source) = text
        .split_once("No prebuilt archive, built with `cxvm install <id> --from-source`:")
        .unwrap_or_else(|| panic!("{text}"));
    assert!(
        prebuilt.contains("13.4.0") && !prebuilt.contains("10.5.0"),
        "{text}"
    );
    assert!(
        from_source.contains("10.5.0") && !from_source.contains("13.4.0"),
        "{text}"
    );

    let out = f
        .cxvm("")
        .args(["list", "--available", "--json"])
        .assert()
        .success()
        .get_output()
        .clone();
    let json: String = stdout(&out).split_whitespace().collect();
    let ids: Vec<&str> = json
        .match_indices("\"id\":")
        .map(|(i, _)| &json[i..])
        .collect();
    assert_eq!(ids.len(), 2, "{json}");
    assert!(ids[0].starts_with("\"id\":\"gcc@13.4.0\""), "{json}");
    assert!(ids[0].contains("\"from_source\":false}"), "{json}");
    assert!(ids[1].starts_with("\"id\":\"gcc@10.5.0\""), "{json}");
    assert!(ids[1].contains("\"from_source\":true}"), "{json}");
}

#[test]
fn an_interrupted_build_leaves_nothing_behind() {
    let f = Fixture::new(&[FakeSource {
        build: "@echo building fake gcc; sleep 600",
        ..FakeSource::new("10.5.0")
    }]);
    let mut cmd = f.sandbox.std_cxvm("");
    cmd.env("CXVM_CATALOG", &f.catalog)
        .env("PATH", f.sandbox.path_with_host())
        .args(["install", "gcc@10.5.0", "--from-source"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = cmd.spawn().unwrap();

    // Wait until `make` runs, then interrupt cxvm as Ctrl-C would.
    let deadline = Instant::now() + Duration::from_secs(60);
    let building = || {
        f.logs()
            .first()
            .and_then(|log| fs::read_to_string(log).ok())
            .is_some_and(|log| log.contains("building fake gcc"))
    };
    while !building() {
        assert!(Instant::now() < deadline, "the build never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    let interrupted = Instant::now();
    let status = std::process::Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let out = child.wait_with_output().unwrap();

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(130), "{stderr}");
    assert!(
        interrupted.elapsed() < Duration::from_secs(30),
        "cxvm waited for the build instead of stopping it"
    );
    assert!(
        stderr.contains("the build of gcc@10.5.0 was interrupted; nothing was installed"),
        "{stderr}"
    );
    assert!(!f.toolchain("10.5.0").exists());
    assert!(f.staging_is_empty());
    let log = fs::read_to_string(&f.logs()[0]).unwrap();
    assert!(log.contains("==> interrupted"), "{log}");
    assert!(
        f.sandbox
            .path("cache/cxvm/downloads/gcc-10.5.0.tar.xz")
            .is_file()
    );
}
