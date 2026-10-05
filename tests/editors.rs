//! `[integrations]`: `CMakeUserPresets.json` and `.vscode/settings.json` following the pin.

mod common;

use std::fs;
use std::path::Path;

use common::{FakeGcc, Sandbox, fake_toolchain};
use predicates::prelude::*;

fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path).unwrap()
}

/// Pins `compiler` by hand with the given `[integrations]` lines.
fn pin(s: &Sandbox, compiler: &str, integrations: &str) {
    fs::write(
        s.project().join(".cxvmrc"),
        format!("compiler = \"{compiler}\"\n\n[integrations]\n{integrations}"),
    )
    .unwrap();
}

#[test]
fn nothing_outside_the_project_folder_is_written_unless_asked() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    s.cxvm("").args(["use", "gcc@13"]).assert().success();
    assert!(!s.project().join("CMakeUserPresets.json").exists());
    assert!(!s.project().join(".vscode").exists());
    assert!(!s.project().join(".cxvm/toolchains").exists());
}

#[test]
fn presets_collect_one_entry_per_pinned_toolchain() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    s.install("clang", "17.0.6");
    let gcc = s.store().join("toolchains/gcc/12.4.0");
    FakeGcc::new("12.4.0").install_into(&gcc);
    pin(&s, "gcc@13.3.0", "cmake_presets = true\n");
    let presets = s.project().join("CMakeUserPresets.json");

    s.cxvm("")
        .arg("install")
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "Updated {} (preset cxvm-gcc-13.3.0)",
            presets.display()
        )));
    s.cxvm("a")
        .args(["use", "clang@17", "--gcc", "gcc@12"])
        .assert()
        .success()
        .stdout(predicate::str::contains("(preset cxvm-clang-17.0.6)"))
        .stdout(predicate::str::contains(format!(
            "  cmake -S {} --preset cxvm-clang-17.0.6\n",
            s.project().display()
        )));

    let text = read(&presets);
    assert!(text.starts_with("{\n  \"version\": 3,\n"), "{text}");
    for line in [
        r#""name": "cxvm-gcc-13.3.0","#,
        r#""binaryDir": "${sourceDir}/build/gcc-13.3.0","#,
        r#""toolchainFile": "${sourceDir}/.cxvm/toolchains/gcc-13.3.0.cmake""#,
        r#""name": "cxvm-clang-17.0.6","#,
        r#""displayName": "clang@17.0.6 with gcc@12.4.0","#,
        r#""toolchainFile": "${sourceDir}/.cxvm/toolchains/clang-17.0.6.cmake""#,
    ] {
        assert!(text.contains(line), "{line}: {text}");
    }

    // Each preset has its toolchain file; the current pin's equals `.cxvm/toolchain.cmake`.
    let dir = s.project().join(".cxvm");
    let gcc_file = read(dir.join("toolchains/gcc-13.3.0.cmake"));
    assert!(
        gcc_file.contains("set(CXVM_TOOLCHAIN \"gcc@13.3.0\")"),
        "{gcc_file}"
    );
    let clang_file = read(dir.join("toolchains/clang-17.0.6.cmake"));
    assert!(clang_file.contains("--gcc-install-dir="), "{clang_file}");
    assert_eq!(clang_file, read(dir.join("toolchain.cmake")));

    // Regenerating says nothing about a file that is up to date.
    s.cxvm("")
        .arg("install")
        .assert()
        .success()
        .stdout(predicate::str::contains("CMakeUserPresets.json").not());
    assert_eq!(read(&presets), text);
}

#[test]
fn presets_are_merged_into_an_existing_file() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    pin(&s, "gcc@13.3.0", "cmake_presets = true\n");
    let presets = s.project().join("CMakeUserPresets.json");
    fs::write(
        &presets,
        "{\n  // mine\n  \"version\": 6,\n  \"configurePresets\": [\n    \
         { \"name\": \"dev\", \"generator\": \"Ninja\" }\n  ]\n}\n",
    )
    .unwrap();
    s.cxvm("").arg("install").assert().success();
    let text = read(&presets);
    assert!(
        text.starts_with("{\n  // mine\n  \"version\": 6,\n"),
        "{text}"
    );
    assert!(
        text.contains("{ \"name\": \"dev\", \"generator\": \"Ninja\" },\n"),
        "{text}"
    );
    assert!(text.contains("\"name\": \"cxvm-gcc-13.3.0\""), "{text}");
}

#[test]
fn vscode_settings_follow_the_pin() {
    let s = Sandbox::new();
    let gcc = s.install("gcc", "13.3.0");
    s.install("clang", "17.0.6");
    let linked = s.path("opt/gcc-11");
    fake_toolchain(&linked, "gcc", "11.4.0");
    s.cxvm("").arg("link").arg(&linked).assert().success();
    pin(&s, "gcc@13.3.0", "vscode = true\n");
    let settings = s.project().join(".vscode/settings.json");

    s.cxvm("")
        .arg("install")
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "Updated {}",
            settings.display()
        )));
    assert_eq!(
        read(&settings),
        format!(
            r#"{{
  "clangd.arguments": [
    "--query-driver={}/bin/*",
    "--compile-commands-dir=${{workspaceFolder}}/build/gcc-13.3.0"
  ],
  "cmake.buildDirectory": "${{workspaceFolder}}/build/gcc-13.3.0",
  "cmake.configureSettings": {{
    "CMAKE_TOOLCHAIN_FILE": "${{workspaceFolder}}/.cxvm/toolchain.cmake"
  }}
}}
"#,
            gcc.display()
        )
    );

    // A linked GCC outside the store replaces the glob, and is itself replaced later.
    s.cxvm("").args(["use", "gcc@11-local"]).assert().success();
    let text = read(&settings);
    assert!(
        text.contains(&format!("\"--query-driver={}/bin/*\"", linked.display())),
        "{text}"
    );
    assert!(!text.contains("13.3.0"), "{text}");
    assert!(text.contains("build/gcc-11.4.0-local\""), "{text}");

    // clangd is not told to query a clang, nor the system compiler.
    for (id, build) in [
        ("clang@17", "build/clang-17.0.6"),
        ("system", "build/system"),
    ] {
        s.cxvm("").args(["use", id]).assert().success();
        let text = read(&settings);
        assert!(!text.contains("--query-driver"), "{text}");
        assert!(
            text.contains(&format!(
                "\"--compile-commands-dir=${{workspaceFolder}}/{build}\""
            )),
            "{text}"
        );
        assert!(
            text.contains(&format!(
                "\"cmake.buildDirectory\": \"${{workspaceFolder}}/{build}\""
            )),
            "{text}"
        );
    }
}

#[test]
fn vscode_settings_of_the_user_are_kept_and_reported() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    pin(&s, "gcc@13.3.0", "vscode = true\n");
    let settings = s.project().join(".vscode/settings.json");
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    fs::write(
        &settings,
        "{\n    // tabs\n    \"editor.tabSize\": 4,\n    \
         \"cmake.buildDirectory\": \"${workspaceFolder}/out\",\n}\n",
    )
    .unwrap();
    s.cxvm("")
        .arg("install")
        .assert()
        .success()
        .stderr(predicate::str::contains("note: `cmake.buildDirectory` in"))
        .stderr(predicate::str::contains("build/gcc-13.3.0"));
    let text = read(&settings);
    assert!(
        text.starts_with(
            "{\n    // tabs\n    \"editor.tabSize\": 4,\n    \
             \"cmake.buildDirectory\": \"${workspaceFolder}/out\",\n"
        ),
        "{text}"
    );
    assert!(text.contains("\"CMAKE_TOOLCHAIN_FILE\""), "{text}");
}

#[test]
fn a_file_that_cannot_be_merged_stops_use_before_anything_changes() {
    let s = Sandbox::new();
    s.install("gcc", "13.3.0");
    s.install("gcc", "14.2.0");
    for (integration, file, content) in [
        ("cmake_presets", "CMakeUserPresets.json", "{\"version\": 2}"),
        ("vscode", ".vscode/settings.json", "{ \"a\": }"),
    ] {
        let rc = format!("compiler = \"gcc@13.3.0\"\n\n[integrations]\n{integration} = true\n");
        fs::write(s.project().join(".cxvmrc"), &rc).unwrap();
        let path = s.project().join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        s.cxvm("")
            .args(["use", "gcc@14"])
            .assert()
            .code(1)
            .stderr(predicate::str::contains(path.display().to_string()))
            .stderr(predicate::str::contains(format!("{integration} = false")));
        assert_eq!(read(s.project().join(".cxvmrc")), rc);
        assert_eq!(read(&path), content);
        assert!(!s.project().join(".cxvm").exists());
        fs::remove_file(&path).unwrap();
    }
}
