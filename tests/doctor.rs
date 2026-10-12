//! `cxvm doctor`: what it finds in the store, about the active toolchain and in the cache, and
//! its exit status (1 only for errors).

mod common;

use std::fs;

use common::{Sandbox, fake_toolchain};
use predicates::prelude::*;

/// What a dynamically linked binary that finds all its libraries shows in `ldd`.
const LDD_OK: &str = "\tlibc.so.6 => /lib/x86_64-linux-gnu/libc.so.6 (0x00007f)";

/// A sandbox with `ldd`, so that only what a test sets up is reported.
fn sandbox() -> Sandbox {
    let s = Sandbox::new();
    s.fake_ldd(LDD_OK);
    s
}

#[test]
fn a_healthy_store_has_no_problems() {
    let s = sandbox();
    let prefix = s.install_with_meta("gcc", "13.3.0");
    s.install_with_meta("clang", "19.1.0");
    s.global("gcc@13");
    s.cxvm("")
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "  ok: gcc@13.3.0 in {}\n",
            prefix.display()
        )))
        .stdout(predicate::str::contains("  ok: clang@19.1.0 in "))
        .stdout(predicate::str::contains(
            "  ok: global default: gcc@13, that is gcc@13.3.0\n",
        ))
        .stdout(predicate::str::contains(
            "Active toolchain: gcc@13 (global default in ",
        ))
        .stdout(predicate::str::contains("Cache: "))
        .stdout(predicate::str::contains(
            "  ok: no downloads or build logs\n",
        ))
        .stdout(predicate::str::ends_with("\nNo problems found\n"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn an_empty_store_is_fine_and_doctor_creates_nothing() {
    let s = sandbox();
    s.cxvm("")
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "  ok: nothing installed or linked yet\n",
        ))
        .stdout(predicate::str::contains(
            "Active toolchain: system (no .cxvmrc or global default)\n  ok: the system `cc` is ",
        ));
    assert!(!s.store().exists());
    assert!(!s.path("cache").exists());
}

#[test]
fn missing_shared_libraries_are_errors_and_fail_the_doctor() {
    let s = Sandbox::new();
    s.fake_ldd("\tlibtinfo.so.5 => not found\n\tlibz.so.1 => /lib/libz.so.1 (0x00007f)");
    s.install_with_meta("clang", "13.0.1");
    s.cxvm("")
        .arg("doctor")
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "  error: clang@13.0.1 cannot run here: missing shared libraries libtinfo.so.5\n    \
             hint: install the packages that provide libtinfo.so.5, then run `cxvm doctor` again",
        ))
        .stderr(predicate::str::starts_with(
            "error: cxvm doctor found 1 error\n  hint: ",
        ));
}

#[test]
fn without_ldd_the_libraries_are_not_checked() {
    let s = Sandbox::new();
    s.install_with_meta("gcc", "13.3.0");
    s.cxvm("")
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "  warning: no `ldd` on PATH, so the shared libraries of the toolchains were not \
             checked\n",
        ))
        .stdout(predicate::str::ends_with(
            "\n1 warning; nothing is broken\n",
        ));
}

#[test]
fn strays_and_missing_metadata_are_warnings() {
    let s = sandbox();
    let toolchains = s.store().join("toolchains");
    s.install("clang", "19.1.0");
    let wrong = s.install_with_meta("gcc", "14.2.0");
    fs::write(
        wrong.join(".cxvm-meta.toml"),
        fs::read_to_string(wrong.join(".cxvm-meta.toml"))
            .unwrap()
            .replace("gcc@14.2.0", "gcc@14.1.0"),
    )
    .unwrap();
    fs::create_dir_all(toolchains.join("gcc/13")).unwrap();
    fs::create_dir_all(toolchains.join("msvc/19.0.0")).unwrap();
    s.cxvm("")
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "  warning: {} is not a toolchain: its name is not an exact version",
            toolchains.join("gcc/13").display()
        )))
        .stdout(predicate::str::contains(format!(
            "  warning: {} is not a toolchain: it is not a directory named after a compiler",
            toolchains.join("msvc").display()
        )))
        .stdout(predicate::str::contains(
            "  warning: clang@19.1.0 has no .cxvm-meta.toml: cxvm did not install it",
        ))
        .stdout(predicate::str::contains(
            "    hint: reinstall it with `cxvm remove clang@19.1.0` and `cxvm install \
             clang@19.1.0`\n",
        ))
        .stdout(predicate::str::contains(
            "describes gcc@14.1.0, not gcc@14.2.0",
        ))
        .stdout(predicate::str::ends_with(
            "\n4 warnings; nothing is broken\n",
        ));
}

#[test]
fn an_incomplete_toolchain_is_an_error() {
    let s = sandbox();
    let prefix = s.install_with_meta("gcc", "13.3.0");
    fs::remove_file(prefix.join("bin/g++")).unwrap();
    s.cxvm("")
        .arg("doctor")
        .assert()
        .code(1)
        .stdout(predicate::str::contains(format!(
            "  error: gcc@13.3.0 is incomplete: {} is missing or not executable\n    \
             hint: delete {} and install it again\n",
            prefix.join("bin/g++").display(),
            prefix.display()
        )));
}

#[test]
fn links_whose_prefix_is_gone_or_changed_are_reported() {
    let s = sandbox();
    let gone = s.path("opt/gcc-11");
    let changed = s.path("opt/gcc-12");
    fake_toolchain(&gone, "gcc", "11.4.0");
    fake_toolchain(&changed, "gcc", "12.3.0");
    for prefix in [&gone, &changed] {
        s.cxvm("")
            .args(["link", prefix.to_str().unwrap()])
            .assert()
            .success();
    }
    fs::remove_dir_all(&gone).unwrap();
    fake_toolchain(&changed, "gcc", "12.4.0");
    s.global("gcc@11.4.0-local");
    s.cxvm("")
        .arg("doctor")
        .assert()
        .code(1)
        .stdout(predicate::str::contains(format!(
            "  error: gcc@11.4.0-local is linked to {}, which is gone\n    \
             hint: restore the install there, or forget it with `cxvm unlink gcc@11.4.0-local`\n",
            gone.display()
        )))
        .stdout(predicate::str::contains(format!(
            "  warning: gcc@12.3.0-local now reports version 12.4.0: the install in {} changed\n    \
             hint: register the install again with `cxvm unlink gcc@12.3.0-local` and `cxvm link {}`\n",
            changed.display(),
            changed.display()
        )))
        .stdout(predicate::str::contains(
            "  note: gcc@11.4.0-local is selected, but its compilers are missing",
        ))
        .stderr(predicate::str::contains("found 1 error and 1 warning"));
}

#[test]
fn a_link_hidden_by_a_store_entry_is_a_warning() {
    let s = sandbox();
    let prefix = s.path("opt/gcc-11");
    fake_toolchain(&prefix, "gcc", "11.4.0");
    s.cxvm("")
        .args(["link", prefix.to_str().unwrap()])
        .assert()
        .success();
    let entry = s.install_with_meta("gcc", "11.4.0-local");
    fake_toolchain(&entry, "gcc", "11.4.0");
    s.cxvm("")
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "  warning: the link gcc@11.4.0-local -> {} is hidden by ",
            prefix.display()
        )));
}

#[test]
fn a_pin_or_global_default_that_is_not_installed_is_an_error() {
    let s = sandbox();
    s.install_with_meta("gcc", "13.3.0");
    let config = s.global("gcc@12");
    let rc = s.pin("", "gcc@14.2.0");
    s.cxvm("a")
        .arg("doctor")
        .assert()
        .code(1)
        .stdout(predicate::str::contains(format!(
            "  error: gcc@12 is not installed (global default in {})\n    hint: run `cxvm install gcc@12`",
            config.display()
        )))
        .stdout(predicate::str::contains(format!(
            "Active toolchain: gcc@14.2.0 (set by {})\n  \
             error: gcc@14.2.0 is not installed (set by {})\n    \
             hint: run `cxvm install gcc@14.2.0`",
            rc.display(),
            rc.display()
        )))
        .stderr(predicate::str::contains("found 2 errors"));
}

#[test]
fn an_invalid_file_is_reported_and_the_checks_go_on() {
    let s = sandbox();
    let config = s.global("gcc@13");
    fs::write(&config, "compiler = 13\n").unwrap();
    s.cxvm("")
        .arg("doctor")
        .assert()
        .code(1)
        .stdout(predicate::str::contains(format!(
            "  error: {} is invalid",
            config.display()
        )))
        .stdout(predicate::str::contains("Cache: "));
}

#[test]
fn staging_leftovers_are_warnings_unless_a_cxvm_runs() {
    let s = sandbox();
    let leftover = s.store().join("staging/clang-19.1.0.4242");
    fs::create_dir_all(&leftover).unwrap();
    s.cxvm("")
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "  warning: {} was left by an interrupted install or removal\n    \
             hint: the next `cxvm install`, `remove`, `link`, `unlink` or `prune` deletes it\n",
            leftover.display()
        )));

    let lock = fs::File::create(s.store().join("lock")).unwrap();
    lock.lock().unwrap();
    s.cxvm("")
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "  note: {} belongs to a cxvm process that is running now\n",
            leftover.display()
        )))
        .stdout(predicate::str::ends_with("\nNo problems found\n"));
    assert!(leftover.exists());
}

#[test]
fn the_cache_size_and_cxvm_compiler_are_notes() {
    let s = sandbox();
    s.install_with_meta("gcc", "13.3.0");
    let downloads = s.path("cache/cxvm/downloads");
    fs::create_dir_all(&downloads).unwrap();
    fs::write(downloads.join("gcc-13.3.0.tar.xz"), vec![0; 2 << 20]).unwrap();
    s.cxvm("")
        .arg("doctor")
        .env("CXVM_COMPILER", "gcc@13.3.0")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Active toolchain: gcc@13.3.0 (set by CXVM_COMPILER)\n  \
             note: CXVM_COMPILER=gcc@13.3.0 overrides .cxvmrc and the global default in this \
             shell\n    hint: unset CXVM_COMPILER to use them\n",
        ))
        .stdout(predicate::str::contains(
            "  note: downloads and build logs take 2 MiB (1 file)\n    \
             hint: nothing installed needs them; `cxvm prune` deletes them\n",
        ))
        .stdout(predicate::str::ends_with("\nNo problems found\n"));
}

#[test]
fn no_system_compiler_is_an_error_when_system_is_active() {
    let s = sandbox();
    fs::remove_file(s.bin().join("cc")).unwrap();
    s.cxvm("")
        .arg("doctor")
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "  error: there is no `cc` on PATH for `system`\n",
        ));
}

/// A sandbox where `proj` pins `id` through `cxvm use`, with `.cxvm/` generated.
fn project_using(id: &str) -> Sandbox {
    let s = sandbox();
    let (kind, version) = id.split_once('@').unwrap();
    s.install_with_meta(kind, version);
    s.cxvm("").args(["use", id]).assert().success();
    s
}

#[test]
fn a_project_whose_files_match_its_pin_is_fine() {
    let s = project_using("gcc@13.3.0");
    s.cxvm("a/b")
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "Project: {}\n  ok: .cxvm/ matches gcc@13.3.0\n",
            s.project().display()
        )))
        .stdout(predicate::str::ends_with("\nNo problems found\n"));
}

#[test]
fn a_fresh_clone_without_cxvm_dir_is_a_warning() {
    let s = sandbox();
    s.install_with_meta("gcc", "13.3.0");
    s.pin("", "gcc@13.3.0");
    s.cxvm("")
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "  warning: {} is missing: nothing was generated here yet\n    \
             hint: `cxvm install` in {} regenerates it\n",
            s.project().join(".cxvm").display(),
            s.project().display()
        )));
}

#[test]
fn files_left_from_another_pin_are_out_of_date() {
    let s = project_using("gcc@13.3.0");
    let other = s.install_with_meta("gcc", "14.2.0");
    // As after a `git pull` that changed the pin.
    s.pin("", "gcc@14.2.0");
    let dir = s.project().join(".cxvm");
    s.cxvm("")
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "  warning: {} points at {}, not at gcc@14.2.0 in {}\n",
            dir.join("current").display(),
            s.store().join("toolchains/gcc/13.3.0").display(),
            other.display()
        )))
        .stdout(predicate::str::contains(format!(
            "  warning: {} is out of date: it was generated for gcc@13.3.0, and the project \
             pins gcc@14.2.0\n",
            dir.join("toolchain.cmake").display()
        )))
        .stdout(predicate::str::contains(format!(
            "  warning: {} is out of date",
            dir.join("env").display()
        )))
        .stdout(predicate::str::ends_with(
            "\n3 warnings; nothing is broken\n",
        ));
    s.cxvm("").arg("install").assert().success();
    s.cxvm("")
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "  ok: .cxvm/ matches gcc@14.2.0\n",
        ));
}

#[test]
fn a_current_link_to_a_removed_toolchain_is_an_error() {
    let s = project_using("gcc@13.3.0");
    s.install_with_meta("gcc", "14.2.0");
    s.pin("", "gcc@14.2.0");
    let old = s.store().join("toolchains/gcc/13.3.0");
    fs::remove_dir_all(&old).unwrap();
    s.cxvm("")
        .arg("doctor")
        .assert()
        .code(1)
        .stdout(predicate::str::contains(format!(
            "  error: {} points at {}, which is gone\n    hint: `cxvm install` in {} points it \
             at gcc@14.2.0\n",
            s.project().join(".cxvm/current").display(),
            old.display(),
            s.project().display()
        )));
}

#[test]
fn a_current_that_is_not_a_symlink_is_an_error() {
    let s = project_using("gcc@13.3.0");
    let current = s.project().join(".cxvm/current");
    fs::remove_file(&current).unwrap();
    fs::create_dir(&current).unwrap();
    s.cxvm("")
        .arg("doctor")
        .assert()
        .code(1)
        .stdout(predicate::str::contains(format!(
            "  error: {} is not a symlink\n",
            current.display()
        )));
}

#[test]
fn a_pairing_with_a_gcc_that_is_not_installed_is_an_error() {
    let s = project_using("clang@19.1.0");
    let rc = s.project().join(".cxvmrc");
    fs::write(
        &rc,
        "compiler = \"clang@19.1.0\"\n\n[clang]\ngcc = \"gcc@12.4.0\"\n",
    )
    .unwrap();
    s.cxvm("")
        .arg("doctor")
        .assert()
        .code(1)
        .stdout(predicate::str::contains(format!(
            "  error: gcc@12.4.0 is not installed (`[clang] gcc` in {})\n",
            rc.display()
        )));
}

#[test]
fn a_pin_replaced_by_cxvm_compiler_is_still_checked() {
    let s = sandbox();
    s.install_with_meta("gcc", "13.3.0");
    let rc = s.pin("", "gcc@14.2.0");
    s.cxvm("")
        .arg("doctor")
        .env("CXVM_COMPILER", "gcc@13.3.0")
        .assert()
        .code(1)
        .stdout(predicate::str::contains(format!(
            "  error: gcc@14.2.0 is not installed (set by {})\n",
            rc.display()
        )));
}

#[test]
fn an_invalid_cxvmrc_is_reported_once() {
    let s = sandbox();
    let rc = s.pin("", "gcc13");
    s.cxvm("")
        .arg("doctor")
        .assert()
        .code(1)
        .stdout(predicate::str::contains(format!(
            "  note: cannot tell which toolchain is active until {} is fixed\n",
            rc.display()
        )))
        .stdout(predicate::str::contains(format!(
            "  error: {} is invalid",
            rc.display()
        )))
        .stderr(predicate::str::contains("found 1 error\n"));
}

#[test]
fn presets_of_toolchains_that_are_gone_are_warnings() {
    let s = sandbox();
    s.install_with_meta("gcc", "13.3.0");
    s.install_with_meta("clang", "19.1.0");
    fs::write(
        s.project().join(".cxvmrc"),
        "compiler = \"gcc@13.3.0\"\n\n[integrations]\ncmake_presets = true\n",
    )
    .unwrap();
    s.cxvm("").args(["use", "gcc@13.3.0"]).assert().success();
    s.cxvm("").args(["use", "clang@19.1.0"]).assert().success();
    s.cxvm("")
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "  ok: the presets of cxvm in CMakeUserPresets.json are up to date\n",
        ));

    fs::remove_dir_all(s.store().join("toolchains/gcc/13.3.0")).unwrap();
    s.cxvm("")
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "  warning: the preset cxvm-gcc-13.3.0 in {} is for gcc@13.3.0, which is not \
             installed\n    hint: delete the preset (cxvm leaves it, since presets of yours may \
             inherit from it), or install it again with `cxvm install gcc@13.3.0`\n",
            s.project().join("CMakeUserPresets.json").display()
        )));

    fs::remove_file(s.project().join(".cxvm/toolchains/clang-19.1.0.cmake")).unwrap();
    s.cxvm("")
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "  warning: {} is missing\n",
            s.project()
                .join(".cxvm/toolchains/clang-19.1.0.cmake")
                .display()
        )));
}

/// Writes what CMake 4.2.3 leaves in `dir` after configuring it with `toolchain_file`: the
/// C++ compiler is recorded in `CMakeFiles/`, not in the cache.
fn configured(
    dir: &std::path::Path,
    toolchain_file: Option<&std::path::Path>,
    cxx: &std::path::Path,
) {
    fs::create_dir_all(dir.join("CMakeFiles/4.2.3")).unwrap();
    let mut cache = String::from(
        "CMAKE_CACHE_MAJOR_VERSION:INTERNAL=4\n\
         CMAKE_CACHE_MINOR_VERSION:INTERNAL=2\n\
         CMAKE_CACHE_PATCH_VERSION:INTERNAL=3\n",
    );
    if let Some(file) = toolchain_file {
        cache.push_str(&format!(
            "CMAKE_TOOLCHAIN_FILE:FILEPATH={}\n",
            file.display()
        ));
    }
    fs::write(dir.join("CMakeCache.txt"), cache).unwrap();
    fs::write(
        dir.join("CMakeFiles/4.2.3/CMakeCXXCompiler.cmake"),
        format!(
            "set(CMAKE_CXX_COMPILER \"{}\")\nset(CMAKE_CXX_COMPILER_ARG1 \"\")\n",
            cxx.display()
        ),
    )
    .unwrap();
}

#[test]
fn a_build_dir_left_on_the_compiler_of_an_earlier_pin_is_an_error() {
    let s = project_using("gcc@13.3.0");
    let toolchain_file = s.project().join(".cxvm/toolchain.cmake");
    let gxx = s.store().join("toolchains/gcc/13.3.0/bin/g++");
    let build = s.project().join("build");
    configured(&build.join("x"), Some(&toolchain_file), &gxx);
    configured(&build.join("gcc-13.3.0"), Some(&toolchain_file), &gxx);
    s.cxvm("")
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "  ok: {} builds with gcc@13.3.0\n",
            build.join("x").display()
        )));

    let clang = s.install_with_meta("clang", "19.1.0");
    s.cxvm("").args(["use", "clang@19.1.0"]).assert().success();
    configured(
        &build.join("clang-19.1.0"),
        Some(&toolchain_file),
        &clang.join("bin/clang++"),
    );
    s.cxvm("")
        .arg("doctor")
        .assert()
        .code(1)
        .stdout(predicate::str::contains(format!(
            "  error: {} builds C++ with {}, but its toolchain file {} now names {} \
             (clang@19.1.0)\n    hint: delete {} and configure it again: CMake keeps the \
             compiler a build dir was first configured with; or keep a build dir per toolchain: \
             `cmake -S {root} -B {root}/build/clang-19.1.0 -DCMAKE_TOOLCHAIN_FILE={}`\n",
            build.join("x").display(),
            gxx.display(),
            toolchain_file.display(),
            clang.join("bin/clang++").display(),
            build.join("x").display(),
            toolchain_file.display(),
            root = s.project().display(),
        )))
        .stdout(predicate::str::contains(format!(
            "  ok: {} builds with gcc@13.3.0\n",
            build.join("gcc-13.3.0").display()
        )))
        .stdout(predicate::str::contains(format!(
            "  ok: {} builds with clang@19.1.0\n",
            build.join("clang-19.1.0").display()
        )))
        .stderr(predicate::str::contains("found 1 error\n"));
}

#[test]
fn a_build_dir_configured_without_toolchain_file_is_compared_with_the_pin() {
    let s = project_using("gcc@13.3.0");
    let build = s.project().join("build");
    configured(&build, None, &s.bin().join("c++"));
    s.cxvm("")
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "  warning: {} builds C++ with {}, and the project pins gcc@13.3.0 ({})\n    \
             hint: if it should follow the pin, delete it and configure it again with \
             `-DCMAKE_TOOLCHAIN_FILE={}`\n",
            build.display(),
            s.bin().join("c++").display(),
            s.store().join("toolchains/gcc/13.3.0/bin/g++").display(),
            s.project().join(".cxvm/toolchain.cmake").display()
        )));
}

#[test]
fn build_dirs_with_toolchain_files_of_their_own_are_left_alone() {
    let s = project_using("gcc@13.3.0");
    let cross = s.project().join("cmake/arm-none-eabi.cmake");
    fs::create_dir_all(cross.parent().unwrap()).unwrap();
    fs::write(&cross, "set(CMAKE_SYSTEM_NAME Generic)\n").unwrap();
    let build = s.project().join("build-arm");
    configured(&build, Some(&cross), &s.path("arm/bin/arm-none-eabi-g++"));
    s.cxvm("")
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("build-arm").not())
        .stdout(predicate::str::ends_with("\nNo problems found\n"));
    s.cxvm("")
        .args(["-v", "doctor"])
        .assert()
        .success()
        .stderr(predicate::str::contains(format!(
            "debug: not checking {}: {} is not a toolchain file of cxvm\n",
            build.display(),
            cross.display()
        )));
}

#[test]
fn a_build_dir_whose_compiler_is_gone_is_an_error() {
    let s = project_using("gcc@13.3.0");
    let removed = s.store().join("toolchains/gcc/12.4.0/bin/g++");
    let build = s.project().join("build/gcc-12.4.0");
    configured(
        &build,
        Some(&s.project().join(".cxvm/toolchain.cmake")),
        &removed,
    );
    s.cxvm("")
        .arg("doctor")
        .assert()
        .code(1)
        .stdout(predicate::str::contains(format!(
            "  error: {} builds C++ with {}, which is gone\n",
            build.display(),
            removed.display()
        )));
}
