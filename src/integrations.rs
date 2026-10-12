//! Generated project files: `toolchain.cmake`, `env`, `.gitignore` entry (M2), the pairing of
//! clang with a GCC, `CMakeUserPresets.json` and VS Code settings (M6).

mod json;
pub mod presets;
pub mod vscode;

use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::detect;
use crate::error::{Error, Result};
use crate::files;
use crate::store::Installed;
use crate::toolchain::{self, Kind};

/// Everything the generated files and `env`/`exec` need to activate a toolchain. All paths are
/// real, absolute paths, never `.cxvm/current` (PLAN.md §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activation {
    /// `gcc@13.3.0`, or `system`.
    pub label: String,
    /// Directory to prepend to `PATH`; `None` for `system`, which leaves `PATH` alone.
    pub bin: Option<PathBuf>,
    /// C compiler; `None` only for `system` without a `cc` on `PATH`.
    pub cc: Option<PathBuf>,
    /// C++ compiler; `None` only for `system` without a `c++` on `PATH`.
    pub cxx: Option<PathBuf>,
    /// Directory holding the `libstdc++.so.6` that programs must load, baked in as an rpath:
    /// the toolchain's own for GCC, the paired GCC's for clang. `None` when the dynamic
    /// loader searches it anyway.
    pub rpath: Option<PathBuf>,
    /// Clang only: the GCC it is told to use instead of the newest one on the host.
    pub pairing: Option<Pairing>,
}

/// A clang told to use one GCC's headers, libstdc++ and runtime files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pairing {
    /// The GCC, e.g. `gcc@13.3.0`.
    pub gcc: String,
    /// The driver option selecting it, which every compiler call must carry.
    pub flag: String,
}

/// The first clang release with `--gcc-install-dir`; older ones only have `--gcc-toolchain`
/// (see docs/backends.md).
const GCC_INSTALL_DIR_SINCE: u32 = 16;

impl Pairing {
    fn new(clang: &Installed, gcc: &Installed) -> Result<Self> {
        check_drivers(gcc)?;
        let cannot = |why: String, hint: &str| Error::Usage {
            message: format!("cannot pair {} with {}: {why}", clang.id, gcc.id),
            hint: format!(
                "{hint}, or set `gcc = \"system\"` in the `[clang]` table of .cxvmrc to let \
                 clang pick a GCC on the host"
            ),
        };
        let Some(install_dir) = detect::gcc_install_dir(&gcc.cc) else {
            return Err(cannot(
                format!(
                    "`{} -print-libgcc-file-name` does not name its install directory",
                    gcc.cc.display()
                ),
                "pair clang with another GCC from `cxvm list`",
            ));
        };
        if clang.id.version().major() >= GCC_INSTALL_DIR_SINCE {
            return Ok(Self {
                gcc: gcc.id.to_string(),
                flag: format!("--gcc-install-dir={}", install_dir.display()),
            });
        }

        // `--gcc-toolchain=<prefix>` makes clang search `<prefix>/lib/gcc/<triple>/` and take
        // the highest version it finds there.
        let in_lib_gcc =
            install_dir.ancestors().nth(2).and_then(Path::file_name) == Some("gcc".as_ref());
        let Some(prefix) = install_dir.ancestors().nth(4).filter(|_| in_lib_gcc) else {
            return Err(cannot(
                format!(
                    "{} is not `<prefix>/lib/gcc/<triple>/<version>`, the only layout a clang \
                     before {GCC_INSTALL_DIR_SINCE} can be pointed at",
                    install_dir.display()
                ),
                "use clang 16 or later, which has `--gcc-install-dir`",
            ));
        };
        let mut others: Vec<String> = install_dir
            .parent()
            .and_then(|triple| fs::read_dir(triple).ok())
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path() != install_dir && entry.path().join("crtbegin.o").exists())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        if !others.is_empty() {
            others.sort();
            return Err(cannot(
                format!(
                    "{} holds other GCCs too ({}), and a clang before {GCC_INSTALL_DIR_SINCE} \
                     always takes the newest in a prefix",
                    prefix.display(),
                    others.join(", ")
                ),
                "pair it with a GCC that has a prefix of its own (`cxvm install \
                 gcc@<version>`), use clang 16 or later",
            ));
        }
        Ok(Self {
            gcc: gcc.id.to_string(),
            flag: format!("--gcc-toolchain={}", prefix.display()),
        })
    }
}

/// Fails if one of the toolchain's drivers is missing.
pub(crate) fn check_drivers(toolchain: &Installed) -> Result<()> {
    let Installed {
        id, path, cc, cxx, ..
    } = toolchain;
    for driver in [cc, cxx] {
        if !files::is_executable(driver) {
            return Err(Error::Usage {
                message: format!(
                    "{id} is incomplete: {} is missing or not executable",
                    driver.display()
                ),
                hint: if toolchain.linked {
                    format!(
                        "restore the install in {}, or `cxvm unlink {id}`",
                        path.display()
                    )
                } else {
                    format!("delete {} and install it again", path.display())
                },
            });
        }
    }
    Ok(())
}

/// The directory of the `libstdc++.so.6` that `cxx` links, if programs need an rpath to load
/// that one.
fn runtime_dir(cxx: &Path) -> Option<PathBuf> {
    detect::libstdcxx_dir(cxx).filter(|dir| !is_default_library_dir(dir))
}

impl Activation {
    /// Activation for an installed or linked toolchain; `gcc` is the GCC a clang is paired
    /// with, if any. Fails if a driver is missing or the pairing cannot be expressed.
    pub fn for_toolchain(toolchain: &Installed, gcc: Option<&Installed>) -> Result<Self> {
        check_drivers(toolchain)?;
        let Installed { id, cc, cxx, .. } = toolchain;
        let (pairing, rpath) = match (id.kind(), gcc) {
            (Kind::Gcc, _) => (None, runtime_dir(cxx)),
            (Kind::Clang, None) => (None, None),
            (Kind::Clang, Some(gcc)) => {
                (Some(Pairing::new(toolchain, gcc)?), runtime_dir(&gcc.cxx))
            }
        };
        Ok(Self {
            label: id.to_string(),
            bin: cc.parent().map(Path::to_path_buf),
            cc: Some(cc.clone()),
            cxx: Some(cxx.clone()),
            rpath,
            pairing,
        })
    }

    /// The values of `CC` and `CXX`: each compiler, followed by the pairing option if there
    /// is one, as in `CC="gcc -m32"`.
    pub fn compiler_env(&self) -> impl Iterator<Item = (&'static str, OsString)> + '_ {
        [("CC", &self.cc), ("CXX", &self.cxx)]
            .into_iter()
            .filter_map(|(name, path)| {
                let mut value = path.clone()?.into_os_string();
                if let Some(pairing) = &self.pairing {
                    value.push(" ");
                    value.push(&pairing.flag);
                }
                Some((name, value))
            })
    }

    /// Activation for `system`: the `cc` and `c++` on `path_var`, skipping cxvm's own dirs.
    pub fn for_system(path_var: Option<&OsStr>, store: &Path) -> Self {
        Self {
            label: "system".into(),
            bin: None,
            cc: toolchain::find_system("cc", path_var, store),
            cxx: toolchain::find_system("c++", path_var, store),
            rpath: None,
            pairing: None,
        }
    }
}

/// Whether the dynamic loader searches `dir` without being told to, as it does for a distro
/// GCC's libstdc++: an rpath naming it would only be noise.
fn is_default_library_dir(dir: &Path) -> bool {
    const DEFAULT: [&str; 6] = [
        "/lib",
        "/lib64",
        "/lib/x86_64-linux-gnu",
        "/usr/lib",
        "/usr/lib64",
        "/usr/lib/x86_64-linux-gnu",
    ];
    DEFAULT.iter().any(|default| dir == Path::new(default))
}

/// Shell syntax for [`render_env`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    /// bash, zsh, dash, …
    Posix,
    Fish,
}

/// The CMake toolchain file from PLAN.md §4.
pub fn render_toolchain_cmake(activation: &Activation) -> String {
    let label = &activation.label;
    let mut out = format!(
        "# Generated by cxvm — do not edit. Toolchain: {label}\n\
         # Regenerate with `cxvm use <id>` or `cxvm install`.\n\
         set(CXVM_TOOLCHAIN {})\n",
        cmake_string(label)
    );
    // The option rides with the compiler itself, not in `CMAKE_<LANG>_FLAGS_INIT`, which a
    // `-DCMAKE_CXX_FLAGS=…` on the command line would silently replace.
    let option = match &activation.pairing {
        Some(Pairing { gcc, flag }) => {
            writeln!(
                out,
                "# Clang uses the headers and libstdc++ of {gcc} (`[clang] gcc` in .cxvmrc)"
            )
            .expect("writing to a String cannot fail");
            format!(" {}", cmake_string(flag))
        }
        None => String::new(),
    };
    for (variable, name, compiler) in [
        ("CMAKE_C_COMPILER  ", "cc", &activation.cc),
        ("CMAKE_CXX_COMPILER", "c++", &activation.cxx),
    ] {
        match compiler {
            Some(path) => writeln!(out, "set({variable} {}{option})", cmake_path(path)),
            None => writeln!(
                out,
                "# no `{name}` found on PATH when this was generated; CMake picks the compiler"
            ),
        }
        .expect("writing to a String cannot fail");
    }
    if let Some(rpath) = &activation.rpath {
        let flag = cmake_string(&format!("-Wl,-rpath,{}", rpath.display()));
        let whose = match &activation.pairing {
            Some(pairing) => format!("the libstdc++ of {}", pairing.gcc),
            None => "this toolchain's libstdc++".to_owned(),
        };
        writeln!(
            out,
            "# Programs must load {whose} at runtime, not the system one\n\
             set(CMAKE_EXE_LINKER_FLAGS_INIT    {flag})\n\
             set(CMAKE_SHARED_LINKER_FLAGS_INIT {flag})\n\
             set(CMAKE_MODULE_LINKER_FLAGS_INIT {flag})"
        )
        .expect("writing to a String cannot fail");
    }
    out
}

/// A CMake quoted argument.
fn cmake_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if matches!(c, '\\' | '"' | '$') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

fn cmake_path(path: &Path) -> String {
    cmake_string(&path.display().to_string())
}

/// Exports that activate the toolchain in `shell`.
pub fn render_env(activation: &Activation, shell: Shell) -> String {
    let set = |name: &str, value: &str| {
        let value = quote(shell, value);
        match shell {
            Shell::Posix => format!("export {name}={value}\n"),
            Shell::Fish => format!("set -gx {name} {value}\n"),
        }
    };
    let mut out = set("CXVM_TOOLCHAIN", &activation.label);
    if let Some(bin) = &activation.bin {
        let bin = quote(shell, &bin.display().to_string());
        out += &match shell {
            Shell::Posix => format!("export PATH={bin}:\"$PATH\"\n"),
            Shell::Fish => format!("set -gx PATH {bin} $PATH\n"),
        };
    }
    for (name, value) in activation.compiler_env() {
        out += &set(name, &value.to_string_lossy());
    }
    out
}

/// Name of the generated, gitignored per-project folder.
pub const PROJECT_DIR: &str = ".cxvm";

/// `label` as a file or directory name: `gcc-13.3.0` for `gcc@13.3.0`. It names the build
/// directory cxvm suggests for a toolchain, `build/<slug>`.
pub fn slug(label: &str) -> String {
    label.replace('@', "-")
}

/// A file that an integration wants written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEdit {
    pub path: PathBuf,
    pub text: String,
    /// What to tell the user once it is written, if anything.
    pub summary: Option<String>,
}

impl FileEdit {
    /// Replaces the file atomically, creating its directory if needed.
    pub fn write(&self) -> Result<()> {
        files::write_atomic(&self.path, &self.text)
    }
}

/// Writes `<root>/.cxvm/`: `toolchain.cmake`, `env`, and `current` as a symlink to `prefix`
/// (removed when `prefix` is `None`, i.e. for `system`).
pub fn write_project_dir(
    root: &Path,
    activation: &Activation,
    prefix: Option<&Path>,
) -> Result<()> {
    let dir = root.join(PROJECT_DIR);
    files::write_atomic(
        &dir.join("toolchain.cmake"),
        &render_toolchain_cmake(activation),
    )?;
    files::write_atomic(
        &dir.join("env"),
        &format!(
            "# Generated by cxvm — do not edit. Toolchain: {}\n\
             # Source it from a POSIX shell: `. {PROJECT_DIR}/env`\n{}",
            activation.label,
            render_env(activation, Shell::Posix)
        ),
    )?;

    let current = dir.join("current");
    let io_error = |action, path: &Path| {
        let path = path.to_owned();
        move |source| Error::Io {
            action,
            path,
            source,
            hint: "cxvm owns the .cxvm folder; move anything you keep there elsewhere",
        }
    };
    match prefix {
        Some(prefix) => {
            let tmp = dir.join(format!(".current.tmp.{}", std::process::id()));
            let _ = fs::remove_file(&tmp);
            std::os::unix::fs::symlink(prefix, &tmp).map_err(io_error("create", &tmp))?;
            fs::rename(&tmp, &current).map_err(|err| {
                let _ = fs::remove_file(&tmp);
                io_error("replace", &current)(err)
            })
        }
        None => match current.symlink_metadata() {
            Ok(meta) if meta.is_symlink() => {
                fs::remove_file(&current).map_err(io_error("remove", &current))
            }
            Ok(_) => Err(io_error("remove", &current)(io::Error::other(
                "it is not a symlink",
            ))),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(io_error("inspect", &current)(err)),
        },
    }
}

/// What [`ensure_gitignored`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gitignore {
    /// `.cxvm/` was appended to this file.
    Added(PathBuf),
    /// Git, or the project's `.gitignore`, already ignores `.cxvm/`.
    AlreadyIgnored,
}

/// Makes sure `<root>/.cxvm/` is ignored by git: asks `git check-ignore` when `root` is in a
/// repository, else looks at `<root>/.gitignore`; appends `.cxvm/` there if needed.
pub fn ensure_gitignored(root: &Path) -> Result<Gitignore> {
    let entry = format!("{PROJECT_DIR}/");
    let git_says_ignored = files::run_captured(Command::new("git").arg("-C").arg(root).args([
        "check-ignore",
        "-q",
        &entry,
    ]))
    .is_ok_and(|output| output.status.success());
    if git_says_ignored {
        return Ok(Gitignore::AlreadyIgnored);
    }

    let path = root.join(".gitignore");
    let mut text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(Error::Io {
                action: "read",
                path,
                source,
                hint: "make sure it is a readable file",
            });
        }
    };
    let listed = text
        .lines()
        .any(|line| matches!(line.trim(), ".cxvm" | ".cxvm/" | "/.cxvm" | "/.cxvm/"));
    if listed {
        return Ok(Gitignore::AlreadyIgnored);
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&entry);
    text.push('\n');
    files::write_atomic(&path, &text)?;
    Ok(Gitignore::Added(path))
}

/// `s` as a single-quoted word in `shell`.
fn quote(shell: Shell, s: &str) -> String {
    match shell {
        Shell::Posix => format!("'{}'", s.replace('\'', r"'\''")),
        Shell::Fish => format!("'{}'", s.replace('\\', r"\\").replace('\'', r"\'")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gcc() -> Activation {
        Activation {
            label: "gcc@13.3.0".into(),
            bin: Some("/s/toolchains/gcc/13.3.0/bin".into()),
            cc: Some("/s/toolchains/gcc/13.3.0/bin/gcc".into()),
            cxx: Some("/s/toolchains/gcc/13.3.0/bin/g++".into()),
            rpath: Some("/s/toolchains/gcc/13.3.0/lib64".into()),
            pairing: None,
        }
    }

    fn system() -> Activation {
        Activation {
            label: "system".into(),
            bin: None,
            cc: Some("/usr/bin/cc".into()),
            cxx: Some("/usr/bin/c++".into()),
            rpath: None,
            pairing: None,
        }
    }

    #[test]
    fn toolchain_file_for_gcc_matches_the_plan() {
        assert_eq!(
            render_toolchain_cmake(&gcc()),
            "# Generated by cxvm — do not edit. Toolchain: gcc@13.3.0\n\
             # Regenerate with `cxvm use <id>` or `cxvm install`.\n\
             set(CXVM_TOOLCHAIN \"gcc@13.3.0\")\n\
             set(CMAKE_C_COMPILER   \"/s/toolchains/gcc/13.3.0/bin/gcc\")\n\
             set(CMAKE_CXX_COMPILER \"/s/toolchains/gcc/13.3.0/bin/g++\")\n\
             # Programs must load this toolchain's libstdc++ at runtime, not the system one\n\
             set(CMAKE_EXE_LINKER_FLAGS_INIT    \"-Wl,-rpath,/s/toolchains/gcc/13.3.0/lib64\")\n\
             set(CMAKE_SHARED_LINKER_FLAGS_INIT \"-Wl,-rpath,/s/toolchains/gcc/13.3.0/lib64\")\n\
             set(CMAKE_MODULE_LINKER_FLAGS_INIT \"-Wl,-rpath,/s/toolchains/gcc/13.3.0/lib64\")\n"
        );
    }

    #[test]
    fn default_library_dirs_need_no_rpath() {
        for dir in ["/usr/lib/x86_64-linux-gnu", "/usr/lib64", "/lib"] {
            assert!(is_default_library_dir(Path::new(dir)), "{dir}");
        }
        for dir in [
            "/s/toolchains/gcc/13.3.0/lib64",
            "/opt/gcc-11/lib64",
            "/usr/lib/gcc/x86_64-linux-gnu/15",
            "/usr/local/lib64",
        ] {
            assert!(!is_default_library_dir(Path::new(dir)), "{dir}");
        }
    }

    #[test]
    fn toolchain_file_without_rpath_has_no_linker_flags() {
        let clang = Activation {
            label: "clang@18.1.8".into(),
            bin: Some("/s/clang/bin".into()),
            cc: Some("/s/clang/bin/clang".into()),
            cxx: Some("/s/clang/bin/clang++".into()),
            rpath: None,
            pairing: None,
        };
        let text = render_toolchain_cmake(&clang);
        assert!(text.contains("set(CMAKE_CXX_COMPILER \"/s/clang/bin/clang++\")\n"));
        assert!(!text.contains("LINKER_FLAGS"), "{text}");
    }

    #[test]
    fn toolchain_file_for_a_paired_clang_passes_the_option_with_the_compiler() {
        let clang = Activation {
            label: "clang@17.0.6".into(),
            bin: Some("/s/clang/bin".into()),
            cc: Some("/s/clang/bin/clang".into()),
            cxx: Some("/s/clang/bin/clang++".into()),
            rpath: Some("/s/gcc/lib64".into()),
            pairing: Some(Pairing {
                gcc: "gcc@13.3.0".into(),
                flag: "--gcc-install-dir=/s/gcc/lib/gcc/x86_64-pc-linux-gnu/13.3.0".into(),
            }),
        };
        assert_eq!(
            render_toolchain_cmake(&clang),
            "# Generated by cxvm — do not edit. Toolchain: clang@17.0.6\n\
             # Regenerate with `cxvm use <id>` or `cxvm install`.\n\
             set(CXVM_TOOLCHAIN \"clang@17.0.6\")\n\
             # Clang uses the headers and libstdc++ of gcc@13.3.0 (`[clang] gcc` in .cxvmrc)\n\
             set(CMAKE_C_COMPILER   \"/s/clang/bin/clang\" \
             \"--gcc-install-dir=/s/gcc/lib/gcc/x86_64-pc-linux-gnu/13.3.0\")\n\
             set(CMAKE_CXX_COMPILER \"/s/clang/bin/clang++\" \
             \"--gcc-install-dir=/s/gcc/lib/gcc/x86_64-pc-linux-gnu/13.3.0\")\n\
             # Programs must load the libstdc++ of gcc@13.3.0 at runtime, not the system one\n\
             set(CMAKE_EXE_LINKER_FLAGS_INIT    \"-Wl,-rpath,/s/gcc/lib64\")\n\
             set(CMAKE_SHARED_LINKER_FLAGS_INIT \"-Wl,-rpath,/s/gcc/lib64\")\n\
             set(CMAKE_MODULE_LINKER_FLAGS_INIT \"-Wl,-rpath,/s/gcc/lib64\")\n"
        );
        assert_eq!(
            render_env(&clang, Shell::Posix),
            "export CXVM_TOOLCHAIN='clang@17.0.6'\n\
             export PATH='/s/clang/bin':\"$PATH\"\n\
             export CC='/s/clang/bin/clang \
             --gcc-install-dir=/s/gcc/lib/gcc/x86_64-pc-linux-gnu/13.3.0'\n\
             export CXX='/s/clang/bin/clang++ \
             --gcc-install-dir=/s/gcc/lib/gcc/x86_64-pc-linux-gnu/13.3.0'\n"
        );
    }

    #[test]
    fn toolchain_file_for_system_without_compilers_leaves_them_to_cmake() {
        let none = Activation {
            cc: None,
            cxx: None,
            ..system()
        };
        let text = render_toolchain_cmake(&none);
        assert!(text.contains("set(CXVM_TOOLCHAIN \"system\")"), "{text}");
        assert!(!text.contains("CMAKE_C_COMPILER"), "{text}");
        assert!(text.contains("no `cc` found on PATH"), "{text}");
        assert!(text.contains("no `c++` found on PATH"), "{text}");
    }

    #[test]
    fn toolchain_file_escapes_cmake_strings() {
        let odd = Activation {
            cc: Some(r#"/a "b"/c\d/${x}/gcc"#.into()),
            ..gcc()
        };
        let text = render_toolchain_cmake(&odd);
        assert!(
            text.contains(r#"set(CMAKE_C_COMPILER   "/a \"b\"/c\\d/\${x}/gcc")"#),
            "{text}"
        );
    }

    #[test]
    fn posix_env_for_a_toolchain() {
        assert_eq!(
            render_env(&gcc(), Shell::Posix),
            "export CXVM_TOOLCHAIN='gcc@13.3.0'\n\
             export PATH='/s/toolchains/gcc/13.3.0/bin':\"$PATH\"\n\
             export CC='/s/toolchains/gcc/13.3.0/bin/gcc'\n\
             export CXX='/s/toolchains/gcc/13.3.0/bin/g++'\n"
        );
    }

    #[test]
    fn fish_env_for_a_toolchain() {
        assert_eq!(
            render_env(&gcc(), Shell::Fish),
            "set -gx CXVM_TOOLCHAIN 'gcc@13.3.0'\n\
             set -gx PATH '/s/toolchains/gcc/13.3.0/bin' $PATH\n\
             set -gx CC '/s/toolchains/gcc/13.3.0/bin/gcc'\n\
             set -gx CXX '/s/toolchains/gcc/13.3.0/bin/g++'\n"
        );
    }

    #[test]
    fn system_env_leaves_path_alone() {
        assert_eq!(
            render_env(&system(), Shell::Posix),
            "export CXVM_TOOLCHAIN='system'\n\
             export CC='/usr/bin/cc'\n\
             export CXX='/usr/bin/c++'\n"
        );
    }

    mod project_dir {
        use std::fs;
        use std::process::Command;

        use super::*;

        #[test]
        fn writes_the_toolchain_file_env_and_current_link() {
            let dir = tempfile::tempdir().unwrap();
            let prefix = dir.path().join("store/gcc/13.3.0");
            write_project_dir(dir.path(), &gcc(), Some(&prefix)).unwrap();
            let project_dir = dir.path().join(PROJECT_DIR);
            assert_eq!(
                fs::read_to_string(project_dir.join("toolchain.cmake")).unwrap(),
                render_toolchain_cmake(&gcc())
            );
            let env = fs::read_to_string(project_dir.join("env")).unwrap();
            assert!(env.starts_with("# Generated by cxvm"), "{env}");
            assert!(env.ends_with(&render_env(&gcc(), Shell::Posix)), "{env}");
            assert_eq!(fs::read_link(project_dir.join("current")).unwrap(), prefix);
        }

        #[test]
        fn rewriting_replaces_everything_and_leaves_no_temp_files() {
            let dir = tempfile::tempdir().unwrap();
            write_project_dir(dir.path(), &gcc(), Some(Path::new("/old"))).unwrap();
            write_project_dir(dir.path(), &gcc(), Some(Path::new("/new"))).unwrap();
            let project_dir = dir.path().join(PROJECT_DIR);
            assert_eq!(
                fs::read_link(project_dir.join("current")).unwrap(),
                Path::new("/new")
            );
            let mut names: Vec<_> = fs::read_dir(&project_dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect();
            names.sort();
            assert_eq!(names, ["current", "env", "toolchain.cmake"]);
        }

        #[test]
        fn system_removes_the_current_link() {
            let dir = tempfile::tempdir().unwrap();
            write_project_dir(dir.path(), &gcc(), Some(Path::new("/old"))).unwrap();
            write_project_dir(dir.path(), &system(), None).unwrap();
            let current = dir.path().join(PROJECT_DIR).join("current");
            assert!(current.symlink_metadata().is_err());
            write_project_dir(dir.path(), &system(), None).unwrap();
        }

        #[test]
        fn a_real_directory_named_current_is_an_error() {
            let dir = tempfile::tempdir().unwrap();
            let current = dir.path().join(PROJECT_DIR).join("current");
            fs::create_dir_all(current.join("keep")).unwrap();
            let err = write_project_dir(dir.path(), &gcc(), Some(Path::new("/new")))
                .unwrap_err()
                .to_string();
            assert!(err.contains(&current.display().to_string()), "{err}");
            let err = write_project_dir(dir.path(), &system(), None)
                .unwrap_err()
                .to_string();
            assert!(err.contains(&current.display().to_string()), "{err}");
            assert!(current.join("keep").exists());
        }

        #[test]
        fn outside_git_creates_a_gitignore_once() {
            let dir = tempfile::tempdir().unwrap();
            let gitignore = dir.path().join(".gitignore");
            assert_eq!(
                ensure_gitignored(dir.path()).unwrap(),
                Gitignore::Added(gitignore.clone())
            );
            assert_eq!(
                ensure_gitignored(dir.path()).unwrap(),
                Gitignore::AlreadyIgnored
            );
            assert_eq!(fs::read_to_string(&gitignore).unwrap(), ".cxvm/\n");
        }

        #[test]
        fn appends_to_an_existing_gitignore_without_trailing_newline() {
            let dir = tempfile::tempdir().unwrap();
            let gitignore = dir.path().join(".gitignore");
            fs::write(&gitignore, "/target").unwrap();
            ensure_gitignored(dir.path()).unwrap();
            assert_eq!(fs::read_to_string(&gitignore).unwrap(), "/target\n.cxvm/\n");
        }

        #[test]
        fn existing_entries_in_any_spelling_are_kept() {
            for entry in [".cxvm", ".cxvm/", "/.cxvm", "/.cxvm/", "  .cxvm/  "] {
                let dir = tempfile::tempdir().unwrap();
                let gitignore = dir.path().join(".gitignore");
                fs::write(&gitignore, format!("/target\n{entry}\n")).unwrap();
                assert_eq!(
                    ensure_gitignored(dir.path()).unwrap(),
                    Gitignore::AlreadyIgnored,
                    "{entry:?}"
                );
            }
        }

        fn git(dir: &Path, args: &[&str]) -> bool {
            Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .is_ok_and(|out| out.status.success())
        }

        #[test]
        fn a_rule_elsewhere_in_the_repository_is_respected() {
            let dir = tempfile::tempdir().unwrap();
            if !git(dir.path(), &["init", "-q"]) {
                eprintln!("git not available; skipping");
                return;
            }
            fs::write(dir.path().join(".gitignore"), ".cxvm/\n").unwrap();
            let project = dir.path().join("sub/project");
            fs::create_dir_all(&project).unwrap();
            assert_eq!(
                ensure_gitignored(&project).unwrap(),
                Gitignore::AlreadyIgnored
            );
            assert!(!project.join(".gitignore").exists());
        }

        #[test]
        fn inside_git_the_project_gitignore_gets_the_entry() {
            let dir = tempfile::tempdir().unwrap();
            if !git(dir.path(), &["init", "-q"]) {
                eprintln!("git not available; skipping");
                return;
            }
            let project = dir.path().join("sub");
            fs::create_dir_all(&project).unwrap();
            assert_eq!(
                ensure_gitignored(&project).unwrap(),
                Gitignore::Added(project.join(".gitignore"))
            );
            assert!(git(&project, &["check-ignore", "-q", ".cxvm/"]));
        }
    }

    #[test]
    fn env_quotes_awkward_paths() {
        let odd = Activation {
            cc: Some("/it's here/gcc".into()),
            cxx: Some(r"/back\slash/g++".into()),
            ..gcc()
        };
        let posix = render_env(&odd, Shell::Posix);
        assert!(posix.contains(r"export CC='/it'\''s here/gcc'"), "{posix}");
        assert!(posix.contains(r"export CXX='/back\slash/g++'"), "{posix}");
        let fish = render_env(&odd, Shell::Fish);
        assert!(fish.contains(r"set -gx CC '/it\'s here/gcc'"), "{fish}");
        assert!(fish.contains(r"set -gx CXX '/back\\slash/g++'"), "{fish}");
    }
}
