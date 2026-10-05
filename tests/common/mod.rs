//! Shared fixture for integration tests: a throwaway store, home, cache and project tree, and a
//! `cxvm` command that sees only those (never the real store or environment).

#![allow(dead_code)]

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use assert_cmd::Command;

pub struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    /// Empty store, a project tree `proj/a/b/c`, a `bin/` on `PATH` holding a fake `cc` and
    /// `c++`, and an empty catalog.
    pub fn new() -> Self {
        let sandbox = Self {
            dir: tempfile::tempdir().expect("temp dir"),
        };
        fs::create_dir_all(sandbox.project().join("a/b/c")).unwrap();
        fs::create_dir_all(sandbox.path("home")).unwrap();
        fs::write(sandbox.empty_catalog(), "").unwrap();
        sandbox.fake_compiler(&sandbox.bin(), "cc");
        sandbox.fake_compiler(&sandbox.bin(), "c++");
        sandbox
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    pub fn store(&self) -> PathBuf {
        self.path("store")
    }

    pub fn project(&self) -> PathBuf {
        self.path("proj")
    }

    pub fn bin(&self) -> PathBuf {
        self.path("bin")
    }

    /// The catalog `cxvm` gets by default: with the one built into cxvm, a test that installs
    /// by mistake would download a real release.
    fn empty_catalog(&self) -> PathBuf {
        self.path("empty-catalog.toml")
    }

    /// Writes an executable script `dir/name` that prints a version line.
    pub fn fake_compiler(&self, dir: &Path, name: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\necho '{name} (fake) 1.0.0'\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// Places a toolchain in the store by hand, e.g. `install("gcc", "13.3.0")`.
    pub fn install(&self, kind: &str, version: &str) -> PathBuf {
        let prefix = self.store().join("toolchains").join(kind).join(version);
        fake_toolchain(&prefix, kind, version);
        prefix
    }

    /// Writes `proj/<rel>/.cxvmrc`.
    pub fn pin(&self, rel: &str, compiler: &str) -> PathBuf {
        let path = self.project().join(rel).join(".cxvmrc");
        fs::write(&path, format!("compiler = \"{compiler}\"\n")).unwrap();
        path
    }

    /// Writes the global `config.toml`.
    pub fn global(&self, compiler: &str) -> PathBuf {
        let path = self.store().join("config.toml");
        fs::create_dir_all(self.store()).unwrap();
        fs::write(&path, format!("compiler = \"{compiler}\"\n")).unwrap();
        path
    }

    /// `cxvm` run from `proj/<rel>` with a clean environment pointing into the sandbox, and
    /// nothing to install unless the test sets `CXVM_CATALOG` or removes it.
    pub fn cxvm(&self, rel: &str) -> Command {
        let mut cmd = Command::cargo_bin("cxvm").expect("cxvm binary built by cargo");
        cmd.current_dir(self.project().join(rel)).env_clear();
        for (name, value) in self.env() {
            cmd.env(name, value);
        }
        cmd
    }

    /// Like [`Sandbox::cxvm`], as a `std` command: for tests that need the running process,
    /// e.g. to send it a signal.
    pub fn std_cxvm(&self, rel: &str) -> std::process::Command {
        let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin("cxvm"));
        cmd.current_dir(self.project().join(rel)).env_clear();
        for (name, value) in self.env() {
            cmd.env(name, value);
        }
        cmd
    }

    fn env(&self) -> [(&'static str, PathBuf); 5] {
        [
            ("HOME", self.path("home")),
            ("CXVM_HOME", self.store()),
            ("XDG_CACHE_HOME", self.path("cache")),
            ("CXVM_CATALOG", self.empty_catalog()),
            ("PATH", self.bin()),
        ]
    }

    /// The sandbox's `bin/` followed by the host's `PATH`: for tests that run real programs
    /// such as `make`.
    pub fn path_with_host(&self) -> std::ffi::OsString {
        let host = std::env::var_os("PATH").unwrap_or_default();
        std::env::join_paths(std::iter::once(self.bin()).chain(std::env::split_paths(&host)))
            .unwrap()
    }

    /// Writes an executable script `dir/name` that "compiles": with `-o <out>` it writes an
    /// executable `out` printing what cxvm's checks expect.
    pub fn compiling_compiler(&self, dir: &Path, name: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, clang_script("1.0.0", true)).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
}

/// Writes `prefix/bin/{gcc,g++}` or `prefix/bin/{clang,clang++}` scripts that answer the
/// probes cxvm runs (`-dumpfullversion`, `--version`, `-print-file-name=…`) like the real
/// drivers. The fake clang's `-dumpversion` prints `4.2.1`, as old releases did.
pub fn fake_toolchain(prefix: &Path, kind: &str, version: &str) {
    let major = version.split('.').next().unwrap();
    let (drivers, script) = match kind {
        "gcc" => (
            ["gcc", "g++"],
            format!(
                "#!/bin/sh\ncase \"$1\" in\n\
                 -dumpfullversion) echo {version} ;;\n\
                 -dumpversion) echo {major} ;;\n\
                 --version) echo \"${{0##*/}} (Fake) {version}\" ;;\n\
                 -print-file-name=*) f=\"${{1#-print-file-name=}}\"; \
                 d=\"${{0%/*}}/../lib64\"; \
                 if [ -e \"$d/$f\" ]; then echo \"$d/$f\"; else echo \"$f\"; fi ;;\n\
                 *) echo \"${{0##*/}} {version} $*\" ;;\n\
                 esac\n"
            ),
        ),
        "clang" => (["clang", "clang++"], clang_script(version, true)),
        other => panic!("unknown kind {other}"),
    };
    for driver in drivers {
        let path = prefix.join("bin").join(driver);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// A fake clang driver for `version`. With `-o <out>` it writes an executable `out` that prints
/// what cxvm's smoke test expects, or fails like a compiler without C++ headers when `compiles`
/// is false. The fake `-dumpversion` prints `4.2.1`, as old releases did.
pub fn clang_script(version: &str, compiles: bool) -> String {
    let compile = if compiles {
        "printf '#!/bin/sh\\necho cxvm-ok\\n' > \"$out\"; /bin/chmod +x \"$out\""
    } else {
        "echo 'fatal error: iostream: file not found' >&2; exit 1"
    };
    format!(
        "#!/bin/sh\ncase \"$1\" in\n\
         -dumpversion) echo 4.2.1 ;;\n\
         --version) echo 'clang version {version} (fake)' ;;\n\
         *) out=; prev=\n\
         for a in \"$@\"; do [ \"$prev\" = -o ] && out=\"$a\"; prev=\"$a\"; done\n\
         if [ -n \"$out\" ]; then {compile}; else echo \"${{0##*/}} {version} $*\"; fi ;;\n\
         esac\n"
    )
}

fn tar_dir(builder: &mut tar::Builder<Vec<u8>>, path: &str) {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Directory);
    header.set_mode(0o755);
    header.set_size(0);
    builder
        .append_data(&mut header, path, std::io::empty())
        .unwrap();
}

fn tar_file(builder: &mut tar::Builder<Vec<u8>>, path: &str, mode: u32, content: &str) {
    let mut header = tar::Header::new_gnu();
    header.set_mode(mode);
    header.set_size(content.len() as u64);
    builder
        .append_data(&mut header, path, content.as_bytes())
        .unwrap();
}

fn tar_symlink(builder: &mut tar::Builder<Vec<u8>>, path: &str, target: &str) {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_size(0);
    builder.append_link(&mut header, path, target).unwrap();
}

/// A `.tar.xz` laid out like an LLVM release: one top-level directory holding `bin/clang` (a
/// fake reporting `version`, see [`clang_script`]) and a `bin/clang++` symlink to it.
pub fn fake_clang_archive(version: &str, compiles: bool) -> Vec<u8> {
    let top = format!("LLVM-{version}-Linux-X64");
    let mut builder = tar::Builder::new(Vec::new());
    tar_dir(&mut builder, &format!("{top}/bin/"));
    tar_file(
        &mut builder,
        &format!("{top}/bin/clang"),
        0o755,
        &clang_script(version, compiles),
    );
    tar_symlink(&mut builder, &format!("{top}/bin/clang++"), "clang");
    let tar = builder.into_inner().unwrap();
    let mut xz = liblzma::write::XzEncoder::new(Vec::new(), 1);
    std::io::Write::write_all(&mut xz, &tar).unwrap();
    xz.finish().unwrap()
}

/// A fake xPack GCC release, see [`FakeGcc::archive`].
pub struct FakeGcc<'a> {
    pub version: &'a str,
    /// Whether the driver "builds" the smoke test's program or fails like a GCC whose linker
    /// cannot read the host's libraries.
    pub compiles: bool,
    /// Whether the archive holds `lib64/libstdc++.so.6`.
    pub libstdcxx: bool,
}

impl<'a> FakeGcc<'a> {
    pub fn new(version: &'a str) -> Self {
        Self {
            version,
            compiles: true,
            libstdcxx: true,
        }
    }

    /// The driver script: it answers the probes like an xPack GCC (paths below `bin/..`, not
    /// normalised), and with `-o <out>` writes an executable `out` that prints what cxvm's
    /// smoke test expects.
    fn script(&self) -> String {
        let version = self.version;
        let major = version.split('.').next().unwrap();
        let compile = if self.compiles {
            "printf '#!/bin/sh\\necho cxvm-ok\\n' > \"$out\"; /bin/chmod +x \"$out\""
        } else {
            "echo \"ld: libm.so.6: unknown type [0x13] section '.relr.dyn'\" >&2; exit 1"
        };
        format!(
            "#!/bin/sh\nprefix=\"${{0%/*}}/..\"\ncase \"$1\" in\n\
             -dumpfullversion) echo {version} ;;\n\
             -dumpversion) echo {major} ;;\n\
             --version) echo \"${{0##*/}} (fake xPack GCC) {version}\" ;;\n\
             -print-file-name=*) echo \"$prefix/lib64/${{1#-print-file-name=}}\" ;;\n\
             -print-libgcc-file-name) \
             echo \"$prefix/lib/gcc/x86_64-pc-linux-gnu/{version}/libgcc.a\" ;;\n\
             *) out=; prev=\n\
             for a in \"$@\"; do [ \"$prev\" = -o ] && out=\"$a\"; prev=\"$a\"; done\n\
             if [ -n \"$out\" ]; then {compile}; else echo \"${{0##*/}} {version} $*\"; fi ;;\n\
             esac\n"
        )
    }

    /// Writes the toolchain into `prefix`, like an unpacked [`FakeGcc::archive`].
    pub fn install_into(&self, prefix: &Path) {
        let version = self.version;
        for driver in ["gcc", "g++"] {
            let path = prefix.join("bin").join(driver);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, self.script()).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let install_dir = prefix.join(format!("lib/gcc/x86_64-pc-linux-gnu/{version}"));
        fs::create_dir_all(&install_dir).unwrap();
        fs::write(install_dir.join("libgcc.a"), "").unwrap();
        fs::create_dir_all(prefix.join("lib64")).unwrap();
        fs::write(prefix.join("lib64/libstdc++.so.6"), "").unwrap();
    }

    /// A `.tar.gz` laid out like an xPack GCC release: one top-level directory holding
    /// `bin/gcc`, `bin/g++`, `lib64/libstdc++.so.6` and `lib/gcc/<triple>/<version>/libgcc.a`.
    pub fn archive(&self) -> Vec<u8> {
        let version = self.version;
        let top = format!("xpack-gcc-{version}-1");
        let script = self.script();
        let mut builder = tar::Builder::new(Vec::new());
        tar_dir(&mut builder, &format!("{top}/bin/"));
        tar_file(&mut builder, &format!("{top}/bin/gcc"), 0o755, &script);
        tar_file(&mut builder, &format!("{top}/bin/g++"), 0o755, &script);
        if self.libstdcxx {
            tar_file(
                &mut builder,
                &format!("{top}/lib64/libstdc++.so.6.0.32"),
                0o755,
                "",
            );
            tar_symlink(
                &mut builder,
                &format!("{top}/lib64/libstdc++.so.6"),
                "libstdc++.so.6.0.32",
            );
        }
        tar_file(
            &mut builder,
            &format!("{top}/lib/gcc/x86_64-pc-linux-gnu/{version}/libgcc.a"),
            0o644,
            "",
        );
        let tar = builder.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&tar).unwrap();
        gz.finish().unwrap()
    }
}

/// A fake GCC source release, see [`FakeSource::archive`].
pub struct FakeSource<'a> {
    pub version: &'a str,
    /// The content of `contrib/download_prerequisites`.
    pub prerequisites: &'a str,
    /// Shell commands `make` runs to "build", e.g. `exit 1`.
    pub build: &'a str,
}

/// What GCC 10's `contrib/download_prerequisites` names.
pub const PREREQUISITES: &str = "gmp='gmp-6.1.0.tar.bz2'\nmpfr='mpfr-3.1.6.tar.bz2'\n\
                                 mpc='mpc-1.0.3.tar.gz'\nisl='isl-0.18.tar.bz2'\n";

impl<'a> FakeSource<'a> {
    pub fn new(version: &'a str) -> Self {
        Self {
            version,
            prerequisites: PREREQUISITES,
            build: "@echo building fake gcc",
        }
    }

    /// A `.tar.xz` laid out like a GCC source release: `gcc-<version>/configure`, which
    /// writes a `Makefile` and `configure.record` (its arguments, `CC`, `CXX`, `PATH` and the
    /// libraries found in the tree), and `contrib/download_prerequisites`. `make` runs
    /// [`FakeSource::build`]; `make install-strip` installs a [`FakeGcc`] and the record, as
    /// `share/configure.record`, below `$(DESTDIR)$(prefix)`.
    pub fn archive(&self) -> Vec<u8> {
        let version = self.version;
        let top = format!("gcc-{version}");
        let configure = format!(
            "#!/bin/sh\nsrc=\"${{0%/*}}\"\nprefix=\n\
             for a in \"$@\"; do case \"$a\" in --prefix=*) prefix=\"${{a#--prefix=}}\" ;; esac; done\n\
             {{ echo \"args: $*\"; echo \"CC=$CC\"; echo \"CXX=$CXX\"; echo \"PATH=$PATH\"\n\
             echo \"CFLAGS=${{CFLAGS-unset}}\"\n\
             for d in gmp mpfr mpc isl; do\n\
             [ -f \"$src/$d/VERSION\" ] && echo \"with $d: $(cat \"$src/$d/VERSION\")\"\n\
             done; }} > configure.record\n\
             echo 'checking for a fake compiler... yes'\n\
             cat > Makefile <<EOF\n\
             prefix = $prefix\nsrc = $src\n\
             gccdir = \\$(DESTDIR)\\$(prefix)/lib/gcc/x86_64-pc-linux-gnu/{version}\n\
             all:\n\t{build}\n\
             install-strip:\n\
             \tmkdir -p \\$(DESTDIR)\\$(prefix)/bin \\$(DESTDIR)\\$(prefix)/lib64\n\
             \tmkdir -p \\$(DESTDIR)\\$(prefix)/share \\$(gccdir)\n\
             \tcp \\$(src)/fake-driver \\$(DESTDIR)\\$(prefix)/bin/gcc\n\
             \tcp \\$(src)/fake-driver \\$(DESTDIR)\\$(prefix)/bin/g++\n\
             \ttouch \\$(DESTDIR)\\$(prefix)/lib64/libstdc++.so.6 \\$(gccdir)/libgcc.a\n\
             \tcp configure.record \\$(DESTDIR)\\$(prefix)/share/configure.record\n\
             EOF\n",
            build = self.build
        );
        let mut builder = tar::Builder::new(Vec::new());
        tar_dir(&mut builder, &format!("{top}/contrib/"));
        tar_file(&mut builder, &format!("{top}/configure"), 0o755, &configure);
        tar_file(
            &mut builder,
            &format!("{top}/fake-driver"),
            0o755,
            &FakeGcc::new(version).script(),
        );
        tar_file(
            &mut builder,
            &format!("{top}/contrib/download_prerequisites"),
            0o755,
            self.prerequisites,
        );
        let tar = builder.into_inner().unwrap();
        let mut xz = liblzma::write::XzEncoder::new(Vec::new(), 1);
        std::io::Write::write_all(&mut xz, &tar).unwrap();
        xz.finish().unwrap()
    }
}

/// A fake library tarball named `asset` (`gmp-6.1.0.tar.bz2`, `mpc-1.0.3.tar.gz`): one
/// top-level directory holding a `VERSION` file with the directory's name.
pub fn fake_prerequisite(asset: &str) -> Vec<u8> {
    let (top, ext) = asset.split_once(".tar.").expect("a tarball name");
    let mut builder = tar::Builder::new(Vec::new());
    tar_file(&mut builder, &format!("{top}/VERSION"), 0o644, top);
    let tar = builder.into_inner().unwrap();
    match ext {
        "bz2" => {
            let mut bz = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
            bz.write_all(&tar).unwrap();
            bz.finish().unwrap()
        }
        "gz" => {
            let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            gz.write_all(&tar).unwrap();
            gz.finish().unwrap()
        }
        other => panic!("unknown compression {other}"),
    }
}

/// Appends one `[[prerequisite]]` entry per `(asset, url, archive)` to the catalog at `path`.
pub fn append_prerequisites(path: &Path, entries: &[(&str, &str, &[u8])]) {
    let mut text = fs::read_to_string(path).unwrap_or_default();
    for (asset, url, archive) in entries {
        text.push_str(&format!(
            "\n[[prerequisite]]\nasset = \"{asset}\"\nurl = \"{url}\"\nsize = {}\n\
             sha256 = \"{}\"\nsource = \"test\"\n",
            archive.len(),
            sha256_hex(archive)
        ));
    }
    fs::write(path, text).unwrap();
}

/// The archive name a release of `kind` has upstream.
pub fn asset_name(kind: &str, version: &str) -> String {
    match kind {
        "clang" => format!("LLVM-{version}-Linux-X64.tar.xz"),
        "gcc" => format!("xpack-gcc-{version}-1-linux-x64.tar.gz"),
        "gcc-source" => format!("gcc-{version}.tar.xz"),
        other => panic!("unknown kind {other}"),
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Writes a catalog file at `path` with one entry per `(kind, version, url, archive)`.
pub fn write_catalog(path: &Path, entries: &[(&str, &str, &str, &[u8])]) {
    let text: Vec<String> = entries
        .iter()
        .map(|(kind, version, url, archive)| {
            format!(
                "[[{kind}]]\nversion = \"{version}\"\nasset = \"{}\"\n\
                 url = \"{url}\"\nsize = {}\nsha256 = \"{}\"\nsource = \"test\"\n",
                asset_name(kind, version),
                archive.len(),
                sha256_hex(archive)
            )
        })
        .collect();
    fs::write(path, text.join("\n")).unwrap();
}

/// A minimal HTTP/1.1 file server on 127.0.0.1 for download tests, with `Range` support.
pub struct Server {
    /// `http://127.0.0.1:<port>`
    pub base: String,
    /// Each request as `<path>` or `<path> <range header>`.
    pub requests: Arc<Mutex<Vec<String>>>,
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    /// When set, the next response announces the whole body but stops after this many bytes.
    truncate_next: Arc<Mutex<Option<usize>>>,
}

impl Server {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let server = Self {
            base: format!("http://{}", listener.local_addr().unwrap()),
            requests: Arc::default(),
            files: Arc::default(),
            truncate_next: Arc::default(),
        };
        let (requests, files, truncate) = (
            Arc::clone(&server.requests),
            Arc::clone(&server.files),
            Arc::clone(&server.truncate_next),
        );
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let _ = respond(stream, &requests, &files, &truncate);
            }
        });
        server
    }

    /// Serves `content` at `/<name>` and returns its URL.
    pub fn add(&self, name: &str, content: Vec<u8>) -> String {
        self.files
            .lock()
            .unwrap()
            .insert(format!("/{name}"), content);
        format!("{}/{name}", self.base)
    }

    pub fn truncate_next_response(&self, bytes: usize) {
        *self.truncate_next.lock().unwrap() = Some(bytes);
    }

    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

fn respond(
    stream: TcpStream,
    requests: &Mutex<Vec<String>>,
    files: &Mutex<HashMap<String, Vec<u8>>>,
    truncate: &Mutex<Option<usize>>,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_owned();
    let mut range = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("range")
        {
            range = Some(value.trim().to_owned());
        }
    }
    requests.lock().unwrap().push(match &range {
        Some(range) => format!("{path} {range}"),
        None => path.clone(),
    });
    let mut stream = stream;
    let Some(content) = files.lock().unwrap().get(&path).cloned() else {
        return stream.write_all(
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
    };
    let start = range
        .as_deref()
        .and_then(|r| r.strip_prefix("bytes="))
        .and_then(|r| r.strip_suffix('-'))
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|&n| n < content.len());
    let (status, body, extra) = match start {
        Some(start) => (
            "206 Partial Content",
            &content[start..],
            format!(
                "Content-Range: bytes {start}-{}/{}\r\n",
                content.len() - 1,
                content.len()
            ),
        ),
        None => ("200 OK", &content[..], String::new()),
    };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
        body.len()
    )?;
    let limit = truncate
        .lock()
        .unwrap()
        .take()
        .unwrap_or(body.len())
        .min(body.len());
    stream.write_all(&body[..limit])?;
    stream.flush()
}

pub fn stdout(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("UTF-8 stdout")
}
