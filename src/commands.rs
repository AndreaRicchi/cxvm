//! Implementations of the commands, on top of the library modules. `cli` parses arguments and
//! calls into here with a [`Context`] built from the process environment.

use std::path::PathBuf;

use serde::Serialize;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::project::{self, Resolution};
use crate::store::Store;
use crate::toolchain::Spec;

/// Everything a command reads from the process environment.
#[derive(Debug, Clone)]
pub struct Context {
    pub paths: Paths,
    pub cwd: PathBuf,
    /// Value of `CXVM_COMPILER`.
    pub env_compiler: Option<String>,
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
        let flags: Vec<&str> = [(entry.active, "active"), (entry.global, "global")]
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
