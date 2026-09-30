//! Shared fixture for integration tests: a throwaway store, home, cache and project tree, and a
//! `cxvm` command that sees only those (never the real store or environment).

#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use assert_cmd::Command;

pub struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    /// Empty store, a project tree `proj/a/b/c`, and a `bin/` on `PATH` holding a fake `cc`
    /// and `c++`.
    pub fn new() -> Self {
        let sandbox = Self {
            dir: tempfile::tempdir().expect("temp dir"),
        };
        fs::create_dir_all(sandbox.project().join("a/b/c")).unwrap();
        fs::create_dir_all(sandbox.path("home")).unwrap();
        sandbox.fake_compiler(&sandbox.bin(), "cc");
        sandbox.fake_compiler(&sandbox.bin(), "c++");
        sandbox
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    pub fn store(&self) -> PathBuf {
        self.path("store")
    }

    pub fn project(&self) -> PathBuf {
        self.path("proj")
    }

    pub fn bin(&self) -> PathBuf {
        self.path("bin")
    }

    /// Writes an executable script `dir/name` that prints a version line.
    pub fn fake_compiler(&self, dir: &Path, name: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\necho '{name} (fake) 1.0.0'\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// Places a toolchain in the store by hand, e.g. `install("gcc", "13.3.0")`.
    pub fn install(&self, kind: &str, version: &str) -> PathBuf {
        let prefix = self.store().join("toolchains").join(kind).join(version);
        fake_toolchain(&prefix, kind, version);
        prefix
    }

    /// Writes `proj/<rel>/.cxvmrc`.
    pub fn pin(&self, rel: &str, compiler: &str) -> PathBuf {
        let path = self.project().join(rel).join(".cxvmrc");
        fs::write(&path, format!("compiler = \"{compiler}\"\n")).unwrap();
        path
    }

    /// Writes the global `config.toml`.
    pub fn global(&self, compiler: &str) -> PathBuf {
        let path = self.store().join("config.toml");
        fs::create_dir_all(self.store()).unwrap();
        fs::write(&path, format!("compiler = \"{compiler}\"\n")).unwrap();
        path
    }

    /// `cxvm` run from `proj/<rel>` with a clean environment pointing into the sandbox.
    pub fn cxvm(&self, rel: &str) -> Command {
        let mut cmd = Command::cargo_bin("cxvm").expect("cxvm binary built by cargo");
        cmd.current_dir(self.project().join(rel))
            .env_clear()
            .env("HOME", self.path("home"))
            .env("CXVM_HOME", self.store())
            .env("XDG_CACHE_HOME", self.path("cache"))
            .env("PATH", self.bin());
        cmd
    }
}

/// Writes `prefix/bin/{gcc,g++}` or `prefix/bin/{clang,clang++}` scripts that answer the
/// probes cxvm runs (`-dumpfullversion`, `--version`, `-print-file-name=…`) like the real
/// drivers. The fake clang's `-dumpversion` prints `4.2.1`, as old releases did.
pub fn fake_toolchain(prefix: &Path, kind: &str, version: &str) {
    let major = version.split('.').next().unwrap();
    let (drivers, script) = match kind {
        "gcc" => (
            ["gcc", "g++"],
            format!(
                "#!/bin/sh\ncase \"$1\" in\n\
                 -dumpfullversion) echo {version} ;;\n\
                 -dumpversion) echo {major} ;;\n\
                 --version) echo \"${{0##*/}} (Fake) {version}\" ;;\n\
                 -print-file-name=*) f=\"${{1#-print-file-name=}}\"; \
                 d=\"${{0%/*}}/../lib64\"; \
                 if [ -e \"$d/$f\" ]; then echo \"$d/$f\"; else echo \"$f\"; fi ;;\n\
                 *) echo \"${{0##*/}} {version} $*\" ;;\n\
                 esac\n"
            ),
        ),
        "clang" => (
            ["clang", "clang++"],
            format!(
                "#!/bin/sh\ncase \"$1\" in\n\
                 -dumpversion) echo 4.2.1 ;;\n\
                 --version) echo 'clang version {version} (fake)' ;;\n\
                 *) echo \"${{0##*/}} {version} $*\" ;;\n\
                 esac\n"
            ),
        ),
        other => panic!("unknown kind {other}"),
    };
    for driver in drivers {
        let path = prefix.join("bin").join(driver);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

pub fn stdout(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("UTF-8 stdout")
}
