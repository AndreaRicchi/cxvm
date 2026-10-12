//! Post-install checks: can the new compiler run on this host, can it build and run a C++
//! program, and does that program load the toolchain's own libstdc++?

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::files;

/// Shared libraries `binary` needs that the dynamic loader cannot find, from `ldd`. Empty when
/// `ldd` is missing or `binary` is not dynamically linked: the version probe catches a binary
/// that cannot run at all.
pub fn missing_libraries(binary: &Path) -> Vec<String> {
    let Ok(output) = files::run_captured(Command::new("ldd").arg(binary)) else {
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

/// The file `ldd` resolves `library` to, if its output lists it with a path.
fn loaded_library(ldd_output: &str, library: &str) -> Option<PathBuf> {
    ldd_output.lines().find_map(|line| {
        let (name, target) = line.trim().split_once(" => ")?;
        let path = target.rsplit_once(" (").map_or(target, |(path, _)| path);
        (name == library && path.starts_with('/')).then(|| PathBuf::from(path))
    })
}

/// Builds and runs a C++ hello-world with `cxx` in `scratch` (created, then deleted). With
/// `rpath`, the directory of the toolchain's `libstdc++.so.6`, the program is linked with
/// `-Wl,-rpath,<rpath>` like the generated `toolchain.cmake` does, and must load its libstdc++
/// from there. `None` if all went well, else what went wrong.
pub fn compile_and_run(cxx: &Path, scratch: &Path, rpath: Option<&Path>) -> Option<String> {
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
        let mut command = Command::new(cxx);
        command.arg(&source).arg("-o").arg(&program);
        if let Some(rpath) = rpath {
            command.arg(format!("-Wl,-rpath,{}", rpath.display()));
        }
        let compile =
            run(&mut command).map_err(|e| format!("cannot run {}: {e}", cxx.display()))?;
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
        if let Some(rpath) = rpath
            && let Ok(ldd) = run(Command::new("ldd").arg(&program))
            && let Some(loaded) =
                loaded_library(&String::from_utf8_lossy(&ldd.stdout), "libstdc++.so.6")
            && loaded.parent().and_then(|dir| dir.canonicalize().ok()) != rpath.canonicalize().ok()
        {
            return Err(format!(
                "the compiled hello-world loads {} instead of the libstdc++.so.6 in {}, \
                 although it was linked with -Wl,-rpath",
                loaded.display(),
                rpath.display()
            ));
        }
        Ok(())
    })();
    let _ = fs::remove_dir_all(scratch);
    result.err()
}

fn run(command: &mut Command) -> std::io::Result<Output> {
    files::run_captured(command)
}

/// The last lines of `stderr`, indented.
pub(crate) fn tail(stderr: &[u8]) -> String {
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
        let problem = compile_and_run(&cxx, &scratch, None).unwrap();
        assert!(problem.contains("iostream not found"), "{problem}");
        assert!(!scratch.exists());
    }

    #[test]
    fn passes_the_rpath_to_the_linker() {
        let dir = tempfile::tempdir().unwrap();
        let cxx = dir.path().join("g++");
        fs::write(&cxx, "#!/bin/sh\necho \"args: $*\" >&2\nexit 1\n").unwrap();
        fs::set_permissions(&cxx, fs::Permissions::from_mode(0o755)).unwrap();
        let scratch = dir.path().join("scratch");
        let problem = compile_and_run(&cxx, &scratch, Some(Path::new("/p/lib64"))).unwrap();
        assert!(problem.contains("-Wl,-rpath,/p/lib64"), "{problem}");
        let problem = compile_and_run(&cxx, &scratch, None).unwrap();
        assert!(!problem.contains("-rpath"), "{problem}");
    }

    #[test]
    fn finds_the_library_a_program_loads() {
        let output = "\tlinux-vdso.so.1 (0x00007ffd)\n\
                      \tlibstdc++.so.6 => /s/gcc/15.2.0/lib64/libstdc++.so.6 (0x000071bc4bbd3000)\n\
                      \tlibm.so.6 => /lib/x86_64-linux-gnu/libm.so.6 (0x00007f)\n\
                      \t/lib64/ld-linux-x86-64.so.2 (0x00007f)\n";
        assert_eq!(
            loaded_library(output, "libstdc++.so.6"),
            Some(PathBuf::from("/s/gcc/15.2.0/lib64/libstdc++.so.6"))
        );
        assert_eq!(loaded_library(output, "libgcc_s.so.1"), None);
        assert_eq!(
            loaded_library("\tlibstdc++.so.6 => not found\n", "libstdc++.so.6"),
            None
        );
        assert_eq!(
            loaded_library("\tnot a dynamic executable\n", "libstdc++.so.6"),
            None
        );
    }
}
