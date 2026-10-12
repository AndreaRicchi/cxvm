//! `CMakeUserPresets.json`: a configure preset per pinned toolchain (opt-in, M6).

use std::path::Path;

use jsonc_parser::cst::CstInputValue;

use super::json::{self, Document};
use super::{Activation, FileEdit, PROJECT_DIR, render_toolchain_cmake, slug};
use crate::error::Result;
use crate::files;

/// The file CMake reads user-local presets from, next to the top-level `CMakeLists.txt`.
pub const FILE: &str = "CMakeUserPresets.json";

/// The presets version cxvm needs: the first with `toolchainFile` (CMake 3.21).
const VERSION: u32 = 3;

const OFF: &str = "set `cmake_presets = false` in the `[integrations]` table of .cxvmrc";

/// Name of the configure preset for the toolchain `label`: `cxvm-gcc-13.3.0`.
pub fn preset_name(label: &str) -> String {
    format!("cxvm-{}", slug(label))
}

/// The toolchain file of the preset for `label`, relative to the project root. Each preset
/// has its own, since `.cxvm/toolchain.cmake` only describes the current pin.
pub fn toolchain_file(label: &str) -> String {
    format!("{PROJECT_DIR}/toolchains/{}.cmake", slug(label))
}

/// The names of cxvm's presets (`cxvm-…`) in the `CMakeUserPresets.json` of the project at
/// `root`; none without the file.
pub fn names(root: &Path) -> Result<Vec<String>> {
    let path = root.join(FILE);
    let Some(text) = files::read_text(&path)? else {
        return Ok(Vec::new());
    };
    let doc = Document::parse(&path, &text, OFF)?;
    let presets = doc.array_in(&doc.object, "configurePresets")?;
    Ok(presets
        .elements()
        .into_iter()
        .filter_map(|element| json::string_in(&element.as_object()?, "name"))
        .filter(|name| name.starts_with("cxvm-"))
        .collect())
}

/// The toolchain of the preset `name`, as [`preset_name`] made it: `gcc@13.3.0` for
/// `cxvm-gcc-13.3.0`.
pub fn label(name: &str) -> Option<String> {
    let slug = name.strip_prefix("cxvm-")?;
    if slug == "system" {
        return Some(slug.to_owned());
    }
    let (kind, version) = slug.split_once('-')?;
    Some(format!("{kind}@{version}"))
}

/// What to write in the project at `root` so that `CMakeUserPresets.json` has a preset for
/// `activation`: the preset's toolchain file and, unless it is up to date, the presets file.
pub fn plan(root: &Path, activation: &Activation) -> Result<Vec<FileEdit>> {
    let mut edits = vec![FileEdit {
        path: root.join(toolchain_file(&activation.label)),
        text: render_toolchain_cmake(activation),
        summary: None,
    }];
    let path = root.join(FILE);
    let old = files::read_text(&path)?.unwrap_or_default();
    let new = merge(&path, &old, activation)?;
    if new != old {
        edits.push(FileEdit {
            summary: Some(format!(
                "Updated {} (preset {})",
                path.display(),
                preset_name(&activation.label)
            )),
            path,
            text: new,
        });
    }
    Ok(edits)
}

/// `text`, the content of the presets file at `path`, with the preset for `activation` added
/// or brought up to date. Presets of other toolchains and everything else stay as written.
fn merge(path: &Path, text: &str, activation: &Activation) -> Result<String> {
    let doc = Document::parse(path, text, OFF)?;
    match doc.object.get("version") {
        None => {
            doc.object.insert(0, "version", VERSION.into());
        }
        Some(version) => {
            let declared = version.value().map(|value| value.to_string());
            let enough = declared
                .as_deref()
                .and_then(|value| value.parse::<u32>().ok())
                .is_some_and(|value| value >= VERSION);
            if !enough {
                return Err(doc.invalid(format!(
                    "its `version` is {}, and the `toolchainFile` field of cxvm's presets needs \
                     version {VERSION} (CMake 3.21) or later",
                    declared.as_deref().unwrap_or("empty")
                )));
            }
        }
    }

    let name = preset_name(&activation.label);
    let preset = preset(&name, activation);
    let presets = doc.array_in(&doc.object, "configurePresets")?;
    let existing = presets.elements().into_iter().find(|element| {
        element
            .as_object()
            .and_then(|preset| json::string_in(&preset, "name"))
            .is_some_and(|existing| existing == name)
    });
    match existing {
        Some(element) => {
            // Not `replace_with`, which keeps the old element's indentation for the new lines.
            let index = element.element_index().unwrap_or(0);
            element.remove();
            presets.insert(index, preset);
        }
        None => {
            presets.append(preset);
        }
    }
    Ok(doc.finish())
}

fn preset(name: &str, activation: &Activation) -> CstInputValue {
    let label = &activation.label;
    let display_name = match &activation.pairing {
        Some(pairing) => format!("{label} with {}", pairing.gcc),
        None => label.clone(),
    };
    let fields = [
        ("name", name.to_owned()),
        ("displayName", display_name),
        (
            "description",
            "Generated by cxvm: `cxvm use` and `cxvm install` rewrite this preset".to_owned(),
        ),
        ("binaryDir", format!("${{sourceDir}}/build/{}", slug(label))),
        (
            "toolchainFile",
            format!("${{sourceDir}}/{}", toolchain_file(label)),
        ),
    ];
    CstInputValue::Object(
        fields
            .into_iter()
            .map(|(key, value)| (key.to_owned(), CstInputValue::String(value)))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrations::Pairing;

    fn gcc() -> Activation {
        Activation {
            label: "gcc@13.3.0".into(),
            bin: Some("/s/gcc/bin".into()),
            cc: Some("/s/gcc/bin/gcc".into()),
            cxx: Some("/s/gcc/bin/g++".into()),
            rpath: None,
            pairing: None,
        }
    }

    fn merged(text: &str, activation: &Activation) -> String {
        merge(Path::new("/p/CMakeUserPresets.json"), text, activation).unwrap()
    }

    const GCC_PRESET: &str = r#"{
      "name": "cxvm-gcc-13.3.0",
      "displayName": "gcc@13.3.0",
      "description": "Generated by cxvm: `cxvm use` and `cxvm install` rewrite this preset",
      "binaryDir": "${sourceDir}/build/gcc-13.3.0",
      "toolchainFile": "${sourceDir}/.cxvm/toolchains/gcc-13.3.0.cmake"
    }"#;

    #[test]
    fn preset_names_give_back_their_toolchain() {
        for label in ["gcc@13.3.0", "clang@17.0.6", "gcc@11.4.0-local", "system"] {
            assert_eq!(super::label(&preset_name(label)).as_deref(), Some(label));
        }
        assert_eq!(super::label("dev"), None);
        assert_eq!(super::label("cxvm-gcc"), None);
    }

    #[test]
    fn names_lists_the_presets_of_cxvm_only() {
        let dir = tempfile::tempdir().unwrap();
        assert!(names(dir.path()).unwrap().is_empty());
        let text = merged(
            "{\"version\": 6, \"configurePresets\": [{\"name\": \"dev\"}, \"odd\"]}",
            &gcc(),
        );
        std::fs::write(dir.path().join(FILE), text).unwrap();
        assert_eq!(names(dir.path()).unwrap(), ["cxvm-gcc-13.3.0"]);
    }

    #[test]
    fn a_new_file_has_the_version_and_the_preset() {
        let expected = format!(
            "{{\n  \"version\": 3,\n  \"configurePresets\": [\n    {GCC_PRESET}\n  ]\n}}\n"
        );
        assert_eq!(merged("", &gcc()), expected);
        assert_eq!(merged("  \n", &gcc()).trim(), expected.trim());
    }

    #[test]
    fn merging_again_changes_nothing() {
        let once = merged("", &gcc());
        assert_eq!(merged(&once, &gcc()), once);
    }

    #[test]
    fn each_toolchain_gets_a_preset_and_earlier_ones_stay() {
        let clang = Activation {
            label: "clang@17.0.6".into(),
            pairing: Some(Pairing {
                gcc: "gcc@13.3.0".into(),
                flag: "--gcc-install-dir=/x".into(),
            }),
            ..gcc()
        };
        let system = Activation {
            label: "system".into(),
            ..gcc()
        };
        let text = merged(&merged(&merged("", &gcc()), &clang), &system);
        let names: Vec<&str> = text
            .lines()
            .filter_map(|line| line.trim().strip_prefix("\"name\": "))
            .collect();
        assert_eq!(
            names,
            [
                "\"cxvm-gcc-13.3.0\",",
                "\"cxvm-clang-17.0.6\",",
                "\"cxvm-system\","
            ]
        );
        assert!(
            text.contains(r#""displayName": "clang@17.0.6 with gcc@13.3.0""#),
            "{text}"
        );
        assert!(
            text.contains(r#""binaryDir": "${sourceDir}/build/system""#),
            "{text}"
        );
        assert!(
            text.contains(r#""toolchainFile": "${sourceDir}/.cxvm/toolchains/clang-17.0.6.cmake""#),
            "{text}"
        );
    }

    #[test]
    fn everything_else_in_the_file_is_kept_as_written() {
        let before = r#"{
    // my presets
    "version": 6,
    "configurePresets": [
        {
            "name": "dev", // mine
            "generator": "Ninja",
            "binaryDir": "${sourceDir}/out"
        },
        {"name": "cxvm-gcc-13.3.0", "binaryDir": "stale"}
    ],
    "buildPresets": [{"name": "dev", "configurePreset": "dev"}]
}
"#;
        let after = merged(before, &gcc());
        let expected = r#"{
    // my presets
    "version": 6,
    "configurePresets": [
        {
            "name": "dev", // mine
            "generator": "Ninja",
            "binaryDir": "${sourceDir}/out"
        },
        {
            "name": "cxvm-gcc-13.3.0",
            "displayName": "gcc@13.3.0",
            "description": "Generated by cxvm: `cxvm use` and `cxvm install` rewrite this preset",
            "binaryDir": "${sourceDir}/build/gcc-13.3.0",
            "toolchainFile": "${sourceDir}/.cxvm/toolchains/gcc-13.3.0.cmake"
        }
    ],
    "buildPresets": [{"name": "dev", "configurePreset": "dev"}]
}
"#;
        assert_eq!(after, expected);
        assert_eq!(merged(&after, &gcc()), after);
    }

    #[test]
    fn a_file_without_presets_gets_the_array() {
        let after = merged("{\n  \"version\": 4\n}\n", &gcc());
        assert!(
            after.starts_with("{\n  \"version\": 4,\n  \"configurePresets\": [\n"),
            "{after}"
        );
    }

    #[test]
    fn files_cxvm_cannot_extend_are_errors_with_a_way_out() {
        for (text, needle) in [
            ("[]", "top level is not an object"),
            ("{ \"version\": }", "line 1"),
            ("{\"version\": 2}", "`version` is 2"),
            ("{\"version\": \"3\"}", "`version` is \"3\""),
            (
                "{\"version\": 3, \"configurePresets\": {}}",
                "`configurePresets` is not an array",
            ),
        ] {
            let err = merge(Path::new("/p/CMakeUserPresets.json"), text, &gcc())
                .unwrap_err()
                .to_string();
            assert!(err.contains("/p/CMakeUserPresets.json"), "{text}: {err}");
            assert!(err.contains(needle), "{text}: {err}");
            assert!(err.contains("cmake_presets = false"), "{text}: {err}");
        }
    }
}
