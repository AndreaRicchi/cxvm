//! Implementations of the commands, on top of the library modules. `cli` parses arguments and
//! calls into here with a [`Context`] built from the process environment.

use std::ffi::OsString;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::backends;
use crate::backends::source::{self, GnuSource, HostCompiler};
use crate::catalog::{self, Catalog, Release};
use crate::config::Config;
use crate::detect;
use crate::error::{Error, Result};
use crate::files;
use crate::integrations::{self, Activation, Gitignore, Shell};
use crate::paths::Paths;
use crate::project::{self, ProjectRc, Resolution, Source};
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
    /// Value of `SHELL`, the default syntax for `cxvm env`.
    pub shell: Option<OsString>,
    /// Value of `CXVM_CATALOG`: a catalog file to use instead of the embedded one.
    pub catalog: Option<PathBuf>,
    /// Whether a person can be asked a question: stdin and stderr are terminals.
    pub interactive: bool,
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
        let catalog = std::env::var_os(catalog::ENV_VAR)
            .filter(|value| !value.is_empty())
            .map(|value| files::absolute(&cwd, Path::new(&value)));
        Ok(Self {
            paths,
            cwd,
            env_compiler,
            path_var: std::env::var_os("PATH"),
            shell: std::env::var_os("SHELL"),
            catalog,
            interactive: std::io::stdin().is_terminal() && std::io::stderr().is_terminal(),
        })
    }

    /// The installable releases: `CXVM_CATALOG`, or the catalog built into cxvm.
    pub fn catalog(&self) -> Result<Catalog> {
        Catalog::load(self.catalog.as_deref())
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

#[derive(Debug, Serialize)]
struct AvailableEntry {
    id: String,
    size: u64,
    installed: bool,
    url: String,
    /// No prebuilt archive: `cxvm install <id> --from-source` builds it.
    from_source: bool,
}

/// `cxvm list --available`: the releases cxvm can install, as a table with one row per major
/// (see [`render_available`]), or with `json` every release, newest first per kind. Source
/// releases are listed where there is no prebuilt archive.
pub fn list_available(ctx: &Context, json: bool) -> Result<()> {
    let catalog = ctx.catalog()?;
    let installed = ctx.store().installed()?;
    let is_installed = |id: &ToolchainId| installed.iter().any(|t| &t.id == id);
    let source_only: Vec<&Release> = catalog
        .source_releases()
        .iter()
        .filter(|source| !catalog.releases().iter().any(|r| r.id == source.id))
        .collect();

    if json {
        let mut releases: Vec<(&Release, bool)> = (catalog.releases().iter())
            .map(|release| (release, false))
            .chain(source_only.iter().map(|release| (*release, true)))
            .collect();
        releases.sort_by(|(a, _), (b, _)| a.id.kind().cmp(&b.id.kind()).then(b.id.cmp(&a.id)));
        let entries: Vec<AvailableEntry> = releases
            .into_iter()
            .map(|(release, from_source)| AvailableEntry {
                id: release.id.to_string(),
                size: release.size,
                installed: is_installed(&release.id),
                url: release.url.clone(),
                from_source,
            })
            .collect();
        let text = serde_json::to_string_pretty(&entries).expect("entries serialize");
        println!("{text}");
        return Ok(());
    }
    print!(
        "{}",
        render_available(
            catalog.releases(),
            &source_only,
            is_installed,
            &catalog.origin()
        )
    );
    Ok(())
}

/// Columns a line of the `list --available` table may fill. Fixed rather than read from the
/// terminal, so the output is the same in a pipe.
const AVAILABLE_WIDTH: usize = 80;

/// A major and its releases, newest first: one row of the `list --available` table.
type MajorRow<'a> = (u32, Vec<&'a Release>);

/// A table of the `list --available` output: the releases of one kind, by major.
struct AvailableTable<'a> {
    /// A line printed above the table, when it starts a new section.
    title: Option<&'static str>,
    kind: Kind,
    rows: Vec<MajorRow<'a>>,
}

const SOURCE_TITLE: &str = "No prebuilt archive, built with `cxvm install <id> --from-source`:";

/// The tables of `cxvm list --available`: per kind, one row per major holding its versions,
/// newest first, wrapped at [`AVAILABLE_WIDTH`]. `*` marks the installed ones; `download` is
/// the archive size of the row's newest version. `source_only`, the releases that can only be
/// built, follow under a title of their own.
fn render_available(
    releases: &[Release],
    source_only: &[&Release],
    is_installed: impl Fn(&ToolchainId) -> bool,
    origin: &str,
) -> String {
    let mut out = format!("Installable toolchains (* = installed), from {origin}:\n");
    let mut tables: Vec<AvailableTable> = Vec::new();
    let sections: [(Option<&'static str>, Vec<&Release>); 2] = [
        (None, releases.iter().collect()),
        (Some(SOURCE_TITLE), source_only.to_vec()),
    ];
    for (mut title, section) in sections {
        for kind in Kind::ALL {
            let mut of_kind: Vec<&Release> = (section.iter().copied())
                .filter(|r| r.id.kind() == kind)
                .collect();
            of_kind.sort_by(|a, b| b.id.cmp(&a.id));
            let mut rows: Vec<MajorRow> = Vec::new();
            for release in of_kind {
                let major = release.id.version().major();
                match rows.last_mut() {
                    Some((last, versions)) if *last == major => versions.push(release),
                    _ => rows.push((major, vec![release])),
                }
            }
            if !rows.is_empty() {
                tables.push(AvailableTable {
                    title: title.take(),
                    kind,
                    rows,
                });
            }
        }
    }
    if tables.is_empty() {
        return out;
    }

    let rows = || tables.iter().flat_map(|table| &table.rows);
    let first = Kind::ALL
        .map(|kind| kind.as_str().len())
        .into_iter()
        .max()
        .unwrap_or(0);
    let size = rows()
        .map(|(_, versions)| human_size(versions[0].size).len())
        .max()
        .unwrap_or(0)
        .max("download".len());
    // One more than the longest version, for the `*`.
    let cell = rows()
        .flat_map(|(_, versions)| versions)
        .map(|release| release.id.version().to_string().len() + 1)
        .max()
        .unwrap_or(0);
    let indent = 2 + first + 2 + size + 2;
    let per_line = (AVAILABLE_WIDTH.saturating_sub(indent) + 1) / (cell + 1);

    for table in &tables {
        if let Some(title) = table.title {
            out.push_str(&format!("\n{title}\n"));
        }
        out.push_str(&format!(
            "\n  {:<first$}  {:>size$}  versions\n",
            table.kind.as_str(),
            "download"
        ));
        for (major, versions) in &table.rows {
            let cells: Vec<String> = versions
                .iter()
                .map(|release| {
                    let mark = if is_installed(&release.id) { "*" } else { "" };
                    format!("{:<cell$}", format!("{}{mark}", release.id.version()))
                })
                .collect();
            let mut start = format!(
                "  {major:<first$}  {:>size$}  ",
                human_size(versions[0].size)
            );
            for line in cells.chunks(per_line.max(1)) {
                out.push_str(format!("{start}{}", line.join(" ")).trim_end());
                out.push('\n');
                start = " ".repeat(indent);
            }
        }
    }
    out
}

/// `1.8 GiB`, `950 MiB`: download sizes for humans.
pub(crate) fn human_size(bytes: u64) -> String {
    const MIB: u64 = 1 << 20;
    const GIB: u64 = 1 << 30;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else {
        format!("{} MiB", bytes.div_ceil(MIB))
    }
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
    /// The GCC a clang is paired with by `[clang] gcc`, or null.
    clang_gcc: Option<PairingReport>,
}

#[derive(Debug, Serialize)]
struct PairingReport {
    id: Option<String>,
    requested: String,
    source_path: String,
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
    // The pairing of an installed clang, and the GCC it selects if that is installed.
    let is_clang = matches!(
        spec,
        Spec::Toolchain {
            kind: Kind::Clang,
            ..
        }
    );
    let pairing = match project::find_rc(&ctx.cwd) {
        Some(rc) if is_clang && installed => match ProjectRc::load(&rc)?.clang_gcc {
            Some(request) => {
                let gcc = store.find(&request)?;
                Some((request, rc, gcc))
            }
            None => None,
        },
        _ => None,
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
            clang_gcc: pairing.as_ref().map(|(request, rc, gcc)| PairingReport {
                id: gcc.as_ref().map(|gcc| gcc.id.to_string()),
                requested: request.to_string(),
                source_path: rc.display().to_string(),
                installed: gcc.is_some(),
                path: gcc.as_ref().map(|gcc| gcc.path.display().to_string()),
            }),
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
        if let Some((request, rc, gcc)) = &pairing {
            let rc = rc.display();
            match gcc {
                Some(gcc) => println!("with {} (`[clang] gcc` in {rc})", gcc.id),
                None => println!("with {request} (`[clang] gcc` in {rc}, not installed)"),
            }
        }
    }

    if !installed {
        return Err(not_installed_error(&store, &spec, format!(" ({source})"))?);
    }
    match &pairing {
        Some((request, rc, None)) => {
            Err(not_installed_error(&store, request, clang_gcc_context(rc))?)
        }
        _ => Ok(()),
    }
}

/// The error for a toolchain spec with no match in the store, with the next step to take.
/// `context` follows "is not installed", e.g. ` (set by /p/.cxvmrc)`.
fn not_installed_error(store: &Store, spec: &Spec, context: String) -> Result<Error> {
    let mut hint = match spec {
        Spec::Toolchain { kind, version } if version.suffix().is_some() => format!(
            "register it with `cxvm link <prefix> --as {kind}@<x.y.z>-{}`",
            version.suffix().unwrap_or_default()
        ),
        _ => format!(
            "run `cxvm install {spec}`, or register an existing install with \
             `cxvm link <prefix>`"
        ),
    };
    if let Spec::Toolchain { kind, version } = spec {
        let same_kind: Vec<ToolchainId> = store
            .installed()?
            .into_iter()
            .filter(|t| t.id.kind() == *kind)
            .map(|t| t.id)
            .collect();
        // A linked toolchain that only its suffix keeps from matching.
        if version.suffix().is_none()
            && let Some(suffix) = same_kind.iter().rev().find_map(|id| {
                let suffix = id.version().suffix()?;
                let with_suffix: Spec = format!("{spec}-{suffix}").parse().ok()?;
                with_suffix.select([id]).map(|_| suffix)
            })
        {
            hint = format!(
                "linked toolchains are selected with their suffix: `{spec}-{suffix}`; or {hint}"
            );
        }
        let same_kind: Vec<String> = same_kind.iter().map(ToString::to_string).collect();
        if !same_kind.is_empty() {
            hint.push_str(&format!("; installed: {}", same_kind.join(", ")));
        }
    }
    Ok(Error::NotInstalled {
        spec: spec.to_string(),
        context,
        hint,
    })
}

/// A resolved, usable toolchain: `system` or an installed or linked one.
#[derive(Debug, Clone)]
enum Target {
    System,
    Toolchain(Installed),
}

impl Target {
    /// The installed toolchain `spec` selects. `context` goes into the not-installed error.
    fn find(store: &Store, spec: &Spec, context: String) -> Result<Self> {
        match spec {
            Spec::System => Ok(Target::System),
            Spec::Toolchain { .. } => match store.find(spec)? {
                Some(toolchain) => Ok(Target::Toolchain(toolchain)),
                None => Err(not_installed_error(store, spec, context)?),
            },
        }
    }

    /// The exact spec to record in `.cxvmrc` or `config.toml`.
    fn spec(&self) -> Spec {
        match self {
            Target::System => Spec::System,
            Target::Toolchain(toolchain) => Spec::from(toolchain.id.clone()),
        }
    }

    /// What `.cxvm/current` points at.
    fn prefix(&self) -> Option<&Path> {
        match self {
            Target::System => None,
            Target::Toolchain(toolchain) => Some(&toolchain.path),
        }
    }

    /// Whether it is a clang cxvm manages, the only target `[clang] gcc` applies to.
    fn is_clang(&self) -> bool {
        matches!(self, Target::Toolchain(toolchain) if toolchain.id.kind() == Kind::Clang)
    }

    /// `gcc` is the GCC a clang is paired with, from [`paired_gcc`].
    fn activation(&self, ctx: &Context, gcc: Option<&Installed>) -> Result<Activation> {
        match self {
            Target::System => Ok(Activation::for_system(
                ctx.path_var.as_deref(),
                ctx.paths.data_dir(),
            )),
            Target::Toolchain(toolchain) => Activation::for_toolchain(toolchain, gcc),
        }
    }
}

/// The GCC that `request` pairs `target` with, `request` being the `[clang] gcc` of the
/// `.cxvmrc` at `rc`: `None` unless `target` is a clang and a GCC is asked for. Fails if that
/// GCC is not installed.
fn paired_gcc(
    store: &Store,
    target: &Target,
    request: Option<&Spec>,
    rc: &Path,
) -> Result<Option<Installed>> {
    let (true, Some(spec)) = (target.is_clang(), request) else {
        return Ok(None);
    };
    match store.find(spec)? {
        Some(gcc) => Ok(Some(gcc)),
        None => Err(not_installed_error(store, spec, clang_gcc_context(rc))?),
    }
}

/// Where a pairing comes from, for messages: `` (`[clang] gcc` in /p/.cxvmrc)``.
fn clang_gcc_context(rc: &Path) -> String {
    format!(" (`[clang] gcc` in {})", rc.display())
}

/// The pairing that applies in the current directory for `target`: `[clang] gcc` of the
/// nearest `.cxvmrc`, whatever selected the clang itself.
fn project_paired_gcc(ctx: &Context, store: &Store, target: &Target) -> Result<Option<Installed>> {
    if !target.is_clang() {
        return Ok(None);
    }
    let Some(rc) = project::find_rc(&ctx.cwd) else {
        return Ok(None);
    };
    let request = ProjectRc::load(&rc)?.clang_gcc;
    paired_gcc(store, target, request.as_ref(), &rc)
}

/// Regenerates `<root>/.cxvm/` for `target` as `rc`, the project's `.cxvmrc`, asks: the
/// pairing of a clang, and the integrations.
fn generate_project(ctx: &Context, root: &Path, target: &Target, rc: &ProjectRc) -> Result<()> {
    let rc_path = root.join(project::RC_FILE);
    let gcc = paired_gcc(&ctx.store(), target, rc.clang_gcc.as_ref(), &rc_path)?;
    let activation = target.activation(ctx, gcc.as_ref())?;
    integrations::write_project_dir(root, &activation, target.prefix())?;
    println!(
        "Generated {} (toolchain.cmake, env{})",
        root.join(integrations::PROJECT_DIR).display(),
        if target.prefix().is_some() {
            ", current"
        } else {
            ""
        }
    );
    if let Some(pairing) = &activation.pairing {
        println!(
            "Clang uses the headers and libstdc++ of {} ({})",
            pairing.gcc, pairing.flag
        );
    }
    if rc.integrations.gitignore
        && let Gitignore::Added(path) = integrations::ensure_gitignored(root)?
    {
        println!("Added {}/ to {}", integrations::PROJECT_DIR, path.display());
    }
    Ok(())
}

/// What `cxvm use --gcc <id>` writes to `[clang] gcc`: the exact id of an installed GCC, or
/// `system`. Fails unless `target` is a clang.
fn pairing_request(store: &Store, target: &Target, gcc: &str) -> Result<Spec> {
    let spec: Spec = gcc.parse()?;
    if matches!(
        spec,
        Spec::Toolchain {
            kind: Kind::Clang,
            ..
        }
    ) {
        return Err(Error::Usage {
            message: format!("`--gcc {gcc}` does not name a GCC"),
            hint: "name the GCC whose libstdc++ clang should use, e.g. `--gcc gcc@13`, or \
                   `--gcc system` to let clang pick one on the host"
                .into(),
        });
    }
    if !target.is_clang() {
        return Err(Error::Usage {
            message: format!(
                "`--gcc` pairs a clang with a GCC, and {} is not a clang cxvm manages",
                target.spec()
            ),
            hint: "drop `--gcc`: a GCC uses its own libstdc++, and `system` is left as it is"
                .into(),
        });
    }
    match &spec {
        Spec::System => Ok(spec),
        Spec::Toolchain { .. } => match store.find(&spec)? {
            Some(gcc) => Ok(Spec::from(gcc.id)),
            None => Err(not_installed_error(store, &spec, " (--gcc)".into())?),
        },
    }
}

/// `cxvm use`: pin `id` in the current project and regenerate `.cxvm/`. With `gcc`, also pair
/// the clang being pinned with that GCC.
pub fn use_(ctx: &Context, id: &str, gcc: Option<&str>) -> Result<()> {
    let spec: Spec = id.parse()?;
    let store = ctx.store();
    let target = Target::find(&store, &spec, String::new())?;
    let exact = target.spec();
    let clang_gcc = gcc
        .map(|gcc| pairing_request(&store, &target, gcc))
        .transpose()?;

    let root = project::project_root(&ctx.cwd);
    let rc = root.join(project::RC_FILE);
    let pending = project::edit_rc(&rc, &exact, clang_gcc.as_ref())?;
    // Fail on an incomplete toolchain or pairing before touching any file.
    let paired = paired_gcc(&store, &target, pending.rc.clang_gcc.as_ref(), &rc)?;
    target.activation(ctx, paired.as_ref())?;

    pending.write()?;
    if exact == spec {
        println!("Pinned {exact} in {}", rc.display());
    } else {
        println!("Pinned {exact} (from {spec}) in {}", rc.display());
    }
    generate_project(ctx, &root, &target, &pending.rc)?;

    let build = format!("build/{}", exact.to_string().replace('@', "-"));
    let toolchain_file = format!("{}/toolchain.cmake", integrations::PROJECT_DIR);
    println!("Configure a build with:");
    if root == ctx.cwd {
        println!("  cmake -S . -B {build} -DCMAKE_TOOLCHAIN_FILE={toolchain_file}");
    } else {
        let root = root.display();
        println!(
            "  cmake -S {root} -B {root}/{build} -DCMAKE_TOOLCHAIN_FILE={root}/{toolchain_file}"
        );
    }
    if let Some(value) = ctx.env_compiler.as_deref().filter(|v| !v.is_empty()) {
        eprintln!(
            "note: {}={value} overrides this pin in this shell",
            project::ENV_VAR
        );
    }
    Ok(())
}

/// How a release gets into the store.
#[derive(Debug, Clone, Copy)]
enum How<'a> {
    /// Unpack the prebuilt archive.
    Prebuilt,
    /// Build the source tarball.
    Source(&'a source::Options),
}

/// The build options when the user agreed to a build without passing any.
static DEFAULT_BUILD: source::Options = source::Options {
    bootstrap: false,
    jobs: None,
    host_compiler: None,
    configure_args: Vec::new(),
};

/// The release to install for `spec` and how: the highest source release when the user asked
/// for a build with `build`; else the highest prebuilt one; else, if there is a source release
/// and the user agrees to wait, that one. `None` when the catalog has nothing for `spec`.
/// `command` is what builds it from source, for the hint when nobody can be asked.
fn pick_release<'a>(
    ctx: &Context,
    catalog: &'a Catalog,
    spec: &Spec,
    build: Option<&'a source::Options>,
    command: &str,
) -> Result<Option<(&'a Release, How<'a>)>> {
    if let Some(options) = build {
        if let Spec::Toolchain {
            kind: Kind::Clang, ..
        } = spec
        {
            return Err(Error::Usage {
                message: format!("cxvm builds only GCC from source, not {spec}"),
                hint: "install a prebuilt Clang: leave `--from-source` out".into(),
            });
        }
        let release = catalog.resolve_source(spec);
        return Ok(release.map(|release| (release, How::Source(options))));
    }
    if let Some(release) = catalog.resolve(spec) {
        return Ok(Some((release, How::Prebuilt)));
    }
    let Some(release) = catalog.resolve_source(spec) else {
        return Ok(None);
    };
    let id = &release.id;
    let jobs = std::thread::available_parallelism().map_or(1, usize::from);
    let minutes = source::estimate_minutes(jobs, false);
    if ctx.interactive {
        eprint!(
            "{id} has no prebuilt archive. Build it from source (about {minutes} min here)? [y/N] "
        );
        let mut answer = String::new();
        let _ = std::io::stdin().read_line(&mut answer);
        if is_yes(&answer) {
            return Ok(Some((release, How::Source(&DEFAULT_BUILD))));
        }
    }
    Err(Error::Usage {
        message: format!("{id} has no prebuilt archive; cxvm can build it from source"),
        hint: format!(
            "run `{command}` (about {minutes} min here); `cxvm install --help` lists the \
             build options"
        ),
    })
}

/// Whether `answer`, a line typed at a `[y/N]` prompt, means yes.
fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
}

/// `cxvm install` without an id: install the project's pinned toolchain if it is missing and
/// the catalog has it, then regenerate `.cxvm/`. With `build`, a missing toolchain is built
/// from source.
pub fn install_pinned(ctx: &Context, build: Option<&source::Options>) -> Result<()> {
    let Some(rc) = project::find_rc(&ctx.cwd) else {
        return Err(Error::Usage {
            message: format!(
                "no {} in {} or its parents",
                project::RC_FILE,
                ctx.cwd.display()
            ),
            hint: "pin a toolchain with `cxvm use <id>`, or install one with `cxvm install <id>`"
                .into(),
        });
    };
    let project = ProjectRc::load(&rc)?;
    let context = format!(" (set by {})", rc.display());
    let target = find_or_install(ctx, &project.compiler, context, build)?;
    if let (true, Some(gcc)) = (target.is_clang(), &project.clang_gcc) {
        find_or_install(ctx, gcc, clang_gcc_context(&rc), build)?;
    }
    let root = rc.parent().unwrap_or(&ctx.cwd).to_owned();
    generate_project(ctx, &root, &target, &project)
}

/// The toolchain `spec` selects, installed first if it is missing and the catalog has it.
/// `context` goes into the not-installed error; with `build`, a missing toolchain is built
/// from source.
fn find_or_install(
    ctx: &Context,
    spec: &Spec,
    context: String,
    build: Option<&source::Options>,
) -> Result<Target> {
    match Target::find(&ctx.store(), spec, context) {
        Ok(target) => {
            println!("{} is available", target.spec());
            Ok(target)
        }
        Err(err @ Error::NotInstalled { .. }) => {
            let installable = match spec {
                Spec::Toolchain { version, .. } => version.suffix().is_none(),
                Spec::System => false,
            };
            if !installable {
                return Err(err);
            }
            let catalog = ctx.catalog()?;
            let command = "cxvm install --from-source";
            match pick_release(ctx, &catalog, spec, build, command)? {
                Some((release, how)) => Ok(Target::Toolchain(install_release(
                    ctx, &catalog, release, how,
                )?)),
                None => Err(err),
            }
        }
        Err(err) => Err(err),
    }
}

/// `cxvm install <id>`: download a release from the catalog and install it in the store, or
/// with `build`, build it from source.
pub fn install(ctx: &Context, id: &str, build: Option<&source::Options>) -> Result<()> {
    let spec: Spec = id.parse()?;
    let Spec::Toolchain { kind, version } = &spec else {
        return Err(Error::Usage {
            message: "`system` is the compiler found on PATH; cxvm does not install it".into(),
            hint: "install a toolchain such as `cxvm install clang@19`, or pin the system \
                   compiler with `cxvm use system`"
                .into(),
        });
    };
    if let Some(suffix) = version.suffix() {
        return Err(Error::Usage {
            message: format!(
                "{spec} has a suffix (`-{suffix}`), which only linked toolchains have"
            ),
            hint: format!(
                "install `{kind}@<x.y.z>` without a suffix, or register an existing install \
                 with `cxvm link <prefix> --as {spec}`"
            ),
        });
    }
    let catalog = ctx.catalog()?;
    let command = format!("cxvm install {spec} --from-source");
    let Some((release, how)) = pick_release(ctx, &catalog, &spec, build, &command)? else {
        let same_major = |releases: &[Release]| -> Vec<String> {
            releases
                .iter()
                .filter(|r| r.id.kind() == *kind && r.id.version().major() == version.major())
                .map(|r| r.id.to_string())
                .collect()
        };
        let prebuilt = match build {
            Some(_) => Vec::new(),
            None => same_major(catalog.releases()),
        };
        let from_source: Vec<String> = same_major(catalog.source_releases())
            .into_iter()
            .filter(|id| !prebuilt.contains(id))
            .collect();
        let mut offers = Vec::new();
        if !prebuilt.is_empty() {
            offers.push(format!("available: {}", prebuilt.join(", ")));
        }
        if !from_source.is_empty() {
            offers.push(format!("with `--from-source`: {}", from_source.join(", ")));
        }
        let hint = if offers.is_empty() {
            "`cxvm list --available` shows what can be installed; register other builds with \
             `cxvm link <prefix>`"
                .to_owned()
        } else {
            offers.join("; ")
        };
        return Err(Error::Usage {
            message: match build {
                Some(_) => format!("{spec} is not a source release in {}", catalog.origin()),
                None => format!("{spec} is not in {}", catalog.origin()),
            },
            hint,
        });
    };
    if !spec.is_exact() {
        println!("Resolved {spec} to {}", release.id);
    }
    let toolchain = install_release(ctx, &catalog, release, how)?;
    println!("  pin it with `cxvm use {}`", toolchain.id);
    Ok(())
}

/// The compiler to build GCC with: the GCC `options` names, else the system's (`None` if
/// there is none).
fn host_compiler(
    ctx: &Context,
    store: &Store,
    options: &source::Options,
) -> Result<Option<HostCompiler>> {
    let system = || HostCompiler::system(ctx.path_var.as_deref(), ctx.paths.data_dir());
    let Some(input) = &options.host_compiler else {
        return Ok(system());
    };
    let spec: Spec = input.parse()?;
    if let Spec::Toolchain {
        kind: Kind::Clang, ..
    } = spec
    {
        return Err(Error::Usage {
            message: format!("`--host-compiler {spec}`: cxvm builds GCC with GCC, not with Clang"),
            hint: "pass a GCC that `cxvm list` shows, or leave the option out to build with \
                   the system compiler"
                .into(),
        });
    }
    match Target::find(store, &spec, " (--host-compiler)".into())? {
        Target::System => Ok(system()),
        Target::Toolchain(toolchain) => Ok(Some(HostCompiler {
            label: toolchain.id.to_string(),
            cc: toolchain.cc,
            cxx: toolchain.cxx,
        })),
    }
}

/// Installs `release` unless the store has it already, and returns it.
fn install_release(
    ctx: &Context,
    catalog: &Catalog,
    release: &Release,
    how: How<'_>,
) -> Result<Installed> {
    let store = ctx.store();
    let present = |store: &Store| -> Result<Option<Installed>> {
        Ok(store.installed()?.into_iter().find(|t| t.id == release.id))
    };
    let already = |toolchain: Installed| {
        println!(
            "{} is already installed in {}",
            toolchain.id,
            toolchain.path.display()
        );
        if matches!(how, How::Source(_)) {
            println!(
                "  to build it again, run `cxvm remove {}` first",
                toolchain.id
            );
        }
        Ok(toolchain)
    };
    if let Some(toolchain) = present(&store)? {
        return already(toolchain);
    }
    let lock = store.lock()?;
    // Another cxvm may have installed it while this one waited for the lock.
    if let Some(toolchain) = present(&store)? {
        return already(toolchain);
    }
    let (report, log) = match how {
        How::Prebuilt => {
            println!(
                "Installing {} ({} download)",
                release.id,
                human_size(release.size)
            );
            let backend = backends::backend_for(release.id.kind());
            (backends::install(&store, &lock, release, backend)?, None)
        }
        How::Source(options) => {
            let host = host_compiler(ctx, &store, options)?;
            let path_var = ctx.path_var.as_deref();
            let builder = GnuSource::new(&store, catalog, release, host, path_var, options)?;
            println!(
                "Building {} from source ({} download)\n{}",
                release.id,
                human_size(release.size),
                builder.summary()
            );
            let report = backends::install(&store, &lock, release, &builder)?;
            (report, Some(builder.log().to_owned()))
        }
    };
    drop(lock);
    let toolchain = report.toolchain;
    if let Some(problem) = report.warning {
        let hint = match (toolchain.id.kind(), how) {
            (Kind::Clang, _) => {
                "clang builds C++ with the newest GCC installation it finds on the host. \
                 Either there is none (install g++ or your distribution's libstdc++ development \
                 package), or its libstdc++ is too new for this clang: pass an older one with \
                 `--gcc-install-dir=/usr/lib/gcc/x86_64-linux-gnu/<version>` (clang 16 and \
                 later), or let cxvm do it in a project: `cxvm use <clang id> --gcc <gcc id>` \
                 pairs this clang with a GCC from `cxvm list`"
            }
            (Kind::Gcc, How::Prebuilt) => {
                "this GCC builds against the host's C library. Either its headers are missing \
                 (install `libc6-dev` on Debian/Ubuntu, `glibc-devel` on Fedora/RHEL), or the \
                 linker bundled with this build is too old for the host's libraries (its \
                 errors mention `.relr.dyn`): pass `-B/usr/bin` to use the host's linker, or \
                 install a newer release of the same major (`cxvm list --available`). See \
                 docs/backends.md"
            }
            (Kind::Gcc, How::Source(_)) => {
                "a GCC built from source uses the host's assembler, linker and C library: \
                 check that `as` and `ld` (binutils) are on PATH when it runs. See \
                 docs/backends.md"
            }
        };
        eprintln!(
            "warning: {} is installed, but cannot build and run a C++ program here: {problem}\n  \
             hint: {hint}",
            toolchain.id
        );
    }
    println!("Installed {} in {}", toolchain.id, toolchain.path.display());
    match log {
        None => println!(
            "  the downloaded archive stays in {}; delete it to free space",
            report.archive.display()
        ),
        Some(log) => println!(
            "  the build log is {}; the downloaded sources stay in {}",
            log.display(),
            ctx.paths.downloads_dir().display()
        ),
    }
    Ok(toolchain)
}

/// `cxvm remove`: delete a toolchain cxvm installed. Refuses the global default unless
/// `force`, and warns when the current directory still pins it.
pub fn remove(ctx: &Context, id: &str, force: bool) -> Result<()> {
    let id: ToolchainId = id.parse()?;
    let store = ctx.store();
    let lock = store.lock()?;
    let installed = store.installed()?;
    let Some(toolchain) = installed.iter().find(|t| t.id == id) else {
        let same_kind: Vec<String> = installed
            .iter()
            .filter(|t| t.id.kind() == id.kind())
            .map(|t| t.id.to_string())
            .collect();
        return Err(Error::Usage {
            message: format!("{id} is not installed"),
            hint: if same_kind.is_empty() {
                "`cxvm list` shows the installed toolchains".into()
            } else {
                format!("installed: {}", same_kind.join(", "))
            },
        });
    };
    if toolchain.linked {
        return Err(Error::Usage {
            message: format!(
                "{id} is linked to {}, not installed by cxvm",
                toolchain.path.display()
            ),
            hint: format!("unregister it with `cxvm unlink {id}`; its files are left alone"),
        });
    }

    // Whether `spec` uses `id` now and nothing once it is gone.
    let others: Vec<&ToolchainId> = installed
        .iter()
        .map(|t| &t.id)
        .filter(|t| **t != id)
        .collect();
    let loses = |spec: &Spec| {
        spec.select(installed.iter().map(|t| &t.id)) == Some(&id)
            && spec.select(others.iter().copied()).is_none()
    };
    let config_file = ctx.paths.config_file();
    let global = Config::load(&config_file)?
        .compiler
        .filter(|spec| loses(spec));
    if let Some(spec) = &global
        && !force
    {
        return Err(Error::Usage {
            message: format!(
                "{id} is the global default ({spec} in {})",
                config_file.display()
            ),
            hint: format!(
                "set another default with `cxvm global <id>` first, or run \
                 `cxvm remove {id} --force`"
            ),
        });
    }

    store.remove(&lock, &id)?;
    drop(lock);
    println!("Removed {id} from {}", toolchain.path.display());
    if global.is_some() {
        eprintln!("warning: {id} was the global default; set another with `cxvm global <id>`");
    }
    if let Ok(Resolution { spec, source }) = ctx.resolve()
        && !matches!(source, Source::Global(_))
        && loses(&spec)
    {
        eprintln!(
            "warning: {id} is still pinned here ({source}); reinstall it with `cxvm install`, \
             or pin another with `cxvm use <id>`"
        );
    }
    Ok(())
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

    let _lock = store.lock()?;
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
    let _lock = store.lock()?;
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

/// `cxvm global`: set the default toolchain used outside projects.
pub fn global(ctx: &Context, id: &str) -> Result<()> {
    let spec: Spec = id.parse()?;
    let target = Target::find(&ctx.store(), &spec, String::new())?;
    let exact = target.spec();
    let config_file = ctx.paths.config_file();
    Config {
        compiler: Some(exact.clone()),
    }
    .save(&config_file)?;
    if exact == spec {
        println!("Global default set to {exact} in {}", config_file.display());
    } else {
        println!(
            "Global default set to {exact} (from {spec}) in {}",
            config_file.display()
        );
    }
    if let Ok(Resolution { source, .. }) = ctx.resolve()
        && matches!(source, Source::Env | Source::ProjectRc(_))
    {
        eprintln!("note: here the toolchain is {source}, which overrides the global default");
    }
    Ok(())
}

/// The activation of the toolchain that is active here; fails if it is not installed.
fn active(ctx: &Context) -> Result<Activation> {
    let Resolution { spec, source } = ctx.resolve()?;
    let store = ctx.store();
    let target = Target::find(&store, &spec, format!(" ({source})"))?;
    let gcc = project_paired_gcc(ctx, &store, &target)?;
    target.activation(ctx, gcc.as_ref())
}

/// `cxvm env`: print exports for `eval`. `shell` defaults to the one in `$SHELL`.
pub fn env(ctx: &Context, shell: Option<Shell>) -> Result<()> {
    let shell = shell.unwrap_or_else(|| {
        let login = ctx
            .shell
            .as_deref()
            .map(Path::new)
            .and_then(Path::file_name);
        if login.is_some_and(|name| name == "fish") {
            Shell::Fish
        } else {
            Shell::Posix
        }
    });
    let activation = active(ctx)?;
    print!("{}", integrations::render_env(&activation, shell));
    Ok(())
}

/// `cxvm exec`: replace this process with `cmd`, run with the active toolchain's environment.
/// Only returns on failure.
pub fn exec(ctx: &Context, cmd: &[String]) -> Result<()> {
    use std::os::unix::process::CommandExt;

    let (program, args) = cmd.split_first().expect("clap requires a command");
    let activation = active(ctx)?;
    let mut command = std::process::Command::new(program);
    command.args(args).env("CXVM_TOOLCHAIN", &activation.label);
    if let Some(bin) = &activation.bin {
        let dirs =
            std::iter::once(bin.clone()).chain(ctx.path_var.iter().flat_map(std::env::split_paths));
        let path = std::env::join_paths(dirs).map_err(|err| Error::Usage {
            message: format!("cannot add {} to PATH: {err}", bin.display()),
            hint: "toolchain paths must not contain `:`".into(),
        })?;
        command.env("PATH", path);
    }
    for (name, value) in activation.compiler_env() {
        command.env(name, value);
    }
    let err = files::retry_busy(|| Err::<(), _>(command.exec())).unwrap_err();
    Err(Error::Usage {
        message: format!("cannot run `{program}`: {err}"),
        hint: "check the command name; the toolchain's bin directory is searched first".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(id: &str, size: u64) -> Release {
        Release {
            id: id.parse().unwrap(),
            asset: "a.tar.xz".into(),
            url: "https://example.invalid/a.tar.xz".into(),
            size,
            sha256: "0".repeat(64),
            source: "test".into(),
        }
    }

    const MIB: u64 = 1 << 20;

    #[test]
    fn available_is_a_table_per_kind_with_a_row_per_major() {
        let releases = [
            release("clang@18.1.8", 997 * MIB),
            release("clang@19.1.0", 1536 * MIB),
            release("clang@19.1.7", 1638 * MIB),
            release("gcc@8.5.0", 111 * MIB),
            release("gcc@13.3.0", 140 * MIB),
            release("gcc@13.4.0", 139 * MIB),
        ];
        let installed: ToolchainId = "gcc@13.3.0".parse().unwrap();
        let text = render_available(&releases, &[], |id| *id == installed, "the catalog");
        assert_eq!(
            text,
            "Installable toolchains (* = installed), from the catalog:\n\
             \n\
             \x20 gcc    download  versions\n\
             \x20 13      139 MiB  13.4.0  13.3.0*\n\
             \x20 8       111 MiB  8.5.0\n\
             \n\
             \x20 clang  download  versions\n\
             \x20 19      1.6 GiB  19.1.7  19.1.0\n\
             \x20 18      997 MiB  18.1.8\n"
        );
    }

    #[test]
    fn available_wraps_long_rows_under_the_versions_column() {
        let releases: Vec<Release> = (0..=8)
            .map(|patch| release(&format!("clang@22.1.{patch}"), 1900 * MIB))
            .collect();
        let text = render_available(&releases, &[], |_| false, "the catalog");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[2..5],
            [
                "  clang  download  versions",
                "  22      1.9 GiB  22.1.8  22.1.7  22.1.6  22.1.5  22.1.4  22.1.3  22.1.2",
                "                   22.1.1  22.1.0",
            ]
        );
        assert!(lines.iter().all(|line| line.len() <= AVAILABLE_WIDTH));
        assert!(lines.iter().all(|line| *line == line.trim_end()));
    }

    #[test]
    fn available_without_releases_is_the_heading_only() {
        assert_eq!(
            render_available(&[], &[], |_| false, "/c.toml (CXVM_CATALOG)"),
            "Installable toolchains (* = installed), from /c.toml (CXVM_CATALOG):\n"
        );
    }

    #[test]
    fn available_lists_source_only_releases_under_their_own_title() {
        let releases = [
            release("clang@19.1.7", 1638 * MIB),
            release("gcc@13.4.0", 139 * MIB),
        ];
        let sources = [
            release("gcc@10.4.0", 72 * MIB),
            release("gcc@10.5.0", 75 * MIB),
            release("gcc@16.2.0", 103 * MIB),
        ];
        let source_only: Vec<&Release> = sources.iter().collect();
        let installed: ToolchainId = "gcc@10.5.0".parse().unwrap();
        let text = render_available(
            &releases,
            &source_only,
            |id| *id == installed,
            "the catalog",
        );
        assert_eq!(
            text,
            "Installable toolchains (* = installed), from the catalog:\n\
             \n\
             \x20 gcc    download  versions\n\
             \x20 13      139 MiB  13.4.0\n\
             \n\
             \x20 clang  download  versions\n\
             \x20 19      1.6 GiB  19.1.7\n\
             \n\
             No prebuilt archive, built with `cxvm install <id> --from-source`:\n\
             \n\
             \x20 gcc    download  versions\n\
             \x20 16      103 MiB  16.2.0\n\
             \x20 10       75 MiB  10.5.0* 10.4.0\n"
        );
        let text = render_available(&[], &source_only, |_| false, "the catalog");
        assert!(text.contains(":\n\nNo prebuilt archive"), "{text}");
    }

    #[test]
    fn only_a_clear_yes_starts_a_build() {
        for answer in ["y\n", "Y\n", " yes \n", "YES"] {
            assert!(is_yes(answer), "{answer:?}");
        }
        for answer in ["", "\n", "n\n", "no\n", "yep\n", "j\n"] {
            assert!(!is_yes(answer), "{answer:?}");
        }
    }
}
