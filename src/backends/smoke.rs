//! Post-install checks: can the new compiler run on this host, and can it build and run a
//! C++ program?

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use crate::files;

/// Shared libraries `binary` needs that the dynamic loader cannot find, from `ldd`. Empty when
/// `ldd` is missing or `binary` is not dynamically linked: the version probe catches a binary
/// that cannot run at all.
pub fn missing_libraries(binary: &Path) -> Vec<String> {
    let Ok(output) = files::retry_busy(|| Command::new("ldd").arg(binary).output()) else {
        return Vec::new();
    };
    parse_ldd(&String::from_utf8_lossy(&output.stdout))
}

/// The libraries `ldd` reports as `not found`.
fn parse_ldd(output: &str) -> Vec<String> {
    output
        .lines()
        .filter(|line| line.contains("=> not found"))
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

/// What to install for the missing `libraries`.
pub fn missing_libraries_hint(libraries: &[String]) -> String {
    let mut hint = format!(
        "install the packages that provide {} and run the same command again; the archive \
         stays in the download cache",
        libraries.join(", ")
    );
    if libraries
        .iter()
        .any(|lib| lib.starts_with("libtinfo.so.5") || lib.starts_with("libncurses.so.5"))
    {
        hint.push_str(
            ". libtinfo.so.5 is `libtinfo5` on older Debian/Ubuntu (Ubuntu 24.04 and later no \
             longer ship it), `ncurses-compat-libs` on Fedora/RHEL, `ncurses5-compat-libs` (AUR) \
             on Arch. Linking it to libtinfo.so.6 does not work (different symbol versions); \
             clang 19 and later do not need it",
        );
    }
    hint
}

/// Builds and runs a C++ hello-world with `cxx` in `scratch` (created, then deleted). `None`
/// if it printed what it should, else what went wrong.
pub fn compile_and_run(cxx: &Path, scratch: &Path) -> Option<String> {
    let result = (|| {
        fs::create_dir_all(scratch)
            .map_err(|e| format!("cannot create {}: {e}", scratch.display()))?;
        let source = scratch.join("hello.cpp");
        let program = scratch.join("hello");
        fs::write(
            &source,
            "#include <iostream>\nint main() { std::cout << \"cxvm-ok\" << std::endl; }\n",
        )
        .map_err(|e| format!("cannot write {}: {e}", source.display()))?;
        let compile = run(Command::new(cxx).arg(&source).arg("-o").arg(&program))
            .map_err(|e| format!("cannot run {}: {e}", cxx.display()))?;
        if !compile.status.success() {
            return Err(format!(
                "compiling a hello-world failed:\n{}",
                tail(&compile.stderr)
            ));
        }
        let output = run(&mut Command::new(&program))
            .map_err(|e| format!("cannot run the compiled hello-world: {e}"))?;
        if !output.status.success() || !String::from_utf8_lossy(&output.stdout).contains("cxvm-ok")
        {
            return Err(format!(
                "the compiled hello-world did not run correctly:\n{}",
                tail(&output.stderr)
            ));
        }
        Ok(())
    })();
    let _ = fs::remove_dir_all(scratch);
    result.err()
}

fn run(command: &mut Command) -> std::io::Result<Output> {
    files::retry_busy(|| command.output())
}

/// The last lines of `stderr`, indented.
fn tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(5);
    lines[start..]
        .iter()
        .map(|line| format!("    {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn finds_libraries_ldd_cannot_resolve() {
        let output = "\tlinux-vdso.so.1 (0x00007ffd)\n\
                      \tlibtinfo.so.5 => not found\n\
                      \tlibz.so.1 => /lib/x86_64-linux-gnu/libz.so.1 (0x00007f)\n\
                      \tlibxml2.so.2 => not found\n";
        assert_eq!(parse_ldd(output), ["libtinfo.so.5", "libxml2.so.2"]);
        assert!(parse_ldd("\tnot a dynamic executable\n").is_empty());
    }

    #[test]
    fn libtinfo_hint_names_the_packages() {
        let hint = missing_libraries_hint(&["libtinfo.so.5".into()]);
        assert!(
            hint.contains("libtinfo5") && hint.contains("ncurses-compat-libs"),
            "{hint}"
        );
        let hint = missing_libraries_hint(&["libxml2.so.2".into()]);
        assert!(
            hint.contains("libxml2.so.2") && !hint.contains("libtinfo5"),
            "{hint}"
        );
    }

    #[test]
    fn reports_a_compiler_that_fails() {
        let dir = tempfile::tempdir().unwrap();
        let cxx = dir.path().join("clang++");
        fs::write(
            &cxx,
            "#!/bin/sh\necho 'fatal error: iostream not found' >&2\nexit 1\n",
        )
        .unwrap();
        fs::set_permissions(&cxx, fs::Permissions::from_mode(0o755)).unwrap();
        let scratch = dir.path().join("scratch");
        let problem = compile_and_run(&cxx, &scratch).unwrap();
        assert!(problem.contains("iostream not found"), "{problem}");
        assert!(!scratch.exists());
    }
}
