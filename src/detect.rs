//! Probing a toolchain prefix: which compiler it holds, its exact version, and where its C++
//! runtime library and GCC install dir live. Used by `cxvm link`, when generating project files
//! (M2) and when installing (M3, M4).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};
use crate::files;
use crate::toolchain::{Kind, Version};

/// What a prefix holds, as reported by its compilers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    pub kind: Kind,
    /// Exact, without a suffix.
    pub version: Version,
    pub cc: PathBuf,
    pub cxx: PathBuf,
    /// First line of `cc --version`, for the record.
    pub version_line: String,
}

/// The C and C++ drivers of `kind` under `prefix`, by convention: `bin/gcc` + `bin/g++` or
/// `bin/clang` + `bin/clang++`.
pub fn compilers(prefix: &Path, kind: Kind) -> (PathBuf, PathBuf) {
    let (cc, cxx) = match kind {
        Kind::Gcc => ("gcc", "g++"),
        Kind::Clang => ("clang", "clang++"),
    };
    (prefix.join("bin").join(cc), prefix.join("bin").join(cxx))
}

/// The kinds whose C driver exists under `prefix`.
pub fn kinds_present(prefix: &Path) -> Vec<Kind> {
    Kind::ALL
        .into_iter()
        .filter(|&kind| files::is_executable(&compilers(prefix, kind).0))
        .collect()
}

/// Runs the compilers of `kind` under `prefix` and reports their exact version. Both drivers
/// must exist and agree.
pub fn probe(prefix: &Path, kind: Kind) -> Result<Detected> {
    let fail = |reason: String, hint: String| Error::Detect {
        prefix: prefix.to_owned(),
        reason,
        hint,
    };
    let (cc, cxx) = compilers(prefix, kind);
    for driver in [&cc, &cxx] {
        if !files::is_executable(driver) {
            return Err(fail(
                format!("{} is missing or not executable", driver.display()),
                "a GCC prefix needs bin/gcc and bin/g++, a Clang prefix bin/clang and bin/clang++"
                    .into(),
            ));
        }
    }
    let version_of = |driver: &Path| {
        let version = match kind {
            Kind::Gcc => capture(driver, "-dumpfullversion")
                .and_then(|out| parse_gcc_version(&out))
                .or_else(|| {
                    capture(driver, "-dumpversion").and_then(|out| parse_gcc_version(&out))
                }),
            Kind::Clang => capture(driver, "--version").and_then(|out| parse_clang_version(&out)),
        };
        version.ok_or_else(|| {
            fail(
                format!("cannot tell the exact version of {}", driver.display()),
                format!("give the id yourself: `cxvm link <prefix> --as {kind}@<x.y.z>-<suffix>`"),
            )
        })
    };
    let version = version_of(&cc)?;
    let cxx_version = version_of(&cxx)?;
    if version != cxx_version {
        return Err(fail(
            format!(
                "{} reports {version} but {} reports {cxx_version}",
                cc.display(),
                cxx.display()
            ),
            "link a prefix whose C and C++ drivers come from the same release".into(),
        ));
    }
    let version_line = version_line(&cc).unwrap_or_default();
    Ok(Detected {
        kind,
        version,
        cc,
        cxx,
        version_line,
    })
}

/// First line of `driver --version`, if it ran and succeeded.
pub fn version_line(driver: &Path) -> Option<String> {
    capture(driver, "--version").and_then(|out| out.lines().next().map(str::to_owned))
}

/// Stdout of `program arg`, if it ran and succeeded.
fn capture(program: &Path, arg: &str) -> Option<String> {
    let output = files::run_captured(Command::new(program).arg(arg)).ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `text` as an exact version without suffix.
fn exact(text: &str) -> Option<Version> {
    text.parse::<Version>()
        .ok()
        .filter(|v| v.is_exact() && v.suffix().is_none())
}

/// Version from `gcc -dumpfullversion` (or `-dumpversion`) output; `None` unless exact.
pub fn parse_gcc_version(output: &str) -> Option<Version> {
    exact(output.trim())
}

/// Version from the `clang version X.Y.Z` part of `clang --version`; `None` unless found.
pub fn parse_clang_version(output: &str) -> Option<Version> {
    const MARKER: &str = "clang version ";
    let rest = &output[output.find(MARKER)? + MARKER.len()..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(rest.len());
    exact(rest[..end].trim_end_matches('.'))
}

/// Directory of the `libstdc++.so.6` that `cxx` links against, from
/// `cxx -print-file-name=libstdc++.so.6`; `None` if the driver does not know one.
pub fn libstdcxx_dir(cxx: &Path) -> Option<PathBuf> {
    let output = capture(cxx, "-print-file-name=libstdc++.so.6")?;
    let path = PathBuf::from(output.trim());
    if !path.is_absolute() {
        return None;
    }
    path.canonicalize().ok()?.parent().map(Path::to_path_buf)
}

/// GCC's install dir (`<prefix>/lib/gcc/<triple>/<version>`): the directory of the `libgcc.a`
/// that `cc` links, from `cc -print-libgcc-file-name`; `None` if the driver does not know one.
pub fn gcc_install_dir(cc: &Path) -> Option<PathBuf> {
    let output = capture(cc, "-print-libgcc-file-name")?;
    let path = PathBuf::from(output.trim());
    if !path.is_absolute() {
        return None;
    }
    path.canonicalize().ok()?.parent().map(Path::to_path_buf)
}

/// The program `driver` runs as `name`, e.g. GCC's `cc1plus`, from `-print-prog-name=`;
/// `None` unless that names a file that exists.
pub fn prog_name(driver: &Path, name: &str) -> Option<PathBuf> {
    let output = capture(driver, &format!("-print-prog-name={name}"))?;
    let path = PathBuf::from(output.trim());
    (path.is_absolute() && path.is_file()).then_some(path)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn version(s: &str) -> Version {
        s.parse().unwrap()
    }

    #[test]
    fn gcc_versions_must_be_exact() {
        assert_eq!(parse_gcc_version("15.2.0\n"), Some(version("15.2.0")));
        assert_eq!(parse_gcc_version("  4.8.5  "), Some(version("4.8.5")));
        assert_eq!(parse_gcc_version("15\n"), None, "-dumpversion on Ubuntu");
        assert_eq!(parse_gcc_version("13.2\n"), None);
        assert_eq!(parse_gcc_version(""), None);
        assert_eq!(parse_gcc_version("gcc: error: unrecognized option"), None);
    }

    #[test]
    fn clang_versions_come_from_the_version_banner() {
        for (banner, expected) in [
            (
                "clang version 18.1.8\nTarget: x86_64-unknown-linux-gnu",
                "18.1.8",
            ),
            (
                "Ubuntu clang version 21.1.8 (6ubuntu1)\nTarget: x86_64-pc-linux-gnu",
                "21.1.8",
            ),
            (
                "clang version 19.1.0-rc3 (https://github.com/llvm/llvm-project abc)",
                "19.1.0",
            ),
            ("clang version 17.0.6git", "17.0.6"),
            ("Debian clang version 14.0.6", "14.0.6"),
        ] {
            assert_eq!(
                parse_clang_version(banner),
                Some(version(expected)),
                "{banner}"
            );
        }
        for banner in ["4.2.1", "clang version 18", "gcc (GCC) 13.3.0", ""] {
            assert_eq!(parse_clang_version(banner), None, "{banner}");
        }
    }

    /// Writes an executable script `prefix/bin/name` answering the probes like `kind` would.
    fn fake(prefix: &Path, name: &str, kind: Kind, version: &str) {
        let script = match kind {
            Kind::Gcc => format!(
                "#!/bin/sh\ncase \"$1\" in\n\
                 -dumpfullversion) echo {version} ;;\n\
                 -dumpversion) echo {major} ;;\n\
                 --version) echo '{name} (Fake) {version}'; echo 'Copyright' ;;\n\
                 -print-file-name=*) f=\"${{1#-print-file-name=}}\"; \
                 d=\"${{0%/*}}/../lib64\"; \
                 if [ -e \"$d/$f\" ]; then echo \"$d/$f\"; else echo \"$f\"; fi ;;\n\
                 -print-libgcc-file-name) \
                 d=\"${{0%/*}}/../lib/gcc/x86_64-pc-linux-gnu/{version}\"; \
                 if [ -e \"$d/libgcc.a\" ]; then echo \"$d/libgcc.a\"; else echo libgcc.a; fi ;;\n\
                 -print-prog-name=*) p=\"${{1#-print-prog-name=}}\"; \
                 d=\"${{0%/*}}/../libexec/gcc/x86_64-pc-linux-gnu/{version}\"; \
                 if [ -e \"$d/$p\" ]; then echo \"$d/$p\"; else echo \"$p\"; fi ;;\n\
                 esac\n",
                major = version.split('.').next().unwrap()
            ),
            Kind::Clang => format!(
                "#!/bin/sh\ncase \"$1\" in\n\
                 -dumpversion) echo 4.2.1 ;;\n\
                 --version) echo 'clang version {version} (fake)'; echo 'Target: x86_64' ;;\n\
                 esac\n"
            ),
        };
        let path = prefix.join("bin").join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn probes_a_gcc_prefix() {
        let dir = tempfile::tempdir().unwrap();
        fake(dir.path(), "gcc", Kind::Gcc, "11.4.0");
        fake(dir.path(), "g++", Kind::Gcc, "11.4.0");
        let detected = probe(dir.path(), Kind::Gcc).unwrap();
        assert_eq!(
            detected,
            Detected {
                kind: Kind::Gcc,
                version: version("11.4.0"),
                cc: dir.path().join("bin/gcc"),
                cxx: dir.path().join("bin/g++"),
                version_line: "gcc (Fake) 11.4.0".into(),
            }
        );
    }

    #[test]
    fn probes_a_clang_prefix_ignoring_dumpversion() {
        let dir = tempfile::tempdir().unwrap();
        fake(dir.path(), "clang", Kind::Clang, "18.1.8");
        fake(dir.path(), "clang++", Kind::Clang, "18.1.8");
        let detected = probe(dir.path(), Kind::Clang).unwrap();
        assert_eq!(detected.version, version("18.1.8"));
        assert_eq!(detected.version_line, "clang version 18.1.8 (fake)");
    }

    #[test]
    fn missing_cxx_driver_is_an_error_naming_it() {
        let dir = tempfile::tempdir().unwrap();
        fake(dir.path(), "gcc", Kind::Gcc, "11.4.0");
        let err = probe(dir.path(), Kind::Gcc).unwrap_err().to_string();
        assert!(err.contains("bin/g++"), "{err}");
        assert!(err.contains("hint:"), "{err}");
    }

    #[test]
    fn drivers_that_disagree_are_an_error() {
        let dir = tempfile::tempdir().unwrap();
        fake(dir.path(), "gcc", Kind::Gcc, "11.4.0");
        fake(dir.path(), "g++", Kind::Gcc, "11.5.0");
        let err = probe(dir.path(), Kind::Gcc).unwrap_err().to_string();
        assert!(err.contains("11.4.0") && err.contains("11.5.0"), "{err}");
    }

    #[test]
    fn undetectable_version_is_an_error_suggesting_as() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["clang", "clang++"] {
            let path = dir.path().join("bin").join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "#!/bin/sh\necho 'weird compiler 1'\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let err = probe(dir.path(), Kind::Clang).unwrap_err().to_string();
        assert!(err.contains("--as"), "{err}");
    }

    #[test]
    fn a_driver_that_is_briefly_busy_is_retried() {
        let dir = tempfile::tempdir().unwrap();
        fake(dir.path(), "gcc", Kind::Gcc, "11.4.0");
        let gcc = dir.path().join("bin/gcc");
        // An open write handle makes exec fail with ETXTBSY, as when another thread forks
        // while a test writes a fake compiler.
        let handle = fs::OpenOptions::new().append(true).open(&gcc).unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            drop(handle);
        });
        assert_eq!(
            capture(&gcc, "-dumpfullversion").as_deref(),
            Some("11.4.0\n")
        );
        release.join().unwrap();
    }

    #[test]
    fn lists_the_kinds_present() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(kinds_present(dir.path()), []);
        fake(dir.path(), "clang", Kind::Clang, "18.1.8");
        assert_eq!(kinds_present(dir.path()), [Kind::Clang]);
        fake(dir.path(), "gcc", Kind::Gcc, "13.3.0");
        assert_eq!(kinds_present(dir.path()), [Kind::Gcc, Kind::Clang]);
    }

    #[test]
    fn finds_the_libstdcxx_dir_the_driver_reports() {
        let dir = tempfile::tempdir().unwrap();
        fake(dir.path(), "g++", Kind::Gcc, "13.3.0");
        let cxx = dir.path().join("bin/g++");
        assert_eq!(libstdcxx_dir(&cxx), None, "driver prints the bare name");
        let lib64 = dir.path().join("lib64");
        fs::create_dir_all(&lib64).unwrap();
        fs::write(lib64.join("libstdc++.so.6"), "").unwrap();
        assert_eq!(libstdcxx_dir(&cxx), Some(lib64.canonicalize().unwrap()));
        assert_eq!(libstdcxx_dir(&dir.path().join("bin/missing")), None);
    }

    #[test]
    fn finds_the_gcc_install_dir_the_driver_reports() {
        let dir = tempfile::tempdir().unwrap();
        fake(dir.path(), "gcc", Kind::Gcc, "13.3.0");
        let cc = dir.path().join("bin/gcc");
        assert_eq!(gcc_install_dir(&cc), None, "driver prints the bare name");
        let install_dir = dir.path().join("lib/gcc/x86_64-pc-linux-gnu/13.3.0");
        fs::create_dir_all(&install_dir).unwrap();
        fs::write(install_dir.join("libgcc.a"), "").unwrap();
        assert_eq!(
            gcc_install_dir(&cc),
            Some(install_dir.canonicalize().unwrap())
        );
        assert_eq!(gcc_install_dir(&dir.path().join("bin/missing")), None);
    }

    #[test]
    fn finds_the_programs_the_driver_runs() {
        let dir = tempfile::tempdir().unwrap();
        fake(dir.path(), "g++", Kind::Gcc, "13.3.0");
        let cxx = dir.path().join("bin/g++");
        assert_eq!(
            prog_name(&cxx, "cc1plus"),
            None,
            "driver prints the bare name"
        );
        let libexec = dir.path().join("libexec/gcc/x86_64-pc-linux-gnu/13.3.0");
        fs::create_dir_all(&libexec).unwrap();
        fs::write(libexec.join("cc1plus"), "").unwrap();
        let found = prog_name(&cxx, "cc1plus").unwrap();
        assert_eq!(
            found.canonicalize().unwrap(),
            libexec.join("cc1plus").canonicalize().unwrap()
        );
    }
}
