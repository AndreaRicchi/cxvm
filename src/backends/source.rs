//! Building GCC from a GNU source release (M5): `cxvm install <id> --from-source`.
//!
//! The verified tarball is unpacked in `staging/`, the GMP, MPFR, MPC and ISL tarballs that its
//! own `contrib/download_prerequisites` names are added to the tree, and GCC is configured for
//! its final store path, built out of tree and installed with `DESTDIR` into staging. All
//! output goes to a log file in the cache, which survives a failed build. See docs/backends.md.

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, BufRead, BufReader, IsTerminal, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use indicatif::{ProgressBar, ProgressStyle};

use super::{Backend, download, extract, smoke};
use crate::catalog::{Catalog, Release};
use crate::detect;
use crate::error::{Error, Result};
use crate::files;
use crate::output::{self, debug};
use crate::store::{self, Meta, Store};
use crate::toolchain;

/// The libraries built with GCC, by the name of their directory in its source tree.
const LIBRARIES: [&str; 4] = ["gmp", "mpfr", "mpc", "isl"];

/// The programs a build needs on `PATH`, besides the host compiler.
const TOOLS: [&str; 3] = ["make", "m4", "awk"];

const PACKAGES_HINT: &str = "install a C and C++ compiler, make and m4: \
     `sudo apt install build-essential m4` (Debian/Ubuntu), \
     `sudo dnf install gcc gcc-c++ make m4` (Fedora/RHEL), \
     `sudo pacman -S base-devel` (Arch); or build with a GCC cxvm manages: \
     `--host-compiler <id>`";

/// The compiler GCC is built with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCompiler {
    /// `system`, or the id of a toolchain cxvm manages.
    pub label: String,
    pub cc: PathBuf,
    pub cxx: PathBuf,
}

impl HostCompiler {
    /// The `cc` and `c++` on `path_var`, outside cxvm's own directories.
    pub fn system(path_var: Option<&OsStr>, store: &Path) -> Option<Self> {
        Some(Self {
            label: "system".into(),
            cc: toolchain::find_system("cc", path_var, store)?,
            cxx: toolchain::find_system("c++", path_var, store)?,
        })
    }
}

/// What the user chose for a build.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// Full three-stage build instead of building once with the host compiler.
    pub bootstrap: bool,
    /// Parallel `make` jobs; default: one per CPU.
    pub jobs: Option<usize>,
    /// The id of a GCC cxvm manages to build with; default: the system compiler.
    pub host_compiler: Option<String>,
    /// Extra arguments for GCC's `configure`.
    pub configure_args: Vec<String>,
}

/// A rough guess of how long a build takes, in minutes: a part that does not get faster with
/// more jobs plus the compile work shared between them, as measured for GCC 16.2.0 (see
/// docs/backends.md; GCC 10 takes about half). A bootstrap compiles the compiler three times.
pub fn estimate_minutes(jobs: usize, bootstrap: bool) -> u64 {
    const SERIAL: f64 = 4.0;
    const PARALLEL: f64 = 70.0;
    let once = SERIAL + PARALLEL / jobs.max(1) as f64;
    let minutes = if bootstrap { once * 3.0 } else { once };
    minutes.ceil() as u64
}

/// Builds GNU source releases of GCC.
#[derive(Debug)]
pub struct GnuSource<'a> {
    catalog: &'a Catalog,
    downloads: PathBuf,
    /// Where the toolchain will live in the store: GCC's `--prefix`.
    prefix: PathBuf,
    host: HostCompiler,
    /// First line of the host compiler's `--version`.
    host_version: String,
    make: PathBuf,
    jobs: usize,
    bootstrap: bool,
    extra_args: Vec<String>,
    log: PathBuf,
    /// Where the build tree goes, for the user to know which disk fills up.
    staging: PathBuf,
    /// `PATH` for the build: the caller's, without cxvm's own directories.
    path: OsString,
}

impl<'a> GnuSource<'a> {
    /// Checks that `release` can be built here: `make`, `m4` and `awk` are on `path_var`, and
    /// `host` (`None` if the system has no compiler) builds C and C++ programs that run.
    /// Call it with the store locked: it compiles in `staging/`.
    pub fn new(
        store: &Store,
        catalog: &'a Catalog,
        release: &Release,
        host: Option<HostCompiler>,
        path_var: Option<&OsStr>,
        options: &Options,
    ) -> Result<Self> {
        let paths = store.paths();
        let id = &release.id;
        let prefix = paths.toolchain_dir(id);
        if prefix
            .to_str()
            .is_none_or(|p| p.contains(char::is_whitespace))
        {
            return Err(Error::Usage {
                message: format!(
                    "cannot build {id} for {}: GCC's build does not support spaces or \
                     non-UTF-8 characters in its prefix",
                    prefix.display()
                ),
                hint: "point CXVM_HOME at a directory whose path has neither".into(),
            });
        }

        let dirs = toolchain::unmanaged_path(path_var, paths.data_dir());
        let path = std::env::join_paths(&dirs).unwrap_or_default();
        let find = |name: &str| {
            dirs.iter()
                .map(|dir| dir.join(name))
                .find(|candidate| files::is_executable(candidate))
        };
        let mut problems: Vec<String> = TOOLS
            .iter()
            .filter(|tool| find(tool).is_none())
            .map(|tool| format!("there is no `{tool}` on PATH"))
            .collect();
        match &host {
            None => problems.push("there is no C and C++ compiler (`cc`, `c++`) on PATH".into()),
            Some(host) => {
                let scratch = store.staging_path("preflight");
                let programs = [
                    (
                        &host.cc,
                        "hello.c",
                        "#include <stdio.h>\nint main(void) { puts(\"cxvm-ok\"); return 0; }\n",
                    ),
                    (
                        &host.cxx,
                        "hello.cpp",
                        "#include <iostream>\nint main() { std::cout << \"cxvm-ok\" << std::endl; }\n",
                    ),
                ];
                for (compiler, file, text) in programs {
                    if let Err(problem) = builds_and_runs(compiler, &path, &scratch, file, text) {
                        problems.push(problem);
                    }
                }
                let _ = fs::remove_dir_all(&scratch);
            }
        }
        let (Some(host), Some(make), true) = (host, find("make"), problems.is_empty()) else {
            return Err(Error::Usage {
                message: format!(
                    "cannot build {id} from source here: {}",
                    problems.join("; ")
                ),
                hint: PACKAGES_HINT.into(),
            });
        };

        let stamp: String = store::utc_timestamp(SystemTime::now())
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        Ok(Self {
            catalog,
            downloads: paths.downloads_dir(),
            prefix,
            host_version: detect::version_line(&host.cc).unwrap_or_default(),
            host,
            make,
            jobs: options
                .jobs
                .or_else(|| std::thread::available_parallelism().ok().map(usize::from))
                .unwrap_or(1)
                .max(1),
            bootstrap: options.bootstrap,
            extra_args: options.configure_args.clone(),
            log: paths
                .logs_dir()
                .join(format!("{}-{}-{stamp}.log", id.kind(), id.version())),
            staging: paths.staging_dir(),
            path,
        })
    }

    /// Where the build's output goes.
    pub fn log(&self) -> &Path {
        &self.log
    }

    /// What is about to happen, for the user to read before a long wait.
    pub fn summary(&self) -> String {
        let minutes = estimate_minutes(self.jobs, self.bootstrap);
        format!(
            "  host compiler: {} ({})\n  \
               configure:     {}\n  \
               jobs:          {}\n  \
               log:           {}\n\
             This takes about {minutes} minute{} here, and up to 7 GB in {} while it runs.\n\
             Ctrl-C cancels it and leaves nothing installed.",
            self.host.label,
            self.host_version,
            self.configure_args().join(" "),
            self.jobs,
            self.log.display(),
            if minutes == 1 { "" } else { "s" },
            self.staging.display(),
        )
    }

    /// The arguments GCC's `configure` is run with.
    fn configure_args(&self) -> Vec<String> {
        let mut args = vec![
            format!("--prefix={}", self.prefix.display()),
            "--enable-languages=c,c++".to_owned(),
            "--disable-multilib".to_owned(),
        ];
        if !self.bootstrap {
            args.push("--disable-bootstrap".to_owned());
        }
        args.extend(self.extra_args.iter().cloned());
        args
    }

    /// `program` run in `dir` with the build's environment: nothing of the caller's but
    /// `HOME` and `TMPDIR`, the filtered `PATH`, and the host compiler as `CC` and `CXX`.
    fn command(&self, program: &Path, dir: &Path) -> Command {
        let mut command = base_command(program, &self.path);
        command
            .current_dir(dir)
            .env("CC", &self.host.cc)
            .env("CXX", &self.host.cxx);
        command
    }

    /// Adds the libraries the tree's `contrib/download_prerequisites` names to `src`, as
    /// `src/gmp`, `src/mpfr`, `src/mpc` and `src/isl`, where GCC's build picks them up.
    fn add_prerequisites(&self, release: &Release, src: &Path) -> Result<()> {
        let fail = |reason: String, hint: &str| Error::Install {
            id: release.id.to_string(),
            reason,
            hint: hint.into(),
        };
        const REPORT: &str = "report it at the cxvm issue tracker: cxvm cannot build this release";
        let script = src.join("contrib/download_prerequisites");
        let text = fs::read_to_string(&script).map_err(|e| {
            fail(
                format!("cannot read {}: {e}", script.display()),
                "the archive may be damaged: delete it from the download cache and run the \
                 same command again",
            )
        })?;
        let wanted = prerequisites(&text);
        for library in ["gmp", "mpfr", "mpc"] {
            if !wanted.iter().any(|(name, _)| *name == library) {
                return Err(fail(
                    format!("its contrib/download_prerequisites names no {library} release"),
                    REPORT,
                ));
            }
        }
        for (library, asset) in wanted {
            let Some(prerequisite) = self.catalog.prerequisite(&asset) else {
                if library == "isl" {
                    output::note(format!(
                        "building without ISL (no Graphite loop optimizations): {asset} is not \
                         in {}",
                        self.catalog.origin()
                    ));
                    continue;
                }
                return Err(fail(
                    format!(
                        "its sources need {asset}, which is not in {}",
                        self.catalog.origin()
                    ),
                    REPORT,
                ));
            };
            let archive = download::fetch(prerequisite.download(), &self.downloads)?;
            extract::unpack_archive(&archive, &src.join(library)).map_err(|reason| {
                fail(
                    reason,
                    "the archive may be damaged: delete it from the download cache and run the \
                     same command again",
                )
            })?;
        }
        Ok(())
    }

    /// Runs one step of the build, with its output appended to the log.
    fn run(
        &self,
        release: &Release,
        phase: &'static str,
        mut command: Command,
        log: &mut fs::File,
        interrupt: &Interrupt,
    ) -> Result<()> {
        let id = &release.id;
        let io_error = |source| Error::Io {
            action: "write",
            path: self.log.clone(),
            source,
            hint: "check that the cache directory is writable and the disk is not full",
        };
        writeln!(log, "\n==> {phase}: {command:?}").map_err(io_error)?;

        let (reader, writer) = io::pipe().map_err(io_error)?;
        command
            .stdin(Stdio::null())
            .stdout(writer.try_clone().map_err(io_error)?)
            .stderr(writer)
            // Its own process group, so that the whole tree of makes and compilers can be
            // stopped at once, and a Ctrl-C in the terminal reaches only cxvm.
            .process_group(0);
        debug!(
            "running {} in {}",
            output::command_line(&command),
            command
                .get_current_dir()
                .unwrap_or(Path::new("."))
                .display()
        );
        let started = Instant::now();
        let mut child = files::retry_busy(|| command.spawn()).map_err(|e| Error::Build {
            id: id.to_string(),
            failure: format!("`{phase}` could not start: {e}"),
            log: self.log.clone(),
            detail: String::new(),
            hint: PACKAGES_HINT.into(),
        })?;
        // The command holds the write ends of the pipe: without this, reading never ends.
        drop(command);

        let progress = Progress::start(phase);
        let mut output = Output::default();
        let group = child.id();
        let finished = AtomicBool::new(false);
        let copied = std::thread::scope(|scope| {
            scope.spawn(|| {
                while !finished.load(Ordering::SeqCst) {
                    if interrupt.requested() {
                        signal_group(group, libc::SIGTERM);
                        let deadline = Instant::now() + Duration::from_secs(5);
                        while !finished.load(Ordering::SeqCst) && Instant::now() < deadline {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        if !finished.load(Ordering::SeqCst) {
                            signal_group(group, libc::SIGKILL);
                        }
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            });
            let mut reader = BufReader::new(reader);
            let mut line = Vec::new();
            let result = loop {
                line.clear();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) => break Ok(()),
                    Ok(_) => {}
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                    Err(err) => break Err(err),
                }
                if let Err(err) = log.write_all(&line) {
                    break Err(err);
                }
                let text = String::from_utf8_lossy(&line);
                let text = text.trim_end();
                progress.show(text);
                output.push(text);
            };
            if result.is_err() {
                // Nothing reads the pipe any more: stop the build rather than let it block.
                signal_group(group, libc::SIGKILL);
            }
            finished.store(true, Ordering::SeqCst);
            result
        });
        let status = child.wait();
        progress.finish();
        copied.map_err(io_error)?;
        let status = status.map_err(io_error)?;

        if interrupt.requested() {
            let _ = writeln!(log, "\n==> interrupted");
            return Err(Error::Cancelled {
                what: format!("the build of {id}"),
            });
        }
        if !status.success() {
            return Err(Error::Build {
                id: id.to_string(),
                failure: format!("`{phase}` {}", outcome(status)),
                log: self.log.clone(),
                detail: output.detail(),
                hint: self.failure_hint(phase, &output),
            });
        }
        eprintln!("  {phase}: {}", human_duration(started.elapsed()));
        Ok(())
    }

    /// What to try after `phase` failed with `output`.
    fn failure_hint(&self, phase: &str, output: &Output) -> String {
        if output.libsanitizer {
            return "the sanitizer runtime (libsanitizer) of this release does not build against \
                    this system's C library, which is common for releases older than the \
                    system. Run the same command with \
                    `--configure-arg=--disable-libsanitizer`: the compiler then works, without \
                    `-fsanitize=`"
                .to_owned();
        }
        let host = match self.host.label.as_str() {
            "system" => "an older or newer GCC than the system's: `--host-compiler <id>` \
                         takes any GCC that `cxvm list` shows"
                .to_owned(),
            label => format!("another GCC than {label} as `--host-compiler <id>`"),
        };
        match phase {
            "configure" => format!(
                "the log and the `config.log` it names say what `configure` missed. If the \
                 host compiler is the problem, try {host}"
            ),
            _ => format!(
                "search the log for the first `error:`. Old releases often do not build with \
                 a much newer compiler: try {host}. A part that does not build against this \
                 system's C library can be left out with `--configure-arg=--disable-<part>`"
            ),
        }
    }
}

impl Backend for GnuSource<'_> {
    fn name(&self) -> &'static str {
        "gnu-source"
    }

    fn prepare(&self, release: &Release, archive: &Path, work: &Path) -> Result<PathBuf> {
        let id = &release.id;
        let src = work.join("src");
        extract::unpack_archive(archive, &src).map_err(|reason| Error::Install {
            id: id.to_string(),
            reason,
            hint: format!(
                "the archive may be damaged: delete {} and run the same command again",
                archive.display()
            ),
        })?;
        self.add_prerequisites(release, &src)?;

        let io_error = |action, path: &Path| {
            let path = path.to_owned();
            move |source| Error::Io {
                action,
                path,
                source,
                hint: "check that the cxvm store and cache are writable and the disk is not full",
            }
        };
        let build = work.join("build");
        fs::create_dir_all(&build).map_err(io_error("create", &build))?;
        let logs = self.log.parent().expect("the log is in the logs directory");
        fs::create_dir_all(logs).map_err(io_error("create", logs))?;
        let mut log = fs::File::create(&self.log).map_err(io_error("write", &self.log))?;
        writeln!(
            log,
            "cxvm {}: building {id} from {}\nhost compiler: {} ({}), {}\nPATH: {}",
            env!("CARGO_PKG_VERSION"),
            release.url,
            self.host.label,
            self.host.cc.display(),
            self.host_version,
            self.path.to_string_lossy(),
        )
        .map_err(io_error("write", &self.log))?;

        let dest = work.join("dest");
        let interrupt = Interrupt::watch().map_err(io_error("write", &self.log))?;
        let mut configure = self.command(&src.join("configure"), &build);
        configure.args(self.configure_args());
        self.run(release, "configure", configure, &mut log, &interrupt)?;

        let mut make = self.command(&self.make, &build);
        make.arg(format!("-j{}", self.jobs));
        self.run(release, "make", make, &mut log, &interrupt)?;

        let mut install = self.command(&self.make, &build);
        let mut destdir = OsString::from("DESTDIR=");
        destdir.push(&dest);
        install.arg("install-strip").arg(destdir);
        self.run(release, "make install", install, &mut log, &interrupt)?;
        drop(interrupt);

        // `DESTDIR` holds the whole path of the prefix: `dest/home/me/.local/…/gcc/10.5.0`.
        let staged = dest.join(self.prefix.strip_prefix("/").unwrap_or(&self.prefix));
        if !staged.join("bin").is_dir() {
            return Err(Error::Build {
                id: id.to_string(),
                failure: format!("`make install` left no {}", staged.join("bin").display()),
                log: self.log.clone(),
                detail: String::new(),
                hint: "report it at the cxvm issue tracker, with the log".into(),
            });
        }
        Ok(staged)
    }

    fn describe(&self, meta: &mut Meta) {
        meta.host_compiler = Some(self.host_version.clone());
        meta.configure_args = self.configure_args();
    }
}

/// `program` with a clean environment: `PATH` is `path`, `HOME` and `TMPDIR` are the
/// caller's, and messages are in English for the log. Flags such as `CFLAGS` or
/// `LIBRARY_PATH` from the caller's shell never reach the build.
fn base_command(program: &Path, path: &OsStr) -> Command {
    let mut command = Command::new(program);
    command.env_clear().env("PATH", path).env("LC_ALL", "C");
    for name in ["HOME", "TMPDIR"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
}

/// Builds `text` as `file` with `compiler` in `scratch` and runs the program. The error is a
/// reason for the user.
fn builds_and_runs(
    compiler: &Path,
    path: &OsStr,
    scratch: &Path,
    file: &str,
    text: &str,
) -> Result<(), String> {
    let name = compiler.display();
    fs::create_dir_all(scratch).map_err(|e| format!("cannot create {}: {e}", scratch.display()))?;
    let source = scratch.join(file);
    let program = scratch.join("hello");
    fs::write(&source, text).map_err(|e| format!("cannot write {}: {e}", source.display()))?;
    let mut compile = base_command(compiler, path);
    compile.arg(&source).arg("-o").arg(&program);
    let output =
        files::run_captured(&mut compile).map_err(|e| format!("{name} cannot be run: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "{name} cannot build a program from {file}:\n{}",
            smoke::tail(&output.stderr)
        ));
    }
    let run = files::run_captured(&mut base_command(&program, path));
    match run {
        Ok(run) if run.status.success() && run.stdout.starts_with(b"cxvm-ok") => Ok(()),
        Ok(run) => Err(format!(
            "the program {name} builds from {file} does not run:\n{}",
            smoke::tail(&run.stderr)
        )),
        Err(e) => Err(format!(
            "the program {name} builds from {file} does not run: {e}"
        )),
    }
}

/// The tarballs GCC's `contrib/download_prerequisites` would fetch, as (library, file name).
///
/// GCC 7 and later name them in full, `gmp='gmp-6.1.0.tar.bz2'`; earlier releases set
/// `GMP=gmp-4.3.2` and fetch `$GMP.tar.bz2`.
pub(crate) fn prerequisites(script: &str) -> Vec<(&'static str, String)> {
    let lines = || script.lines().map(str::trim);
    LIBRARIES
        .into_iter()
        .filter_map(|library| {
            let version = format!("{library}-");
            let new = lines().find_map(|line| {
                let value = line.strip_prefix(library)?.strip_prefix('=')?;
                Some(value.trim_matches(['\'', '"']).to_owned())
            });
            let old = || {
                let variable = library.to_uppercase();
                let value = lines().find_map(|line| {
                    let value = line.strip_prefix(variable.as_str())?.strip_prefix('=')?;
                    Some(value.trim_matches(['\'', '"']))
                })?;
                let fetched = format!("${variable}.tar.");
                let rest = &script[script.find(&fetched)? + fetched.len()..];
                let ext: String = rest
                    .chars()
                    .take_while(char::is_ascii_alphanumeric)
                    .collect();
                Some(format!("{value}.tar.{ext}"))
            };
            let asset = new.or_else(old)?;
            (asset.starts_with(&version) && files::is_plain_file_name(&asset))
                .then_some((library, asset))
        })
        .collect()
}

/// What a step of the build printed that is worth quoting when it fails.
#[derive(Debug, Default)]
struct Output {
    first_error: Option<String>,
    last: VecDeque<String>,
    /// A `make` error in the sanitizer runtime's directory.
    libsanitizer: bool,
}

impl Output {
    const LAST: usize = 12;

    fn push(&mut self, line: &str) {
        if self.first_error.is_none()
            && (line.contains(" error: ") || line.starts_with("configure: error:"))
        {
            self.first_error = Some(line.to_owned());
        }
        if line.contains("***") && line.contains("libsanitizer") {
            self.libsanitizer = true;
        }
        if self.last.len() == Self::LAST {
            self.last.pop_front();
        }
        self.last.push_back(line.to_owned());
    }

    /// The first error and the last lines, indented.
    fn detail(&self) -> String {
        let mut detail = String::new();
        if let Some(error) = &self.first_error
            && !self.last.contains(error)
        {
            detail.push_str(&format!("  first error:\n    {error}\n"));
        }
        if !self.last.is_empty() {
            detail.push_str("  last lines:\n");
            for line in &self.last {
                detail.push_str(&format!("    {line}\n"));
            }
        }
        detail
    }
}

/// How a failed step ended, e.g. `exited with status 2`.
fn outcome(status: ExitStatus) -> String {
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("exited with status {code}"),
        (None, Some(signal)) => format!("was killed by signal {signal}"),
        (None, None) => "failed".into(),
    }
}

/// `9m 03s`, `14s`.
fn human_duration(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs >= 60 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

/// On a terminal, a spinner with the step, the time it has taken and the line it just
/// printed; otherwise one line saying that the step started.
struct Progress(ProgressBar);

impl Progress {
    fn start(phase: &str) -> Self {
        if !io::stderr().is_terminal() {
            eprintln!("Running {phase}");
            return Self(ProgressBar::hidden());
        }
        let bar = ProgressBar::new_spinner().with_style(
            ProgressStyle::with_template("{spinner} {prefix} [{elapsed}] {wide_msg}")
                .expect("valid template"),
        );
        bar.set_prefix(phase.to_owned());
        bar.enable_steady_tick(Duration::from_millis(120));
        Self(bar)
    }

    fn show(&self, line: &str) {
        if !self.0.is_hidden() {
            let line: String = line
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            self.0.set_message(line);
        }
    }

    fn finish(&self) {
        self.0.finish_and_clear();
    }
}

/// Catches SIGINT, SIGTERM and SIGHUP while a build runs, so that cxvm can stop the build and
/// delete its files instead of dying with gigabytes left in `staging/`. A second signal, or
/// one after the build, ends the process as usual.
struct Interrupt {
    requested: Arc<AtomicBool>,
    /// Once set, the signals have their default effect again.
    passive: Arc<AtomicBool>,
}

impl Interrupt {
    fn watch() -> io::Result<Self> {
        use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
        use signal_hook::flag;

        let interrupt = Self {
            requested: Arc::default(),
            passive: Arc::default(),
        };
        for signal in [SIGINT, SIGTERM, SIGHUP] {
            // In this order: the first signal only asks to stop, the next one terminates.
            flag::register_conditional_default(signal, Arc::clone(&interrupt.passive))?;
            flag::register(signal, Arc::clone(&interrupt.requested))?;
            flag::register(signal, Arc::clone(&interrupt.passive))?;
        }
        Ok(interrupt)
    }

    fn requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }
}

impl Drop for Interrupt {
    fn drop(&mut self) {
        // signal-hook cannot uninstall its handler; make it act like the default one.
        self.passive.store(true, Ordering::SeqCst);
    }
}

/// Sends `signal` to every process of the group `group`.
fn signal_group(group: u32, signal: libc::c_int) {
    let Ok(group) = libc::pid_t::try_from(group) else {
        return;
    };
    // SAFETY: kill(2) only takes integers and touches no memory of this process. The group
    // is the one of a child that has not been waited for yet, so the id cannot have been
    // reused. Failure (the group is already gone) needs no handling.
    unsafe {
        libc::kill(-group, signal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_prerequisites_of_gcc_7_and_later() {
        let script = "#! /bin/sh\n\
                      gmp='gmp-6.1.0.tar.bz2'\n\
                      mpfr='mpfr-3.1.6.tar.bz2'\n\
                      mpc='mpc-1.0.3.tar.gz'\n\
                      isl='isl-0.18.tar.bz2'\n\
                      gettext='gettext-0.22.tar.gz'\n\
                      base_url='http://gcc.gnu.org/pub/gcc/infrastructure/'\n\
                      echo_archives() {\n    echo \"${gmp}\"\n    \
                      if [ ${graphite} -gt 0 ]; then echo \"${isl}\"; fi\n}\n\
                      graphite=1\n";
        assert_eq!(
            prerequisites(script),
            [
                ("gmp", "gmp-6.1.0.tar.bz2".to_owned()),
                ("mpfr", "mpfr-3.1.6.tar.bz2".to_owned()),
                ("mpc", "mpc-1.0.3.tar.gz".to_owned()),
                ("isl", "isl-0.18.tar.bz2".to_owned()),
            ]
        );
    }

    #[test]
    fn reads_the_prerequisites_of_gcc_6_and_earlier() {
        let script = "GRAPHITE_LOOP_OPT=yes\n\
                      MPFR=mpfr-2.4.2\nGMP=gmp-4.3.2\nMPC=mpc-0.8.1\n\
                      wget ftp://gcc.gnu.org/pub/gcc/infrastructure/$MPFR.tar.bz2 || exit 1\n\
                      tar xjf $MPFR.tar.bz2 || exit 1\n\
                      wget ftp://gcc.gnu.org/pub/gcc/infrastructure/$GMP.tar.bz2 || exit 1\n\
                      wget ftp://gcc.gnu.org/pub/gcc/infrastructure/$MPC.tar.gz || exit 1\n\
                      if [ \"$GRAPHITE_LOOP_OPT\" = \"yes\" ] ; then\n  ISL=isl-0.15\n  \
                      wget ftp://gcc.gnu.org/pub/gcc/infrastructure/$ISL.tar.bz2 || exit 1\n  \
                      CLOOG=cloog-0.18.1\nfi\n";
        assert_eq!(
            prerequisites(script),
            [
                ("gmp", "gmp-4.3.2.tar.bz2".to_owned()),
                ("mpfr", "mpfr-2.4.2.tar.bz2".to_owned()),
                ("mpc", "mpc-0.8.1.tar.gz".to_owned()),
                ("isl", "isl-0.15.tar.bz2".to_owned()),
            ]
        );
    }

    #[test]
    fn ignores_what_is_not_a_library_tarball() {
        assert_eq!(prerequisites(""), []);
        assert_eq!(prerequisites("gmp='../evil.tar.bz2'\nmpfr=\n"), []);
        assert_eq!(prerequisites("GMP=gmp-4.3.2\necho no download line\n"), []);
    }

    #[test]
    fn estimates_get_shorter_with_more_jobs_and_longer_with_a_bootstrap() {
        assert!(estimate_minutes(1, false) > estimate_minutes(4, false));
        assert!(estimate_minutes(4, false) > estimate_minutes(22, false));
        assert!(estimate_minutes(64, false) >= 4);
        // GCC 16.2.0 took 7m19s with 22 jobs.
        assert_eq!(estimate_minutes(22, false), 8);
        assert!(estimate_minutes(8, true) > 2 * estimate_minutes(8, false));
        assert_eq!(estimate_minutes(0, false), estimate_minutes(1, false));
    }

    #[test]
    fn failures_quote_the_first_error_and_the_last_lines() {
        let mut output = Output::default();
        output.push("checking for gcc... gcc");
        output.push("a.c:1:1: error: unknown type name 'foo'");
        for n in 0..20 {
            output.push(&format!("make[2]: line {n}"));
        }
        output.push("make: *** [Makefile:1035: all] Error 2");
        let detail = output.detail();
        assert!(
            detail.starts_with("  first error:\n    a.c:1:1: error: unknown type name 'foo'\n"),
            "{detail}"
        );
        assert!(
            detail.ends_with("    make: *** [Makefile:1035: all] Error 2\n"),
            "{detail}"
        );
        assert_eq!(detail.lines().count(), 2 + 1 + Output::LAST);
        assert!(!output.libsanitizer);

        output.push("make[1]: *** [Makefile:13082: all-target-libsanitizer] Error 2");
        assert!(output.libsanitizer);
        assert_eq!(Output::default().detail(), "");
    }

    #[test]
    fn durations_read_like_a_stopwatch() {
        assert_eq!(human_duration(Duration::from_secs(14)), "14s");
        assert_eq!(human_duration(Duration::from_secs(543)), "9m 03s");
    }
}
