//! CMake build directories in a project, and the compilers they build with.
//!
//! CMake records the compiler of a build directory when it first configures it, and keeps it:
//! with CMake 4.2 a changed toolchain file, even passed again with `-DCMAKE_TOOLCHAIN_FILE`,
//! does not change it. A compiler set in a toolchain file is not in `CMakeCache.txt`
//! either, only in `CMakeFiles/<version>/CMake<LANG>Compiler.cmake`, which is read first.

use std::fs;
use std::path::{Path, PathBuf};

use crate::project;

/// A language CMake records a compiler for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    C,
    Cxx,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::Cxx, Lang::C];

    /// As in CMake's variable names: `C`, `CXX`.
    fn cmake(self) -> &'static str {
        match self {
            Lang::C => "C",
            Lang::Cxx => "CXX",
        }
    }

    /// For people: `C`, `C++`.
    pub fn name(self) -> &'static str {
        match self {
            Lang::C => "C",
            Lang::Cxx => "C++",
        }
    }
}

/// A compiler as CMake runs it: the program, and the argument that goes with it every time
/// (`CMAKE_<LANG>_COMPILER_ARG1`, where the pairing option of a clang ends up).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compiler {
    pub path: PathBuf,
    /// Trimmed; empty for none.
    pub arg: String,
}

impl Compiler {
    /// Whether `self` and `other` run the same program with the same argument.
    pub fn same_as(&self, other: &Compiler) -> bool {
        let resolve = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_owned());
        resolve(&self.path) == resolve(&other.path) && self.arg == other.arg
    }
}

impl std::fmt::Display for Compiler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.path.display())?;
        if !self.arg.is_empty() {
            write!(f, " {}", self.arg)?;
        }
        Ok(())
    }
}

/// A configured CMake build directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildDir {
    pub path: PathBuf,
    /// `CMAKE_TOOLCHAIN_FILE` of its cache.
    pub toolchain_file: Option<PathBuf>,
    /// The compiler of each language it was configured for, C++ first.
    pub compilers: Vec<(Lang, Compiler)>,
}

/// How deep below the project root build directories are looked for: `build/<toolchain>` is
/// two levels down, `out/build/<preset>` three.
const MAX_DEPTH: usize = 3;

/// The CMake build directories of the project at `root`: directories up to [`MAX_DEPTH`]
/// below it, `root` included, that hold a `CMakeCache.txt`. Hidden directories, symlinks and
/// projects of their own (with a `.cxvmrc`) are left out, and a build directory is not
/// searched further.
pub fn build_dirs(root: &Path) -> Vec<BuildDir> {
    let mut found = Vec::new();
    let mut pending = vec![(root.to_owned(), 0)];
    while let Some((dir, depth)) = pending.pop() {
        if dir.join("CMakeCache.txt").is_file() {
            found.extend(read(&dir));
            continue;
        }
        if depth == MAX_DEPTH {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(|entry| entry.ok()) {
            let path = entry.path();
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
            if is_dir && !hidden && !path.join(project::RC_FILE).exists() {
                pending.push((path, depth + 1));
            }
        }
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

/// The build directory at `dir`, from its `CMakeCache.txt`; `None` if that cannot be read.
pub fn read(dir: &Path) -> Option<BuildDir> {
    let cache = fs::read_to_string(dir.join("CMakeCache.txt")).ok()?;
    let entry = |name: &str| cache_entry(&cache, name);
    let version = ["MAJOR", "MINOR", "PATCH"]
        .map(|part| entry(&format!("CMAKE_CACHE_{part}_VERSION")).unwrap_or_default())
        .join(".");
    let mut compilers = Vec::new();
    for lang in Lang::ALL {
        let variable = format!("CMAKE_{}_COMPILER", lang.cmake());
        let recorded = dir
            .join("CMakeFiles")
            .join(&version)
            .join(format!("CMake{}Compiler.cmake", lang.cmake()));
        let from_file = fs::read_to_string(recorded).ok().and_then(|text| {
            let path = set_values(&text, &variable)?.into_iter().next()?;
            let arg = set_values(&text, &format!("{variable}_ARG1"))
                .and_then(|values| values.into_iter().next())
                .unwrap_or_default();
            Some((path, arg))
        });
        let recorded = from_file.or_else(|| {
            let path = entry(&variable)?;
            Some((path, entry(&format!("{variable}_ARG1")).unwrap_or_default()))
        });
        if let Some((path, arg)) = recorded.filter(|(path, _)| !path.is_empty()) {
            let compiler = Compiler {
                path: path.into(),
                arg: arg.trim().to_owned(),
            };
            compilers.push((lang, compiler));
        }
    }
    // A relative toolchain file is looked up in the build tree, then in the source tree.
    let toolchain_file = entry("CMAKE_TOOLCHAIN_FILE")
        .filter(|path| !path.is_empty())
        .map(|file| {
            let file = PathBuf::from(file);
            if file.is_absolute() {
                return file;
            }
            let in_build = dir.join(&file);
            match entry("CMAKE_HOME_DIRECTORY") {
                Some(source) if !in_build.exists() => Path::new(&source).join(file),
                _ => in_build,
            }
        });
    Some(BuildDir {
        path: dir.to_owned(),
        toolchain_file,
        compilers,
    })
}

/// The compilers a CMake toolchain file sets, by language.
pub fn toolchain_compilers(text: &str) -> Vec<(Lang, Compiler)> {
    Lang::ALL
        .into_iter()
        .filter_map(|lang| {
            let mut values = set_values(text, &format!("CMAKE_{}_COMPILER", lang.cmake()))?;
            let path = values.remove(0);
            let compiler = Compiler {
                path: path.into(),
                arg: values.join(" ").trim().to_owned(),
            };
            Some((lang, compiler))
        })
        .collect()
}

/// The value of `name` in a `CMakeCache.txt`, from its `NAME:TYPE=value` line.
fn cache_entry(cache: &str, name: &str) -> Option<String> {
    cache.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        let (key, _type) = key.split_once(':')?;
        (key == name).then(|| value.to_owned())
    })
}

/// The quoted values of the first `set(<variable> "…" …)` in `text`, unescaped; `None`
/// without one.
fn set_values(text: &str, variable: &str) -> Option<Vec<String>> {
    text.lines().find_map(|line| {
        let rest = line
            .trim_start()
            .strip_prefix("set(")?
            .strip_prefix(variable)?;
        if !rest.starts_with([' ', '\t']) {
            return None;
        }
        let mut values = Vec::new();
        let mut chars = rest.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => {
                    let mut value = String::new();
                    while let Some(c) = chars.next() {
                        match c {
                            '\\' => value.extend(chars.next()),
                            '"' => break,
                            c => value.push(c),
                        }
                    }
                    values.push(value);
                }
                ')' => break,
                _ => {}
            }
        }
        (!values.is_empty()).then_some(values)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A build dir configured by CMake 4.2.3 with a toolchain file: the compilers are in
    /// `CMakeFiles/4.2.3/`, not in the cache.
    fn configured(dir: &Path, toolchain_file: Option<&str>, cxx: &str, cxx_arg: &str) {
        fs::create_dir_all(dir.join("CMakeFiles/4.2.3")).unwrap();
        let mut cache = String::from(
            "# This is the CMakeCache file.\n\
             CMAKE_CACHE_MAJOR_VERSION:INTERNAL=4\n\
             CMAKE_CACHE_MINOR_VERSION:INTERNAL=2\n\
             CMAKE_CACHE_PATCH_VERSION:INTERNAL=3\n\
             CMAKE_CXX_COMPILER_AR:FILEPATH=/usr/bin/gcc-ar-15\n",
        );
        if let Some(file) = toolchain_file {
            cache.push_str(&format!("CMAKE_TOOLCHAIN_FILE:FILEPATH={file}\n"));
        }
        fs::write(dir.join("CMakeCache.txt"), cache).unwrap();
        fs::write(
            dir.join("CMakeFiles/4.2.3/CMakeCXXCompiler.cmake"),
            format!(
                "set(CMAKE_CXX_COMPILER \"{cxx}\")\nset(CMAKE_CXX_COMPILER_ARG1 \"{cxx_arg}\")\n\
                 set(CMAKE_CXX_COMPILER_ID \"GNU\")\n"
            ),
        )
        .unwrap();
    }

    #[test]
    fn reads_the_compiler_cmake_recorded_and_its_argument() {
        let dir = tempfile::tempdir().unwrap();
        configured(
            dir.path(),
            Some("/p/.cxvm/toolchain.cmake"),
            "/s/clang/bin/clang++",
            "--gcc-install-dir=/s/gcc/lib/gcc/x86_64-pc-linux-gnu/13.3.0",
        );
        let build = read(dir.path()).unwrap();
        assert_eq!(
            build.toolchain_file.as_deref(),
            Some(Path::new("/p/.cxvm/toolchain.cmake"))
        );
        assert_eq!(
            build.compilers,
            [(
                Lang::Cxx,
                Compiler {
                    path: "/s/clang/bin/clang++".into(),
                    arg: "--gcc-install-dir=/s/gcc/lib/gcc/x86_64-pc-linux-gnu/13.3.0".into(),
                }
            )]
        );
    }

    #[test]
    fn a_relative_toolchain_file_is_found_like_cmake_finds_it() {
        let dir = tempfile::tempdir().unwrap();
        let build = dir.path().join("build/x");
        configured(&build, Some(".cxvm/toolchain.cmake"), "/usr/bin/g++", "");
        let cache = build.join("CMakeCache.txt");
        let text = fs::read_to_string(&cache).unwrap();
        let source = format!("CMAKE_HOME_DIRECTORY:INTERNAL={}\n", dir.path().display());
        fs::write(&cache, text + &source).unwrap();
        assert_eq!(
            read(&build).unwrap().toolchain_file,
            Some(dir.path().join(".cxvm/toolchain.cmake"))
        );
        fs::create_dir_all(build.join(".cxvm")).unwrap();
        fs::write(build.join(".cxvm/toolchain.cmake"), "").unwrap();
        assert_eq!(
            read(&build).unwrap().toolchain_file,
            Some(build.join(".cxvm/toolchain.cmake"))
        );
    }

    #[test]
    fn falls_back_to_the_cache_without_cmakefiles() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("CMakeCache.txt"),
            "CMAKE_C_COMPILER:FILEPATH=/usr/bin/cc\n\
             CMAKE_CXX_COMPILER:FILEPATH=/usr/bin/clang++\n\
             CMAKE_CXX_COMPILER_ARG1:STRING= -v\n",
        )
        .unwrap();
        let build = read(dir.path()).unwrap();
        assert_eq!(build.toolchain_file, None);
        assert_eq!(
            build.compilers,
            [
                (
                    Lang::Cxx,
                    Compiler {
                        path: "/usr/bin/clang++".into(),
                        arg: "-v".into()
                    }
                ),
                (
                    Lang::C,
                    Compiler {
                        path: "/usr/bin/cc".into(),
                        arg: String::new()
                    }
                ),
            ]
        );
    }

    #[test]
    fn reads_the_compilers_of_a_generated_toolchain_file() {
        let text = "# Generated by cxvm — do not edit. Toolchain: clang@17.0.6\n\
                    set(CXVM_TOOLCHAIN \"clang@17.0.6\")\n\
                    set(CMAKE_C_COMPILER   \"/s/clang/bin/clang\" \"--gcc-install-dir=/g\")\n\
                    set(CMAKE_CXX_COMPILER \"/s/a \\\"b\\\"/clang++\" \"--gcc-install-dir=/g\")\n\
                    set(CMAKE_CXX_COMPILER_LAUNCHER \"ccache\")\n";
        assert_eq!(
            toolchain_compilers(text),
            [
                (
                    Lang::Cxx,
                    Compiler {
                        path: "/s/a \"b\"/clang++".into(),
                        arg: "--gcc-install-dir=/g".into()
                    }
                ),
                (
                    Lang::C,
                    Compiler {
                        path: "/s/clang/bin/clang".into(),
                        arg: "--gcc-install-dir=/g".into()
                    }
                ),
            ]
        );
        assert!(toolchain_compilers("# no compilers on PATH\n").is_empty());
    }

    #[test]
    fn finds_build_dirs_where_people_put_them() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for build in [
            "build/gcc-13.3.0",
            "build-arm",
            "out/build/dev",
            "cmake-build-debug",
        ] {
            configured(&root.join(build), None, "/usr/bin/c++", "");
        }
        // Not searched: too deep, hidden, another project, inside a build dir.
        configured(&root.join("a/b/c/build"), None, "/usr/bin/c++", "");
        configured(&root.join(".git/build"), None, "/usr/bin/c++", "");
        configured(
            &root.join("third_party/lib/build"),
            None,
            "/usr/bin/c++",
            "",
        );
        fs::write(root.join("third_party/lib/.cxvmrc"), "").unwrap();
        configured(&root.join("build-arm/sub"), None, "/usr/bin/c++", "");
        let found: Vec<PathBuf> = build_dirs(root)
            .into_iter()
            .map(|build| build.path.strip_prefix(root).unwrap().to_owned())
            .collect();
        assert_eq!(
            found,
            [
                Path::new("build/gcc-13.3.0"),
                Path::new("build-arm"),
                Path::new("cmake-build-debug"),
                Path::new("out/build/dev"),
            ]
        );
    }

    #[test]
    fn compilers_compare_by_the_file_they_run_and_the_argument() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("g++-15");
        fs::write(&real, "").unwrap();
        let link = dir.path().join("c++");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let compiler = |path: &Path, arg: &str| Compiler {
            path: path.to_owned(),
            arg: arg.into(),
        };
        assert!(compiler(&link, "").same_as(&compiler(&real, "")));
        assert!(!compiler(&link, "-m32").same_as(&compiler(&real, "")));
        assert!(!compiler(&link, "").same_as(&compiler(&dir.path().join("clang++"), "")));
    }
}
