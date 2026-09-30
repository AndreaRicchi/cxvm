//! Implementations of the commands, on top of the library modules. `cli` parses arguments and
//! calls into here with a [`Context`] built from the process environment.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::config::Config;
use crate::detect;
use crate::error::{Error, Result};
use crate::files;
use crate::paths::Paths;
use crate::project::{self, Resolution, Source};
use crate::store::{Installed, Link, Store};
use crate::toolchain::{self, Kind, Spec, ToolchainId};

/// Everything a command reads from the process environment.
#[derive(Debug, Clone)]
pub struct Context {
    pub paths: Paths,
    pub cwd: PathBuf,
    /// Value of `CXVM_COMPILER`.
    pub env_compiler: Option<String>,
    /// Value of `PATH`, to find the `system` compiler.
    pub path_var: Option<OsString>,
}

impl Context {
    pub fn from_process_env() -> Result<Self> {
        let paths = Paths::from_process_env()?;
        let cwd = std::env::current_dir().map_err(|source| Error::Io {
            action: "read",
            path: ".".into(),
            source,
            hint: "cd into a directory that still exists",
        })?;
        let env_compiler = match std::env::var(project::ENV_VAR) {
            Ok(value) => Some(value),
            Err(std::env::VarError::NotPresent) => None,
            Err(std::env::VarError::NotUnicode(value)) => {
                return Err(Error::InvalidEnvCompiler {
                    input: value.to_string_lossy().into_owned(),
                    reason: "not valid UTF-8".into(),
                });
            }
        };
        Ok(Self {
            paths,
            cwd,
            env_compiler,
            path_var: std::env::var_os("PATH"),
        })
    }

    pub fn store(&self) -> Store {
        Store::new(self.paths.clone())
    }

    /// The toolchain that should be active here, and why.
    pub fn resolve(&self) -> Result<Resolution> {
        project::resolve(
            &self.cwd,
            self.env_compiler.as_deref(),
            &self.paths.config_file(),
        )
    }
}

#[derive(Debug, Serialize)]
struct ListEntry {
    id: String,
    path: String,
    active: bool,
    global: bool,
    linked: bool,
}

/// `cxvm list`: installed toolchains, marking the active one and the global default.
pub fn list(ctx: &Context, json: bool) -> Result<()> {
    let installed = ctx.store().installed()?;
    let ids = || installed.iter().map(|t| &t.id);
    let resolution = ctx.resolve()?;
    let active = resolution.spec.select(ids());
    let config_file = ctx.paths.config_file();
    let global_spec = Config::load(&config_file)?.compiler;
    let global = global_spec.as_ref().and_then(|spec| spec.select(ids()));

    let entries: Vec<ListEntry> = installed
        .iter()
        .map(|t| ListEntry {
            id: t.id.to_string(),
            path: t.path.display().to_string(),
            active: active == Some(&t.id),
            global: global == Some(&t.id),
            linked: t.linked,
        })
        .collect();

    if json {
        let text = serde_json::to_string_pretty(&entries).expect("list entries serialize");
        println!("{text}");
        return Ok(());
    }

    if entries.is_empty() {
        println!(
            "No toolchains installed in {}.\n  \
             hint: install one with `cxvm install <id>`, \
             or register an existing one with `cxvm link <prefix>`",
            ctx.paths.toolchains_dir().display()
        );
    }
    let width = entries.iter().map(|e| e.id.len()).max().unwrap_or(0);
    for entry in &entries {
        let marker = if entry.active { '*' } else { ' ' };
        let flags: Vec<&str> = [
            (entry.active, "active"),
            (entry.global, "global"),
            (entry.linked, "linked"),
        ]
        .into_iter()
        .filter_map(|(set, name)| set.then_some(name))
        .collect();
        let line = format!("{marker} {:<width$}  {}", entry.id, flags.join(", "));
        println!("{}", line.trim_end());
    }

    let mut notes = Vec::new();
    if active.is_none() {
        notes.push(format!(
            "active: {} ({}){}",
            resolution.spec,
            resolution.source,
            not_installed(&resolution.spec)
        ));
    }
    if let Some(spec) = global_spec.filter(|_| global.is_none()) {
        notes.push(format!(
            "global: {spec} (in {}){}",
            config_file.display(),
            not_installed(&spec)
        ));
    }
    if !notes.is_empty() {
        if !entries.is_empty() {
            println!();
        }
        for note in notes {
            println!("{note}");
        }
    }
    Ok(())
}

/// Suffix for a spec that matched nothing in the store.
fn not_installed(spec: &Spec) -> &'static str {
    match spec {
        Spec::System => "",
        Spec::Toolchain { .. } => ", not installed",
    }
}

#[derive(Debug, Serialize)]
struct CurrentReport {
    id: Option<String>,
    requested: String,
    source: &'static str,
    source_path: Option<String>,
    installed: bool,
    path: Option<String>,
}

/// `cxvm current`: the active toolchain and why. Fails, after printing, when the resolved
/// toolchain is not installed.
pub fn current(ctx: &Context, json: bool) -> Result<()> {
    let store = ctx.store();
    let Resolution { spec, source } = ctx.resolve()?;
    let (id, path, installed) = match &spec {
        Spec::System => {
            let cc = toolchain::find_system("cc", ctx.path_var.as_deref(), ctx.paths.data_dir());
            (Some("system".to_owned()), cc, true)
        }
        Spec::Toolchain { .. } => match store.find(&spec)? {
            Some(Installed { id, path, .. }) => (Some(id.to_string()), Some(path), true),
            None => (None, None, false),
        },
    };

    if json {
        let (source_name, source_path) = match &source {
            Source::Env => ("env", None),
            Source::ProjectRc(path) => ("cxvmrc", Some(path)),
            Source::Global(path) => ("global", Some(path)),
            Source::Default => ("default", None),
        };
        let report = CurrentReport {
            id: id.clone(),
            requested: spec.to_string(),
            source: source_name,
            source_path: source_path.map(|p| p.display().to_string()),
            installed,
            path: path.as_ref().map(|p| p.display().to_string()),
        };
        let text = serde_json::to_string_pretty(&report).expect("report serializes");
        println!("{text}");
    } else {
        let requested = spec.to_string();
        match id.as_deref() {
            Some(id) if id != requested => println!("{id} ({requested}, {source})"),
            _ => println!("{requested} ({source})"),
        }
        match (&path, &spec) {
            (Some(path), _) => println!("{}", path.display()),
            (None, Spec::System) => println!("no `cc` found on PATH"),
            (None, Spec::Toolchain { .. }) => {}
        }
    }

    if installed {
        Ok(())
    } else {
        Err(not_installed_error(&store, &spec, &source)?)
    }
}

/// The error for a toolchain spec with no match in the store, with the next step to take.
fn not_installed_error(store: &Store, spec: &Spec, source: &Source) -> Result<Error> {
    let mut hint = match spec {
        Spec::Toolchain { kind, version } if version.suffix().is_some() => format!(
            "register it with `cxvm link <prefix> --as {kind}@<x.y.z>-{}`",
            version.suffix().unwrap_or_default()
        ),
        _ => format!("run `cxvm install {spec}`"),
    };
    if let Spec::Toolchain { kind, .. } = spec {
        let same_kind: Vec<String> = store
            .installed()?
            .into_iter()
            .filter(|t| t.id.kind() == *kind)
            .map(|t| t.id.to_string())
            .collect();
        if !same_kind.is_empty() {
            hint.push_str(&format!(
                ", or pin an installed version: {}",
                same_kind.join(", ")
            ));
        }
    }
    Ok(Error::NotInstalled {
        spec: spec.to_string(),
        reason: source.to_string(),
        hint,
    })
}

/// `cxvm link`: register the toolchain in `prefix` under a detected `-local` id, or `as_id`.
pub fn link(ctx: &Context, prefix: &Path, as_id: Option<&str>) -> Result<()> {
    let prefix = files::absolute(&ctx.cwd, prefix);
    if !prefix.is_dir() {
        return Err(Error::Usage {
            message: format!("{} is missing or not a directory", prefix.display()),
            hint: "pass the install prefix, the directory that contains `bin/`".into(),
        });
    }
    let store = ctx.store();
    let data_dir = ctx.paths.data_dir();
    let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_owned());
    if canonical(&prefix).starts_with(canonical(data_dir)) {
        return Err(Error::Usage {
            message: format!("{} is inside the cxvm store", prefix.display()),
            hint: "toolchains in the store are available already; see `cxvm list`".into(),
        });
    }

    let as_id = as_id.map(parse_link_id).transpose()?;
    let (id, detected) = match &as_id {
        Some(id) => (id.clone(), probe_as(&prefix, id)?),
        None => {
            let detected = probe_any(&prefix)?;
            let version = format!("{}-local", detected.version)
                .parse()
                .expect("an exact version with a suffix parses");
            let id = ToolchainId::new(detected.kind, version).expect("the version is exact");
            (id, Some(detected))
        }
    };
    let (cc, cxx) = detect::compilers(&prefix, id.kind());
    let version_line = detected.map(|d| d.version_line).unwrap_or_default();

    let mut links = store.links()?;
    if let Some(existing) = links.iter().find(|link| link.id == id) {
        if existing.prefix == prefix {
            println!("{id} is already linked to {}", prefix.display());
            return Ok(());
        }
        return Err(Error::Usage {
            message: format!("{id} is already linked to {}", existing.prefix.display()),
            hint: format!(
                "pick another id with `--as {}@{}-<suffix>`, or `cxvm unlink {id}` first",
                id.kind(),
                numbers(&id)
            ),
        });
    }
    if store.installed()?.iter().any(|t| t.id == id && !t.linked) {
        return Err(Error::Usage {
            message: format!("{id} is already installed in the cxvm store"),
            hint: format!(
                "link it under another id with `--as {}@{}-<suffix>`",
                id.kind(),
                numbers(&id)
            ),
        });
    }
    links.push(Link {
        id: id.clone(),
        prefix: prefix.clone(),
        cc,
        cxx,
        version: version_line,
    });
    store.save_links(&links)?;
    println!("Linked {id} -> {}", prefix.display());
    println!("  pin it with `cxvm use {id}`");
    Ok(())
}

/// The numeric part of an exact id's version, e.g. `11.4.0` for `gcc@11.4.0-local`.
fn numbers(id: &ToolchainId) -> String {
    let version = id.version().to_string();
    match id.version().suffix() {
        Some(suffix) => version[..version.len() - suffix.len() - 1].to_owned(),
        None => version,
    }
}

/// Parses `--as`: an exact id with a suffix, so it cannot collide with store installs.
fn parse_link_id(input: &str) -> Result<ToolchainId> {
    let id: ToolchainId = input.parse()?;
    if id.version().suffix().is_none() {
        return Err(Error::Usage {
            message: format!("`--as {id}` has no suffix"),
            hint: format!(
                "linked ids need a suffix so they never clash with installed ones, e.g. `{id}-local`"
            ),
        });
    }
    Ok(id)
}

/// Detects the only kind of compiler in `prefix`.
fn probe_any(prefix: &Path) -> Result<detect::Detected> {
    let kinds = detect::kinds_present(prefix);
    match kinds.as_slice() {
        [kind] => detect::probe(prefix, *kind),
        [] => Err(Error::Detect {
            prefix: prefix.to_owned(),
            reason: "it has neither bin/gcc nor bin/clang".into(),
            hint: "pass the install prefix, the directory that contains `bin/`".into(),
        }),
        _ => Err(Error::Detect {
            prefix: prefix.to_owned(),
            reason: "it holds both bin/gcc and bin/clang".into(),
            hint: "say which one to link with `--as gcc@<x.y.z>-<suffix>` or \
                   `--as clang@<x.y.z>-<suffix>`"
                .into(),
        }),
    }
}

/// Checks that `prefix` holds the compilers `id` claims. Returns the probe result, or `None`
/// with a warning when the version cannot be detected and `id` is taken on trust.
fn probe_as(prefix: &Path, id: &ToolchainId) -> Result<Option<detect::Detected>> {
    let (cc, cxx) = detect::compilers(prefix, id.kind());
    for driver in [&cc, &cxx] {
        if !files::is_executable(driver) {
            return Err(Error::Detect {
                prefix: prefix.to_owned(),
                reason: format!("{} is missing or not executable", driver.display()),
                hint: match id.kind() {
                    Kind::Gcc => "a GCC prefix needs bin/gcc and bin/g++",
                    Kind::Clang => "a Clang prefix needs bin/clang and bin/clang++",
                }
                .into(),
            });
        }
    }
    match detect::probe(prefix, id.kind()) {
        Ok(detected) => {
            if detected.version.to_string() != numbers(id) {
                return Err(Error::Usage {
                    message: format!(
                        "`--as {id}` does not match: {} reports version {}",
                        detected.cc.display(),
                        detected.version
                    ),
                    hint: format!("use `--as {}@{}-<suffix>`", id.kind(), detected.version),
                });
            }
            Ok(Some(detected))
        }
        Err(err) => {
            eprintln!("warning: taking the version from `--as {id}` on trust: {err}");
            Ok(None)
        }
    }
}

/// `cxvm unlink`: forget a linked toolchain. Its files are left alone.
pub fn unlink(ctx: &Context, id: &str) -> Result<()> {
    let id: ToolchainId = id.parse()?;
    let store = ctx.store();
    let mut links = store.links()?;
    let Some(index) = links.iter().position(|link| link.id == id) else {
        if store.installed()?.iter().any(|t| t.id == id) {
            return Err(Error::Usage {
                message: format!("{id} is installed in the cxvm store, not linked"),
                hint: format!("remove it with `cxvm remove {id}`"),
            });
        }
        return Err(Error::Usage {
            message: format!("{id} is not linked"),
            hint: "`cxvm list` shows the linked toolchains".into(),
        });
    };
    let link = links.remove(index);
    store.save_links(&links)?;

    let selects = |spec: &Spec| spec.select([&id]).is_some();
    let config_file = ctx.paths.config_file();
    if Config::load(&config_file)?
        .compiler
        .as_ref()
        .is_some_and(selects)
    {
        eprintln!(
            "warning: {id} was the global default in {}; set another with `cxvm global <id>`",
            config_file.display()
        );
    }
    if let Ok(Resolution { spec, source }) = ctx.resolve()
        && selects(&spec)
        && !matches!(source, Source::Global(_))
    {
        eprintln!(
            "warning: {id} is still pinned here ({source}); pin another with `cxvm use <id>`"
        );
    }
    println!(
        "Unlinked {id}; the files in {} are left alone",
        link.prefix.display()
    );
    Ok(())
}
