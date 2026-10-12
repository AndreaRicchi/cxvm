//! `cxvm doctor`: checks the store, the active toolchain, the current project and the cache,
//! and says what to do about each problem it finds.
//!
//! It only reads: it takes no lock and writes nothing. A file it cannot parse is one more
//! finding, never the end of the report.

pub mod cmake;

use cmake::{Compiler, Lang};

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anstyle::{AnsiColor, Style};

use crate::backends::smoke;
use crate::commands::{self, Context, human_size};
use crate::config::Config;
use crate::detect;
use crate::error::{Error, Result};
use crate::files;
use crate::integrations::{self, Activation, PROJECT_DIR, presets};
use crate::output::{self, Label, debug};
use crate::project::{self, ProjectRc, Resolution};
use crate::store::{Installed, META_FILE, Meta, StoreEntry};
use crate::toolchain::{self, Kind, Spec};

/// How bad a finding is. Only errors make `cxvm doctor` fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Something fails, or silently does the wrong thing.
    Error,
    /// Something to clean up; nothing fails because of it.
    Warning,
    /// Worth knowing.
    Note,
}

impl Severity {
    fn label(self) -> Label {
        match self {
            Severity::Error => Label::Error,
            Severity::Warning => Label::Warning,
            Severity::Note => Label::Note,
        }
    }
}

/// A line of the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// A check that passed, and what it found.
    Ok(String),
    Finding {
        severity: Severity,
        message: String,
        /// What to do about it; every error and warning has one.
        hint: Option<String>,
    },
}

/// A part of the report: what was checked, under a title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub title: String,
    pub lines: Vec<Line>,
}

impl Section {
    fn new(title: String) -> Self {
        Self {
            title,
            lines: Vec::new(),
        }
    }

    fn ok(&mut self, text: String) {
        self.lines.push(Line::Ok(text));
    }

    fn find(&mut self, severity: Severity, message: String, hint: Option<String>) {
        self.lines.push(Line::Finding {
            severity,
            message,
            hint,
        });
    }

    fn error(&mut self, message: String, hint: impl Into<String>) {
        self.find(Severity::Error, message, Some(hint.into()));
    }

    fn warning(&mut self, message: String, hint: impl Into<String>) {
        self.find(Severity::Warning, message, Some(hint.into()));
    }

    fn note(&mut self, message: String, hint: Option<String>) {
        self.find(Severity::Note, message, hint);
    }

    /// `err` as a finding: its message, and its `hint:` as the hint.
    fn report(&mut self, severity: Severity, err: &Error) {
        let text = err.to_string();
        match text.rsplit_once("\n  hint: ") {
            Some((message, hint)) => self.find(severity, message.into(), Some(hint.into())),
            None => self.find(severity, text, None),
        }
    }

    /// The number of findings so far, to tell whether a check added any.
    fn findings(&self) -> usize {
        self.lines
            .iter()
            .filter(|line| matches!(line, Line::Finding { .. }))
            .count()
    }
}

/// `cxvm doctor`: prints the report; fails if it holds an error.
pub fn run(ctx: &Context) -> Result<()> {
    let sections = check(ctx);
    let _ = write!(anstream::stdout(), "{}", render(&sections));
    let count = |severity: Severity| {
        (sections.iter().flat_map(|section| &section.lines))
            .filter(|line| matches!(line, Line::Finding { severity: s, .. } if *s == severity))
            .count()
    };
    let (errors, warnings) = (count(Severity::Error), count(Severity::Warning));
    let warnings_text = plural(warnings, "warning");
    match (errors, warnings) {
        (0, 0) => println!("\nNo problems found"),
        (0, _) => println!("\n{warnings_text}; nothing is broken"),
        (_, 0) => {
            return Err(Error::Diagnosed {
                summary: plural(errors, "error"),
            });
        }
        _ => {
            return Err(Error::Diagnosed {
                summary: format!("{} and {warnings_text}", plural(errors, "error")),
            });
        }
    }
    Ok(())
}

/// `1 error`, `2 warnings`.
fn plural(count: usize, word: &str) -> String {
    match count {
        1 => format!("1 {word}"),
        _ => format!("{count} {word}s"),
    }
}

/// Every check, by section.
pub fn check(ctx: &Context) -> Vec<Section> {
    let (store, toolchains) = check_store(ctx);
    let mut sections = vec![store, check_active(ctx, &toolchains)];
    sections.extend(check_project(ctx, &toolchains));
    sections.push(check_cache(ctx));
    sections
}

/// The style of `ok:`.
const OK: Style = AnsiColor::Green.on_default();

/// The report as text, with the styles for [`anstream`] to keep or strip.
fn render(sections: &[Section]) -> String {
    let mut out = String::new();
    for (index, section) in sections.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(&output::paint(Style::new().bold(), &section.title));
        out.push('\n');
        for line in &section.lines {
            let text = match line {
                Line::Ok(text) => format!("{} {text}", output::paint(OK, "ok:")),
                Line::Finding {
                    severity,
                    message,
                    hint,
                } => {
                    let text = match hint {
                        Some(hint) => format!("{message}\n  hint: {hint}"),
                        None => message.clone(),
                    };
                    output::render(severity.label(), &text)
                }
            };
            for line in text.lines() {
                out.push_str("  ");
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

/// The store section, and the toolchains cxvm can select: those in the store and the links
/// they do not hide, usable or not.
fn check_store(ctx: &Context) -> (Section, Vec<Installed>) {
    let store = ctx.store();
    let mut section = Section::new(format!("Store: {}", ctx.paths.data_dir().display()));
    let entries = store.scan().unwrap_or_else(|err| {
        section.report(Severity::Error, &err);
        Vec::new()
    });
    let links = store.links().unwrap_or_else(|err| {
        section.report(Severity::Error, &err);
        Vec::new()
    });

    let ldd = has_ldd(ctx);
    let any = links.len()
        + entries
            .iter()
            .filter(|entry| matches!(entry, StoreEntry::Toolchain(_)))
            .count();
    if !ldd && any > 0 {
        section.warning(
            "no `ldd` on PATH, so the shared libraries of the toolchains were not checked".into(),
            "`ldd` comes with the C library (`libc-bin` on Debian/Ubuntu, `glibc-common` on \
             Fedora/RHEL); put it on PATH and run `cxvm doctor` again",
        );
    }

    let mut toolchains: Vec<Installed> = Vec::new();
    for entry in entries {
        match entry {
            StoreEntry::Stray { path, reason } => section.warning(
                format!("{} is not a toolchain: {reason}", path.display()),
                "cxvm ignores it: delete it, or move a toolchain you want to keep out of the \
                 store and register it with `cxvm link <prefix>`",
            ),
            StoreEntry::Toolchain(toolchain) => {
                let before = section.findings();
                check_meta(&mut section, &toolchain);
                check_toolchain(&mut section, &toolchain, ldd);
                if section.findings() == before {
                    section.ok(format!("{} in {}", toolchain.id, toolchain.path.display()));
                }
                toolchains.push(toolchain);
            }
        }
    }
    for link in links {
        if let Some(entry) = toolchains.iter().find(|t| t.id == link.id) {
            section.warning(
                format!(
                    "the link {} -> {} is hidden by {}, which has the same id",
                    link.id,
                    link.prefix.display(),
                    entry.path.display()
                ),
                format!(
                    "`cxvm unlink {}`, or link it again under another id with `cxvm link {} \
                     --as {}@{}-<suffix>`",
                    link.id,
                    link.prefix.display(),
                    link.id.kind(),
                    commands::numbers(&link.id)
                ),
            );
            continue;
        }
        let toolchain = Installed {
            id: link.id,
            path: link.prefix,
            cc: link.cc,
            cxx: link.cxx,
            linked: true,
        };
        let before = section.findings();
        if toolchain.path.is_dir() {
            check_toolchain(&mut section, &toolchain, ldd);
        } else {
            section.error(
                format!(
                    "{} is linked to {}, which is gone",
                    toolchain.id,
                    toolchain.path.display()
                ),
                format!(
                    "restore the install there, or forget it with `cxvm unlink {}`",
                    toolchain.id
                ),
            );
        }
        if section.findings() == before {
            section.ok(format!(
                "{} -> {} (linked)",
                toolchain.id,
                toolchain.path.display()
            ));
        }
        toolchains.push(toolchain);
    }
    toolchains.sort_by(|a, b| a.id.cmp(&b.id));
    if toolchains.is_empty() && section.lines.is_empty() {
        section.ok("nothing installed or linked yet".into());
    }

    check_global(ctx, &mut section, &toolchains);
    check_staging(ctx, &mut section);
    (section, toolchains)
}

/// Whether an executable `ldd` is on `PATH`.
fn has_ldd(ctx: &Context) -> bool {
    (ctx.path_var.iter().flat_map(std::env::split_paths))
        .any(|dir| files::is_executable(&dir.join("ldd")))
}

/// How to get a store toolchain back in order: reinstall it, or for one cxvm cannot install,
/// link it from elsewhere.
fn reinstall_hint(toolchain: &Installed) -> String {
    let id = &toolchain.id;
    match id.version().suffix() {
        Some(_) => {
            format!("move it out of the store and register it with `cxvm link <prefix> --as {id}`")
        }
        None => format!("reinstall it with `cxvm remove {id}` and `cxvm install {id}`"),
    }
}

/// The `.cxvm-meta.toml` of a toolchain in the store.
fn check_meta(section: &mut Section, toolchain: &Installed) {
    let id = &toolchain.id;
    match Meta::load(&toolchain.path) {
        Ok(Some(meta)) if meta.id == id.to_string() => {}
        Ok(Some(meta)) => section.warning(
            format!(
                "the {META_FILE} in {} describes {}, not {id}",
                toolchain.path.display(),
                meta.id
            ),
            reinstall_hint(toolchain),
        ),
        Ok(None) => section.warning(
            format!(
                "{id} has no {META_FILE}: cxvm did not install it, or it was copied into the \
                 store"
            ),
            reinstall_hint(toolchain),
        ),
        Err(err) => section.report(Severity::Warning, &err),
    }
}

/// Whether the compilers of `toolchain` run, report its version, and find their libraries.
fn check_toolchain(section: &mut Section, toolchain: &Installed, ldd: bool) {
    if let Err(err) = integrations::check_drivers(toolchain) {
        section.report(Severity::Error, &err);
        return;
    }
    let id = &toolchain.id;
    let fix = || match toolchain.linked {
        true => format!(
            "register the install again with `cxvm unlink {id}` and `cxvm link {}`",
            toolchain.path.display()
        ),
        false => reinstall_hint(toolchain),
    };
    match detect::probe(&toolchain.path, id.kind()) {
        Ok(detected) if detected.version.to_string() == commands::numbers(id) => {}
        Ok(detected) if toolchain.linked => section.warning(
            format!(
                "{id} now reports version {}: the install in {} changed",
                detected.version,
                toolchain.path.display()
            ),
            fix(),
        ),
        Ok(detected) => section.error(
            format!(
                "{} reports version {}, not {}",
                detected.cc.display(),
                detected.version,
                commands::numbers(id)
            ),
            fix(),
        ),
        Err(Error::Detect { reason, .. }) => {
            section.error(format!("{id} does not run as it should: {reason}"), fix());
        }
        Err(err) => section.report(Severity::Error, &err),
    }
    if !ldd {
        return;
    }
    let mut binaries = vec![toolchain.cc.clone(), toolchain.cxx.clone()];
    // GCC's drivers are small; the compiler proper is `cc1plus`.
    if id.kind() == Kind::Gcc {
        binaries.extend(detect::prog_name(&toolchain.cxx, "cc1plus"));
    }
    let mut missing: Vec<String> = Vec::new();
    for binary in &binaries {
        for library in smoke::missing_libraries(binary) {
            if !missing.contains(&library) {
                missing.push(library);
            }
        }
    }
    if !missing.is_empty() {
        section.error(
            format!(
                "{id} cannot run here: missing shared libraries {}",
                missing.join(", ")
            ),
            smoke::missing_libraries_hint(&missing, "run `cxvm doctor` again"),
        );
    }
}

/// The global default in `config.toml`.
fn check_global(ctx: &Context, section: &mut Section, toolchains: &[Installed]) {
    let config_file = ctx.paths.config_file();
    let spec = match Config::load(&config_file) {
        Ok(Config {
            compiler: Some(spec),
        }) => spec,
        Ok(Config { compiler: None }) => return,
        Err(err) => return section.report(Severity::Error, &err),
    };
    match &spec {
        Spec::System => section.ok("global default: system".into()),
        Spec::Toolchain { .. } => match select(&spec, toolchains) {
            Some(toolchain) if !usable(toolchain) => section.note(broken(toolchain), None),
            Some(toolchain) if spec.is_exact() => {
                section.ok(format!("global default: {}", toolchain.id));
            }
            Some(toolchain) => {
                section.ok(format!("global default: {spec}, that is {}", toolchain.id));
            }
            None => {
                let context = format!(" (global default in {})", config_file.display());
                match commands::not_installed_error(&ctx.store(), &spec, context) {
                    Ok(err) | Err(err) => section.report(Severity::Error, &err),
                }
            }
        },
    }
}

/// What `staging/` holds: in use by a cxvm that runs now, or left by one that was stopped.
fn check_staging(ctx: &Context, section: &mut Section) {
    let store = ctx.store();
    let staged = match store.staged() {
        Ok(staged) => staged,
        Err(err) => return section.report(Severity::Warning, &err),
    };
    if staged.is_empty() {
        return;
    }
    let running = store.lock_is_held().unwrap_or(false);
    for path in staged {
        if running {
            section.note(
                format!(
                    "{} belongs to a cxvm process that is running now",
                    path.display()
                ),
                None,
            );
        } else {
            section.warning(
                format!(
                    "{} was left by an interrupted install or removal",
                    path.display()
                ),
                "the next `cxvm install`, `remove`, `link`, `unlink` or `prune` deletes it",
            );
        }
    }
}

/// The toolchain that is active in the current directory, as `cxvm current` resolves it.
fn check_active(ctx: &Context, toolchains: &[Installed]) -> Section {
    let Resolution { spec, source } = match ctx.resolve() {
        Ok(resolution) => resolution,
        Err(err) => {
            let mut section = Section::new("Active toolchain".into());
            match project::find_rc(&ctx.cwd).filter(|rc| ProjectRc::load(rc).is_err()) {
                // The project section reports the file.
                Some(rc) => section.note(
                    format!(
                        "cannot tell which toolchain is active until {} is fixed",
                        rc.display()
                    ),
                    None,
                ),
                None => section.report(Severity::Error, &err),
            }
            return section;
        }
    };
    let mut section = Section::new(format!("Active toolchain: {spec} ({source})"));
    if let Some(value) = ctx.env_compiler.as_deref().filter(|v| !v.is_empty()) {
        section.note(
            format!(
                "{}={value} overrides .cxvmrc and the global default in this shell",
                project::ENV_VAR
            ),
            Some(format!("unset {} to use them", project::ENV_VAR)),
        );
    }
    match &spec {
        Spec::System => {
            match toolchain::find_system("cc", ctx.path_var.as_deref(), ctx.paths.data_dir()) {
                Some(cc) => section.ok(format!("the system `cc` is {}", cc.display())),
                None => section.error(
                    "there is no `cc` on PATH for `system`".into(),
                    "install a C and C++ compiler from your distribution (`build-essential` on \
                     Debian/Ubuntu, `gcc gcc-c++` on Fedora/RHEL), or pin a toolchain with \
                     `cxvm use <id>`",
                ),
            }
        }
        Spec::Toolchain { .. } => match select(&spec, toolchains) {
            Some(toolchain) if !usable(toolchain) => section.note(broken(toolchain), None),
            Some(toolchain) => {
                section.ok(format!("{} in {}", toolchain.id, toolchain.path.display()));
            }
            None => {
                let context = format!(" ({source})");
                match commands::not_installed_error(&ctx.store(), &spec, context) {
                    Ok(err) | Err(err) => section.report(Severity::Error, &err),
                }
            }
        },
    }
    section
}

/// The toolchain `spec` selects among `toolchains`, as `Store::find` would.
fn select<'a>(spec: &Spec, toolchains: &'a [Installed]) -> Option<&'a Installed> {
    let id = spec.select(toolchains.iter().map(|t| &t.id))?;
    toolchains.iter().find(|t| &t.id == id)
}

/// A selected toolchain that cannot run, whose error the store section reports.
fn broken(toolchain: &Installed) -> String {
    format!(
        "{} is selected, but its compilers are missing (see the store above)",
        toolchain.id
    )
}

/// Whether both drivers of `toolchain` are there to run.
fn usable(toolchain: &Installed) -> bool {
    [&toolchain.cc, &toolchain.cxx]
        .into_iter()
        .all(|driver| files::is_executable(driver))
}

/// The project of the current directory, if it is in one: its `.cxvmrc`, and what cxvm
/// generated from it.
fn check_project(ctx: &Context, toolchains: &[Installed]) -> Option<Section> {
    let rc_path = project::find_rc(&ctx.cwd)?;
    let root = rc_path.parent().unwrap_or(&ctx.cwd).to_owned();
    let mut section = Section::new(format!("Project: {}", root.display()));
    let rc = match ProjectRc::load(&rc_path) {
        Ok(rc) => rc,
        Err(err) => {
            section.report(Severity::Error, &err);
            return Some(section);
        }
    };
    let activation = project_activation(ctx, &mut section, &rc_path, &rc, toolchains);
    if let Some((activation, prefix)) = &activation {
        check_project_dir(&mut section, &root, activation, *prefix);
        check_presets(&mut section, &root, &rc, activation, toolchains);
    }
    let activation = activation.as_ref().map(|(activation, _)| activation);
    check_build_dirs(&mut section, &root, activation, toolchains);
    Some(section)
}

/// What the project's pin activates, and the prefix `.cxvm/current` must point at. `None`
/// when the pin, or the GCC a clang is paired with, cannot be used, which is reported.
fn project_activation<'a>(
    ctx: &Context,
    section: &mut Section,
    rc_path: &Path,
    rc: &ProjectRc,
    toolchains: &'a [Installed],
) -> Option<(Activation, Option<&'a Path>)> {
    let spec = &rc.compiler;
    let Spec::Toolchain { .. } = spec else {
        let activation = Activation::for_system(ctx.path_var.as_deref(), ctx.paths.data_dir());
        return Some((activation, None));
    };
    let overridden = ctx.env_compiler.as_deref().is_some_and(|v| !v.is_empty());
    let toolchain = match select(spec, toolchains) {
        Some(toolchain) if usable(toolchain) => toolchain,
        Some(toolchain) => {
            section.note(broken(toolchain), None);
            return None;
        }
        // The active toolchain section reports it, unless CXVM_COMPILER replaced the pin.
        None if !overridden => {
            section.note(
                format!(
                    "{PROJECT_DIR}/ was not checked: the pinned {spec} is not installed (see \
                     above)"
                ),
                None,
            );
            return None;
        }
        None => {
            let context = format!(" (set by {})", rc_path.display());
            match commands::not_installed_error(&ctx.store(), spec, context) {
                Ok(err) | Err(err) => section.report(Severity::Error, &err),
            }
            return None;
        }
    };
    let gcc = match &rc.clang_gcc {
        Some(gcc_spec) if toolchain.id.kind() == Kind::Clang => {
            match select(gcc_spec, toolchains) {
                Some(gcc) if usable(gcc) => Some(gcc),
                Some(gcc) => {
                    section.note(broken(gcc), None);
                    return None;
                }
                None => {
                    let context = commands::clang_gcc_context(rc_path);
                    match commands::not_installed_error(&ctx.store(), gcc_spec, context) {
                        Ok(err) | Err(err) => section.report(Severity::Error, &err),
                    }
                    return None;
                }
            }
        }
        _ => None,
    };
    match Activation::for_toolchain(toolchain, gcc) {
        Ok(activation) => {
            if let Some(pairing) = &activation.pairing {
                section.ok(format!(
                    "{} uses the headers and libstdc++ of {} ({})",
                    toolchain.id, pairing.gcc, pairing.flag
                ));
            }
            Some((activation, Some(toolchain.path.as_path())))
        }
        Err(err) => {
            section.report(Severity::Error, &err);
            None
        }
    }
}

/// Whether `a` and `b` name the same file, after following symlinks where they resolve.
fn same_path(a: &Path, b: &Path) -> bool {
    let resolve = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_owned());
    resolve(a) == resolve(b)
}

/// `.cxvm/`: whether `current`, `toolchain.cmake` and `env` are what `cxvm install` would
/// write for `activation` now, `prefix` being where `current` must point.
fn check_project_dir(
    section: &mut Section,
    root: &Path,
    activation: &Activation,
    prefix: Option<&Path>,
) {
    let dir = root.join(PROJECT_DIR);
    let label = &activation.label;
    let regenerate = format!("`cxvm install` in {} regenerates it", root.display());
    if !dir.is_dir() {
        section.warning(
            format!(
                "{} is missing: nothing was generated here yet",
                dir.display()
            ),
            regenerate,
        );
        return;
    }
    let before = section.findings();
    let current = dir.join("current");
    match (current.symlink_metadata(), prefix) {
        (Err(_), None) => {}
        (Err(_), Some(_)) => {
            section.warning(format!("{} is missing", current.display()), &regenerate)
        }
        (Ok(meta), _) if !meta.is_symlink() => section.error(
            format!("{} is not a symlink", current.display()),
            "cxvm owns the .cxvm folder: move what you keep there elsewhere, then run `cxvm \
             install`",
        ),
        (Ok(_), None) => section.warning(
            format!(
                "{} is left from an earlier pin: `system` has none",
                current.display()
            ),
            "`cxvm install` removes it",
        ),
        (Ok(_), Some(prefix)) => {
            let target = fs::read_link(&current).unwrap_or_default();
            if !target.exists() {
                section.error(
                    format!(
                        "{} points at {}, which is gone",
                        current.display(),
                        target.display()
                    ),
                    format!("`cxvm install` in {} points it at {label}", root.display()),
                );
            } else if !same_path(&target, prefix) {
                section.warning(
                    format!(
                        "{} points at {}, not at {label} in {}",
                        current.display(),
                        target.display(),
                        prefix.display()
                    ),
                    &regenerate,
                );
            }
        }
    }
    let files = [
        (
            dir.join("toolchain.cmake"),
            integrations::render_toolchain_cmake(activation),
        ),
        (dir.join("env"), integrations::render_env_file(activation)),
    ];
    for (path, expected) in files {
        check_generated(section, &path, &expected, label, &regenerate);
    }
    if section.findings() == before {
        section.ok(format!("{PROJECT_DIR}/ matches {label}"));
    }
}

/// A file cxvm generated for `label`, which must read `expected` now.
fn check_generated(section: &mut Section, path: &Path, expected: &str, label: &str, fix: &str) {
    match fs::read_to_string(path) {
        Ok(text) if text == expected => {}
        Ok(text) => {
            let why = match integrations::generated_for(&text) {
                Some(old) if old != label => {
                    format!("it was generated for {old}, and the project pins {label}")
                }
                _ => format!("the paths of {label} changed since it was generated"),
            };
            section.warning(format!("{} is out of date: {why}", path.display()), fix);
        }
        Err(_) => section.warning(format!("{} is missing", path.display()), fix),
    }
}

/// cxvm's presets in `CMakeUserPresets.json`: each for a toolchain that is there, with its
/// toolchain file, and with `cmake_presets` one for the pin.
fn check_presets(
    section: &mut Section,
    root: &Path,
    rc: &ProjectRc,
    activation: &Activation,
    toolchains: &[Installed],
) {
    let file = root.join(presets::FILE);
    let names = match presets::names(root) {
        Ok(names) => names,
        Err(err) => return section.report(Severity::Error, &err),
    };
    let before = section.findings();
    for name in &names {
        let Some(label) = presets::label(name) else {
            continue;
        };
        let toolchain_file = root.join(presets::toolchain_file(&label));
        let installed = label == "system" || toolchains.iter().any(|t| t.id.to_string() == label);
        if !installed {
            let again = match label.split_once('-') {
                Some(_) => format!("register it again with `cxvm link <prefix> --as {label}`"),
                None => format!("install it again with `cxvm install {label}`"),
            };
            section.warning(
                format!(
                    "the preset {name} in {} is for {label}, which is not installed",
                    file.display()
                ),
                format!(
                    "delete the preset (cxvm leaves it, since presets of yours may inherit from \
                     it), or {again}"
                ),
            );
        } else if label == activation.label {
            let fix = format!("`cxvm install` in {} regenerates it", root.display());
            let expected = integrations::render_toolchain_cmake(activation);
            check_generated(section, &toolchain_file, &expected, &label, &fix);
        } else if !toolchain_file.exists() {
            section.warning(
                format!(
                    "{}, the toolchain file of the preset {name}, is missing",
                    toolchain_file.display()
                ),
                format!("`cxvm use {label}` writes it again, or delete the preset"),
            );
        }
    }
    let name = presets::preset_name(&activation.label);
    if rc.integrations.cmake_presets && !names.contains(&name) {
        section.warning(
            format!("{} has no preset {name} for the pin", file.display()),
            format!("`cxvm install` in {} adds it", root.display()),
        );
    }
    if !names.is_empty() && section.findings() == before {
        section.ok(format!(
            "the presets of cxvm in {} are up to date",
            presets::FILE
        ));
    }
}

/// The compilers `activation` makes CMake use, by language.
fn activation_compilers(activation: &Activation) -> Vec<(Lang, Compiler)> {
    let arg = activation
        .pairing
        .as_ref()
        .map(|pairing| pairing.flag.clone())
        .unwrap_or_default();
    [(Lang::Cxx, &activation.cxx), (Lang::C, &activation.cc)]
        .into_iter()
        .filter_map(|(lang, path)| {
            let compiler = Compiler {
                path: path.clone()?,
                arg: arg.clone(),
            };
            Some((lang, compiler))
        })
        .collect()
}

/// What a build dir should build with, and why.
enum Expected {
    /// What the cxvm toolchain file it was configured with names now.
    ToolchainFile {
        file: PathBuf,
        label: String,
        compilers: Vec<(Lang, Compiler)>,
    },
    /// No toolchain file: the pin's compilers.
    Pin {
        label: String,
        compilers: Vec<(Lang, Compiler)>,
    },
}

/// The CMake build dirs of the project at `root`. One configured with a toolchain file of
/// cxvm must build with the compilers that file names now, unless it is named after the
/// toolchain it builds with (`build/gcc-13.3.0`, as `cxvm use` suggests); one configured
/// without a toolchain file should build with `activation`, the pin's. Build dirs with
/// toolchain files of their own, e.g. for cross compiling, are left alone.
fn check_build_dirs(
    section: &mut Section,
    root: &Path,
    activation: Option<&Activation>,
    toolchains: &[Installed],
) {
    let cxvm_dir = root.join(PROJECT_DIR);
    for build in cmake::build_dirs(root) {
        let shown = build.path.display();
        let expected = match &build.toolchain_file {
            None => match activation {
                Some(activation) => Expected::Pin {
                    label: activation.label.clone(),
                    compilers: activation_compilers(activation),
                },
                None => continue,
            },
            Some(file) => match fs::read_to_string(file) {
                Ok(text) => match integrations::generated_for(&text) {
                    Some(label) => Expected::ToolchainFile {
                        file: file.clone(),
                        label: label.to_owned(),
                        compilers: cmake::toolchain_compilers(&text),
                    },
                    None => {
                        debug!(
                            "not checking {shown}: {} is not a toolchain file of cxvm",
                            file.display()
                        );
                        continue;
                    }
                },
                Err(_) if file.starts_with(&cxvm_dir) => {
                    section.warning(
                        format!(
                            "{shown} was configured with {}, which is gone",
                            file.display()
                        ),
                        format!("delete {shown} if you no longer need it"),
                    );
                    continue;
                }
                Err(_) => {
                    debug!("not checking {shown}: {} is gone", file.display());
                    continue;
                }
            },
        };
        let reconfigure = format!(
            "delete {shown} and configure it again: CMake keeps the compiler a build dir was \
             first configured with"
        );
        if let Some((lang, gone)) = (build.compilers.iter()).find(|(_, c)| !c.path.exists()) {
            section.error(
                format!(
                    "{shown} builds {} with {}, which is gone",
                    lang.name(),
                    gone.path.display()
                ),
                reconfigure,
            );
            continue;
        }
        let (Expected::ToolchainFile { compilers, .. } | Expected::Pin { compilers, .. }) =
            &expected;
        let mismatch = build.compilers.iter().find_map(|(lang, actual)| {
            let (_, wanted) = compilers.iter().find(|(l, _)| l == lang)?;
            (!actual.same_as(wanted)).then_some((*lang, actual, wanted))
        });
        let Some((lang, actual, wanted)) = mismatch else {
            let label = match &expected {
                Expected::ToolchainFile { label, .. } | Expected::Pin { label, .. } => label,
            };
            section.ok(format!("{shown} builds with {label}"));
            continue;
        };
        // A build dir per toolchain, named after the one it builds with.
        let named_after = toolchains.iter().find(|toolchain| {
            let slug = integrations::slug(&toolchain.id.to_string());
            build.path.file_name() == Some(slug.as_ref())
                && build.compilers.iter().all(|(lang, compiler)| {
                    let driver = match lang {
                        Lang::C => &toolchain.cc,
                        Lang::Cxx => &toolchain.cxx,
                    };
                    let driver = Compiler {
                        path: driver.clone(),
                        arg: compiler.arg.clone(),
                    };
                    compiler.same_as(&driver)
                })
        });
        if let Some(toolchain) = named_after {
            section.ok(format!("{shown} builds with {}", toolchain.id));
            continue;
        }
        let lang = lang.name();
        match &expected {
            Expected::ToolchainFile { file, label, .. } => {
                let mut hint = reconfigure;
                if same_path(file, &cxvm_dir.join("toolchain.cmake")) {
                    hint.push_str(&format!(
                        "; or keep a build dir per toolchain: `cmake -S {} -B {}/build/{} \
                         -DCMAKE_TOOLCHAIN_FILE={}`",
                        root.display(),
                        root.display(),
                        integrations::slug(label),
                        file.display()
                    ));
                }
                section.error(
                    format!(
                        "{shown} builds {lang} with {actual}, but its toolchain file {} now names \
                         {wanted} ({label})",
                        file.display()
                    ),
                    hint,
                );
            }
            Expected::Pin { label, .. } => section.warning(
                format!(
                    "{shown} builds {lang} with {actual}, and the project pins {label} ({wanted})"
                ),
                format!(
                    "if it should follow the pin, delete it and configure it again with \
                     `-DCMAKE_TOOLCHAIN_FILE={}`",
                    cxvm_dir.join("toolchain.cmake").display()
                ),
            ),
        }
    }
}

/// The download cache and the build logs, which `cxvm prune` deletes.
fn check_cache(ctx: &Context) -> Section {
    let mut section = Section::new(format!("Cache: {}", ctx.paths.cache_dir().display()));
    match commands::cache_entries(&ctx.paths) {
        Ok(entries) if entries.is_empty() => section.ok("no downloads or build logs".into()),
        Ok(entries) => {
            let total: u64 = entries.iter().map(|(_, size)| size).sum();
            section.note(
                format!(
                    "downloads and build logs take {} ({})",
                    human_size(total),
                    plural(entries.len(), "file")
                ),
                Some("nothing installed needs them; `cxvm prune` deletes them".into()),
            );
        }
        Err(err) => section.report(Severity::Warning, &err),
    }
    section
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> String {
        anstream::adapter::strip_str(text).to_string()
    }

    #[test]
    fn the_report_indents_findings_and_their_hints() {
        let sections = [
            Section {
                title: "Store: /s".into(),
                lines: vec![
                    Line::Ok("gcc@13.3.0 in /s/toolchains/gcc/13.3.0".into()),
                    Line::Finding {
                        severity: Severity::Error,
                        message: "gcc@11.2.0 cannot run here".into(),
                        hint: Some("install libfoo".into()),
                    },
                ],
            },
            Section {
                title: "Cache: /c".into(),
                lines: vec![Line::Finding {
                    severity: Severity::Note,
                    message: "downloads take 1 MiB".into(),
                    hint: None,
                }],
            },
        ];
        assert_eq!(
            plain(&render(&sections)),
            "Store: /s\n\
             \x20 ok: gcc@13.3.0 in /s/toolchains/gcc/13.3.0\n\
             \x20 error: gcc@11.2.0 cannot run here\n\
             \x20   hint: install libfoo\n\
             \n\
             Cache: /c\n\
             \x20 note: downloads take 1 MiB\n"
        );
    }

    #[test]
    fn errors_become_findings_with_their_hint() {
        let mut section = Section::new("t".into());
        section.report(
            Severity::Warning,
            &Error::Usage {
                message: "it is broken".into(),
                hint: "fix it".into(),
            },
        );
        assert_eq!(
            section.lines,
            [Line::Finding {
                severity: Severity::Warning,
                message: "it is broken".into(),
                hint: Some("fix it".into()),
            }]
        );
    }

    #[test]
    fn counts_read_naturally() {
        assert_eq!(plural(1, "error"), "1 error");
        assert_eq!(plural(2, "warning"), "2 warnings");
        assert_eq!(plural(0, "file"), "0 files");
    }
}
