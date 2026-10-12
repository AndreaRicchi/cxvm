//! `.vscode/settings.json`: clangd and CMake Tools settings that follow the pin (opt-in).

use std::path::Path;

use jsonc_parser::cst::{CstArray, CstInputValue, CstStringLit};

use super::json::{self, Document};
use super::{Activation, FileEdit, PROJECT_DIR, slug};
use crate::error::Result;
use crate::files;
use crate::toolchain::{Kind, ToolchainId};

/// The workspace settings file, relative to the project root.
pub const FILE: &str = ".vscode/settings.json";

const OFF: &str = "set `vscode = false` in the `[integrations]` table of .cxvmrc";

const QUERY_DRIVER: &str = "--query-driver=";
const COMPILE_COMMANDS_DIR: &str = "--compile-commands-dir=";

/// What the settings should say for the current pin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// Glob of the drivers clangd may ask for their built-in include directories: the bin
    /// directory of a GCC. `None` for clang and `system`: clangd understands a clang driver
    /// by itself, and asking one would lose the option that pairs it with a GCC.
    pub query_driver: Option<String>,
    /// Globs an earlier pin may have left: the bin directories of the toolchains cxvm knows.
    pub known_drivers: Vec<String>,
    /// The store's `toolchains/` directory with a trailing `/`: any glob below it is cxvm's.
    pub store_prefix: String,
    /// `build/<slug>`, relative to the workspace folder.
    pub build_dir: String,
}

impl Request {
    /// The request for `activation`; `toolchains` are the bin directories of every installed
    /// or linked toolchain, `store` the store's `toolchains/` directory.
    pub fn new<'a>(
        activation: &Activation,
        toolchains: impl IntoIterator<Item = &'a Path>,
        store: &Path,
    ) -> Self {
        let glob = |bin: &Path| format!("{}/*", bin.display());
        let is_gcc = activation
            .label
            .parse::<ToolchainId>()
            .is_ok_and(|id| id.kind() == Kind::Gcc);
        Self {
            query_driver: activation.bin.as_deref().filter(|_| is_gcc).map(glob),
            known_drivers: toolchains.into_iter().map(glob).collect(),
            store_prefix: format!("{}/", store.display()),
            build_dir: format!("build/{}", slug(&activation.label)),
        }
    }

    /// Whether cxvm wrote `glob` for some pin.
    fn owns(&self, glob: &str) -> bool {
        glob.starts_with(&self.store_prefix) || self.known_drivers.iter().any(|known| known == glob)
    }
}

/// The settings file of the project at `root` brought up to date for `request`, if it needs
/// writing, and notes about settings cxvm found set by someone else and left alone.
pub fn plan(root: &Path, request: &Request) -> Result<(Option<FileEdit>, Vec<String>)> {
    let path = root.join(FILE);
    let old = files::read_text(&path)?.unwrap_or_default();
    let (new, notes) = merge(&path, &old, request)?;
    let edit = (new != old).then(|| FileEdit {
        summary: Some(format!("Updated {}", path.display())),
        path,
        text: new,
    });
    Ok((edit, notes))
}

/// `text`, the content of the settings file at `path`, with cxvm's settings brought up to
/// date, and the notes. Everything else stays as written.
fn merge(path: &Path, text: &str, request: &Request) -> Result<(String, Vec<String>)> {
    let doc = Document::parse(path, text, OFF)?;
    let mut notes = Vec::new();
    let build_dir = format!("${{workspaceFolder}}/{}", request.build_dir);

    // clangd: which drivers to trust, and where the compilation database of the pin is.
    let is_new = doc.object.get("clangd.arguments").is_none();
    let arguments = doc.array_in(&doc.object, "clangd.arguments")?;
    if is_new {
        // One argument per line: they hold long paths.
        arguments.ensure_multiline();
    }
    if strings(&arguments).any(|(_, argument)| argument == QUERY_DRIVER.trim_end_matches('=')) {
        return Err(doc.invalid(
            "`clangd.arguments` passes `--query-driver` and its value as two elements; cxvm \
             only edits the form `--query-driver=<globs>`"
                .into(),
        ));
    }
    set_query_driver(&arguments, request);
    let existing = strings(&arguments).find_map(|(node, argument)| {
        let dir = argument.strip_prefix(COMPILE_COMMANDS_DIR)?.to_owned();
        Some((node, dir))
    });
    match existing {
        None => {
            arguments.append(format!("{COMPILE_COMMANDS_DIR}{build_dir}").into());
        }
        Some((_, dir)) if dir == build_dir => {}
        Some((node, dir)) if is_cxvm_build_dir(&dir) => {
            node.replace_with(format!("{COMPILE_COMMANDS_DIR}{build_dir}").into());
        }
        Some((_, dir)) => notes.push(format!(
            "`clangd.arguments` in {} has `{COMPILE_COMMANDS_DIR}{dir}`, which cxvm did not \
             write and leaves alone; clangd follows the pin when it reads {}",
            path.display(),
            request.build_dir
        )),
    }

    // CMake Tools: one build directory per toolchain, configured with the toolchain file.
    match json::string_in(&doc.object, "cmake.buildDirectory") {
        Some(dir) if dir != build_dir && !is_cxvm_build_dir(&dir) => notes.push(format!(
            "`cmake.buildDirectory` in {} is `{dir}`, which cxvm did not write and leaves \
             alone; cxvm's build directory for this toolchain is {}",
            path.display(),
            request.build_dir
        )),
        Some(dir) if dir == build_dir => {}
        _ => json::set_string(&doc.object, "cmake.buildDirectory", &build_dir),
    }
    let settings = doc.object_in(&doc.object, "cmake.configureSettings")?;
    let toolchain_file = format!("${{workspaceFolder}}/{PROJECT_DIR}/toolchain.cmake");
    if json::string_in(&settings, "CMAKE_TOOLCHAIN_FILE").as_deref() != Some(&toolchain_file) {
        json::set_string(&settings, "CMAKE_TOOLCHAIN_FILE", &toolchain_file);
    }
    Ok((doc.finish(), notes))
}

/// The string elements of `array`, with their nodes.
fn strings(array: &CstArray) -> impl Iterator<Item = (CstStringLit, String)> {
    array.elements().into_iter().filter_map(|node| {
        let node = node.as_string_lit()?;
        let text = node.decoded_value().ok()?;
        Some((node, text))
    })
}

/// Brings the `--query-driver=<glob>,<glob>…` argument up to date: globs of earlier pins go,
/// the one of this pin is added, other people's globs stay. clangd accepts the option once.
fn set_query_driver(arguments: &CstArray, request: &Request) {
    let existing = strings(arguments).find_map(|(node, argument)| {
        let globs = argument.strip_prefix(QUERY_DRIVER)?.to_owned();
        Some((node, globs))
    });
    let old: Vec<&str> = existing
        .iter()
        .flat_map(|(_, globs)| globs.split(','))
        .filter(|glob| !glob.is_empty())
        .collect();
    let mut new: Vec<&str> = old
        .iter()
        .copied()
        .filter(|glob| !request.owns(glob))
        .collect();
    new.extend(request.query_driver.as_deref());
    if new == old {
        return;
    }
    let argument = CstInputValue::String(format!("{QUERY_DRIVER}{}", new.join(",")));
    match existing {
        Some((node, _)) if new.is_empty() => node.remove(),
        Some((node, _)) => {
            node.replace_with(argument);
        }
        None => {
            arguments.append(argument);
        }
    }
}

/// Whether `dir` is a build directory as cxvm writes it, for any toolchain:
/// `${workspaceFolder}/build/gcc-13.3.0` or `${workspaceFolder}/build/system`.
fn is_cxvm_build_dir(dir: &str) -> bool {
    dir.strip_prefix("${workspaceFolder}/build/")
        .is_some_and(|name| {
            name == "system" || name.replacen('-', "@", 1).parse::<ToolchainId>().is_ok()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH: &str = "/p/.vscode/settings.json";

    fn gcc() -> Request {
        Request {
            query_driver: Some("/s/toolchains/gcc/13.3.0/bin/*".into()),
            known_drivers: vec![
                "/s/toolchains/gcc/13.3.0/bin/*".into(),
                "/opt/gcc-11/bin/*".into(),
            ],
            store_prefix: "/s/toolchains/".into(),
            build_dir: "build/gcc-13.3.0".into(),
        }
    }

    fn clang() -> Request {
        Request {
            query_driver: None,
            build_dir: "build/clang-17.0.6".into(),
            ..gcc()
        }
    }

    fn merged(text: &str, request: &Request) -> String {
        let (text, notes) = merge(Path::new(PATH), text, request).unwrap();
        assert_eq!(notes, Vec::<String>::new());
        text
    }

    #[test]
    fn a_new_file_for_a_gcc() {
        assert_eq!(
            merged("", &gcc()),
            r#"{
  "clangd.arguments": [
    "--query-driver=/s/toolchains/gcc/13.3.0/bin/*",
    "--compile-commands-dir=${workspaceFolder}/build/gcc-13.3.0"
  ],
  "cmake.buildDirectory": "${workspaceFolder}/build/gcc-13.3.0",
  "cmake.configureSettings": {
    "CMAKE_TOOLCHAIN_FILE": "${workspaceFolder}/.cxvm/toolchain.cmake"
  }
}
"#
        );
    }

    #[test]
    fn a_clang_gets_no_query_driver() {
        let text = merged("", &clang());
        assert!(!text.contains("--query-driver"), "{text}");
        assert!(
            text.contains("\"--compile-commands-dir=${workspaceFolder}/build/clang-17.0.6\""),
            "{text}"
        );
    }

    #[test]
    fn merging_again_changes_nothing() {
        for request in [gcc(), clang()] {
            let once = merged("", &request);
            assert_eq!(merged(&once, &request), once);
        }
    }

    #[test]
    fn switching_toolchains_replaces_what_cxvm_wrote() {
        let other_gcc = Request {
            query_driver: Some("/opt/gcc-11/bin/*".into()),
            build_dir: "build/gcc-11.4.0-local".into(),
            ..gcc()
        };
        let text = merged(&merged("", &gcc()), &other_gcc);
        assert!(
            text.contains("\"--query-driver=/opt/gcc-11/bin/*\""),
            "{text}"
        );
        assert!(!text.contains("13.3.0"), "{text}");
        assert!(
            text.contains(
                "\"cmake.buildDirectory\": \"${workspaceFolder}/build/gcc-11.4.0-local\""
            ),
            "{text}"
        );
        // To a clang: the driver glob goes, the rest follows.
        let text = merged(&text, &clang());
        assert_eq!(text, merged("", &clang()));
        // A glob of a toolchain that was removed since is still recognised by the store path.
        let stale = merged("", &gcc()).replace("13.3.0", "12.1.0");
        assert_eq!(merged(&stale, &clang()), merged("", &clang()));
    }

    #[test]
    fn other_settings_comments_and_drivers_are_kept() {
        let before = r#"{
    // editor
    "editor.tabSize": 4,
    "clangd.arguments": [
        "--background-index", // keep
        "--query-driver=/opt/arm/bin/arm-none-eabi-*,/s/toolchains/gcc/12.1.0/bin/*"
    ],
    "cmake.configureSettings": {
        "BUILD_TESTING": "ON"
    },
}
"#;
        let expected = r#"{
    // editor
    "editor.tabSize": 4,
    "clangd.arguments": [
        "--background-index", // keep
        "--query-driver=/opt/arm/bin/arm-none-eabi-*,/s/toolchains/gcc/13.3.0/bin/*",
        "--compile-commands-dir=${workspaceFolder}/build/gcc-13.3.0"
    ],
    "cmake.configureSettings": {
        "BUILD_TESTING": "ON",
        "CMAKE_TOOLCHAIN_FILE": "${workspaceFolder}/.cxvm/toolchain.cmake"
    },
    "cmake.buildDirectory": "${workspaceFolder}/build/gcc-13.3.0",
}
"#;
        let after = merged(before, &gcc());
        assert_eq!(after, expected);
        assert_eq!(merged(&after, &gcc()), after);
        // A clang keeps the cross compiler's glob.
        let text = merged(&after, &clang());
        assert!(
            text.contains("\"--query-driver=/opt/arm/bin/arm-none-eabi-*\""),
            "{text}"
        );
    }

    #[test]
    fn directories_set_by_someone_else_are_left_alone_with_a_note() {
        let before = r#"{
  "clangd.arguments": ["--compile-commands-dir=out"],
  "cmake.buildDirectory": "${workspaceFolder}/out"
}
"#;
        let (after, notes) = merge(Path::new(PATH), before, &gcc()).unwrap();
        assert!(after.contains("\"--compile-commands-dir=out\""), "{after}");
        assert!(
            after.contains("\"cmake.buildDirectory\": \"${workspaceFolder}/out\""),
            "{after}"
        );
        assert!(
            after.contains("--query-driver=/s/toolchains/gcc/13.3.0/bin/*"),
            "{after}"
        );
        assert!(after.contains("CMAKE_TOOLCHAIN_FILE"), "{after}");
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes[0].contains("--compile-commands-dir=out"), "{notes:?}");
        assert!(notes[1].contains("cmake.buildDirectory"), "{notes:?}");
        assert!(
            notes.iter().all(|note| note.contains("build/gcc-13.3.0")),
            "{notes:?}"
        );
    }

    #[test]
    fn files_cxvm_cannot_extend_are_errors_with_a_way_out() {
        for (text, needle) in [
            ("[]", "top level is not an object"),
            ("{ \"a\": }", "line 1"),
            (
                "{\"clangd.arguments\": \"--log=verbose\"}",
                "`clangd.arguments` is not an array",
            ),
            (
                "{\"clangd.arguments\": [\"--query-driver\", \"/usr/bin/*\"]}",
                "two elements",
            ),
            (
                "{\"cmake.configureSettings\": []}",
                "`cmake.configureSettings` is not an object",
            ),
        ] {
            let err = merge(Path::new(PATH), text, &gcc())
                .unwrap_err()
                .to_string();
            assert!(err.contains(PATH), "{text}: {err}");
            assert!(err.contains(needle), "{text}: {err}");
            assert!(err.contains("vscode = false"), "{text}: {err}");
        }
    }

    #[test]
    fn build_directories_of_cxvm_are_recognised() {
        for dir in ["gcc-13.3.0", "clang-17.0.6", "gcc-11.4.0-local", "system"] {
            assert!(
                is_cxvm_build_dir(&format!("${{workspaceFolder}}/build/{dir}")),
                "{dir}"
            );
        }
        for dir in [
            "${workspaceFolder}/build",
            "${workspaceFolder}/build/debug",
            "${workspaceFolder}/build/gcc-13",
            "build/gcc-13.3.0",
        ] {
            assert!(!is_cxvm_build_dir(dir), "{dir}");
        }
    }

    #[test]
    fn the_request_asks_clangd_to_query_only_a_gcc() {
        let activation = |label: &str, bin: Option<&str>| Activation {
            label: label.into(),
            bin: bin.map(Into::into),
            cc: None,
            cxx: None,
            rpath: None,
            pairing: None,
        };
        let known = [Path::new("/opt/gcc-11/bin")];
        let store = Path::new("/s/toolchains");
        let request = Request::new(
            &activation("gcc@13.3.0", Some("/s/toolchains/gcc/13.3.0/bin")),
            known,
            store,
        );
        assert_eq!(
            request,
            Request {
                query_driver: Some("/s/toolchains/gcc/13.3.0/bin/*".into()),
                known_drivers: vec!["/opt/gcc-11/bin/*".into()],
                store_prefix: "/s/toolchains/".into(),
                build_dir: "build/gcc-13.3.0".into(),
            }
        );
        let request = Request::new(
            &activation("gcc@11.4.0-local", Some("/opt/gcc-11/bin")),
            known,
            store,
        );
        assert_eq!(request.query_driver.as_deref(), Some("/opt/gcc-11/bin/*"));
        let request = Request::new(
            &activation("clang@17.0.6", Some("/s/toolchains/clang/17.0.6/bin")),
            known,
            store,
        );
        assert_eq!(request.query_driver, None);
        assert_eq!(request.build_dir, "build/clang-17.0.6");
        let request = Request::new(&activation("system", None), known, store);
        assert_eq!(request.query_driver, None);
        assert_eq!(request.build_dir, "build/system");
    }
}
