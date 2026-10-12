//! Toolchain identity: ids (`gcc@13.3.0`, `clang@18`, `system`, `-local` suffix), version
//! parsing and matching, and per-toolchain `.cxvm-meta.toml`.
//!
//! A [`Spec`] is what the user typed and may be partial (`gcc@13`); a [`ToolchainId`] is always
//! exact (`gcc@13.3.0`) and names one toolchain in the store.

use std::cmp::Ordering;
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::error::{Error, Result};
use crate::files;
use crate::output::debug;

/// Compiler family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Gcc,
    Clang,
}

impl Kind {
    /// Every kind, in display order.
    pub const ALL: [Kind; 2] = [Kind::Gcc, Kind::Clang];

    /// Name used in ids and store directories.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Gcc => "gcc",
            Kind::Clang => "clang",
        }
    }
}

impl FromStr for Kind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        Kind::ALL
            .into_iter()
            .find(|kind| kind.as_str() == s)
            .ok_or_else(|| format!("unknown compiler `{s}` (known: gcc, clang)"))
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A version: one to three numeric components and an optional suffix (`11.4.0-local`).
///
/// Three components make it exact; fewer make it a prefix that matches exact versions.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Version {
    numbers: Vec<u32>,
    suffix: Option<String>,
}

impl Version {
    /// The first numeric component: 13 for `13.3.0`.
    pub fn major(&self) -> u32 {
        self.numbers[0]
    }

    /// The part after `-`, e.g. `local` in `11.4.0-local`.
    pub fn suffix(&self) -> Option<&str> {
        self.suffix.as_deref()
    }

    /// Whether all three numeric components are present.
    pub fn is_exact(&self) -> bool {
        self.numbers.len() == 3
    }

    /// Whether the exact version `other` is matched by `self` used as a pattern: same suffix,
    /// and `self`'s numbers are a prefix of `other`'s.
    pub fn matches(&self, other: &Version) -> bool {
        self.suffix == other.suffix && other.numbers.starts_with(&self.numbers)
    }
}

impl FromStr for Version {
    type Err = String;

    /// Parses `13`, `13.3`, `13.3.0`, `11.4.0-local`; the error is the reason only.
    fn from_str(s: &str) -> Result<Self, String> {
        if s.is_empty() {
            return Err("missing version after `@`".into());
        }
        let (numbers, suffix) = match s.split_once('-') {
            Some((numbers, suffix)) => (numbers, Some(parse_suffix(suffix)?)),
            None => (s, None),
        };
        let numbers = numbers
            .split('.')
            .map(parse_number)
            .collect::<Result<Vec<_>, _>>()?;
        if numbers.len() > 3 {
            return Err("a version has at most three components (`13.3.0`)".into());
        }
        Ok(Version { numbers, suffix })
    }
}

fn parse_number(s: &str) -> Result<u32, String> {
    if s.is_empty() {
        return Err("empty version component".into());
    }
    if !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("`{s}` is not a number"));
    }
    if s.len() > 1 && s.starts_with('0') {
        return Err(format!("leading zero in `{s}`"));
    }
    s.parse()
        .map_err(|_| format!("version component `{s}` is too large"))
}

fn parse_suffix(s: &str) -> Result<String, String> {
    if s.is_empty() {
        return Err("empty suffix after `-`".into());
    }
    let valid_first = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    let valid_rest = |b: u8| valid_first(b) || matches!(b, b'.' | b'_' | b'-');
    let bytes = s.as_bytes();
    if !valid_first(bytes[0]) || !bytes[1..].iter().all(|&b| valid_rest(b)) {
        return Err(format!(
            "suffix `{s}` must start with a lowercase letter or digit and contain only \
             lowercase letters, digits, `.`, `_` and `-`"
        ));
    }
    Ok(s.to_owned())
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, n) in self.numbers.iter().enumerate() {
            if i > 0 {
                f.write_str(".")?;
            }
            write!(f, "{n}")?;
        }
        if let Some(suffix) = &self.suffix {
            write!(f, "-{suffix}")?;
        }
        Ok(())
    }
}

impl Ord for Version {
    /// Numeric order, then unsuffixed before suffixed, then suffixes alphabetically.
    fn cmp(&self, other: &Self) -> Ordering {
        // `Option` orders `None` first, which puts unsuffixed versions first.
        (&self.numbers, &self.suffix).cmp(&(&other.numbers, &other.suffix))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// An exact toolchain id such as `gcc@13.3.0` or `gcc@11.4.0-local`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ToolchainId {
    kind: Kind,
    version: Version,
}

impl ToolchainId {
    /// Builds an id; `None` if `version` is not exact.
    pub fn new(kind: Kind, version: Version) -> Option<Self> {
        version.is_exact().then_some(Self { kind, version })
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn version(&self) -> &Version {
        &self.version
    }
}

impl fmt::Display for ToolchainId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.kind, self.version)
    }
}

impl FromStr for ToolchainId {
    type Err = Error;

    /// Parses an exact id; partial versions and `system` are rejected.
    fn from_str(s: &str) -> Result<Self> {
        let not_exact = || Error::InvalidId {
            input: s.to_owned(),
            reason: "an exact `<major>.<minor>.<patch>` version is required here".into(),
        };
        match s.parse::<Spec>()? {
            Spec::System => Err(not_exact()),
            Spec::Toolchain { kind, version } => {
                ToolchainId::new(kind, version).ok_or_else(not_exact)
            }
        }
    }
}

/// A toolchain as requested by the user, `.cxvmrc`, the global config or `CXVM_COMPILER`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Spec {
    /// Whatever `cc` / `c++` resolve to on `PATH`, outside cxvm.
    System,
    /// A kind and a possibly partial version.
    Toolchain { kind: Kind, version: Version },
}

impl Spec {
    /// Whether the spec names exactly one toolchain (or `system`).
    pub fn is_exact(&self) -> bool {
        match self {
            Spec::System => true,
            Spec::Toolchain { version, .. } => version.is_exact(),
        }
    }

    /// The highest candidate this spec matches. `system` matches none.
    pub fn select<'a, I>(&self, candidates: I) -> Option<&'a ToolchainId>
    where
        I: IntoIterator<Item = &'a ToolchainId>,
    {
        let Spec::Toolchain { kind, version } = self else {
            return None;
        };
        candidates
            .into_iter()
            .filter(|id| id.kind == *kind && version.matches(&id.version))
            .max()
    }
}

impl fmt::Display for Spec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Spec::System => f.write_str("system"),
            Spec::Toolchain { kind, version } => write!(f, "{kind}@{version}"),
        }
    }
}

impl FromStr for Spec {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        let invalid = |reason: String| Error::InvalidId {
            input: s.to_owned(),
            reason,
        };
        if s == "system" {
            return Ok(Spec::System);
        }
        if s.is_empty() {
            return Err(invalid("the id is empty".into()));
        }
        let Some((kind, version)) = s.split_once('@') else {
            let kind: Kind = s.parse().map_err(invalid)?;
            return Err(invalid(format!("missing `@<version>` after `{kind}`")));
        };
        let kind: Kind = kind.parse().map_err(invalid)?;
        if version.contains('@') {
            return Err(invalid("unexpected `@` in the version".into()));
        }
        let version: Version = version.parse().map_err(invalid)?;
        Ok(Spec::Toolchain { kind, version })
    }
}

/// The directories of `path_var` that belong to the system: without the entries cxvm manages
/// (inside `store`, or inside a `.cxvm` directory) and the relative ones.
pub fn unmanaged_path(path_var: Option<&OsStr>, store: &Path) -> Vec<PathBuf> {
    let managed =
        |dir: &Path| dir.starts_with(store) || dir.components().any(|c| c.as_os_str() == ".cxvm");
    path_var
        .map(std::env::split_paths)
        .into_iter()
        .flatten()
        .filter(|dir| dir.is_absolute() && !managed(dir))
        .collect()
}

/// The `system` compiler: the first executable `name` (`cc`, `c++`) on `path_var`, skipping
/// entries that cxvm manages (see [`unmanaged_path`]).
pub fn find_system(name: &str, path_var: Option<&OsStr>, store: &Path) -> Option<PathBuf> {
    let found = unmanaged_path(path_var, store)
        .into_iter()
        .map(|dir| dir.join(name))
        .find(|candidate| files::is_executable(candidate));
    match &found {
        Some(path) => debug!("the system `{name}` is {}", path.display()),
        None => debug!("no `{name}` on PATH outside cxvm's directories"),
    }
    found
}

impl From<ToolchainId> for Spec {
    fn from(id: ToolchainId) -> Self {
        Spec::Toolchain {
            kind: id.kind,
            version: id.version,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(s: &str) -> Spec {
        s.parse().unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    fn id(s: &str) -> ToolchainId {
        s.parse().unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    fn spec_error(s: &str) -> String {
        match s.parse::<Spec>() {
            Ok(parsed) => panic!("`{s}` parsed as {parsed:?}"),
            Err(e) => e.to_string(),
        }
    }

    #[test]
    fn parses_exact_partial_and_system_specs() {
        assert_eq!(spec("system"), Spec::System);
        for (input, kind, exact) in [
            ("gcc@13.3.0", Kind::Gcc, true),
            ("gcc@13.3", Kind::Gcc, false),
            ("gcc@13", Kind::Gcc, false),
            ("clang@18.1.8", Kind::Clang, true),
            ("clang@18", Kind::Clang, false),
            ("gcc@11.4.0-local", Kind::Gcc, true),
            ("gcc@11-local", Kind::Gcc, false),
            ("gcc@0.0.0", Kind::Gcc, true),
        ] {
            let parsed = spec(input);
            assert!(
                matches!(parsed, Spec::Toolchain { kind: k, .. } if k == kind),
                "{input}"
            );
            assert_eq!(parsed.is_exact(), exact, "{input}");
            assert_eq!(parsed.to_string(), input, "round trip");
        }
        assert!(Spec::System.is_exact());
    }

    #[test]
    fn rejects_malformed_ids_with_a_reason_and_a_hint() {
        for (input, reason) in [
            ("", "empty"),
            ("gcc", "missing `@<version>`"),
            ("gcc@", "missing version"),
            ("msvc@19", "unknown compiler `msvc`"),
            ("GCC@13", "unknown compiler `GCC`"),
            ("gcc@13.3.0.1", "at most three"),
            ("gcc@13..0", "empty version component"),
            ("gcc@.13", "empty version component"),
            ("gcc@13.x", "`x` is not a number"),
            ("gcc@+13", "`+13` is not a number"),
            ("gcc@013", "leading zero"),
            ("gcc@99999999999", "too large"),
            ("gcc@13-", "empty suffix"),
            ("gcc@13-Local", "suffix `Local`"),
            ("gcc@13-_x", "suffix `_x`"),
            ("gcc@13@14", "unexpected `@`"),
            ("System", "unknown compiler `System`"),
        ] {
            let err = spec_error(input);
            assert!(err.contains(&format!("`{input}`")), "{input}: {err}");
            assert!(
                err.contains(reason),
                "{input}: expected {reason:?} in {err}"
            );
            assert!(err.contains("hint:"), "{input}: {err}");
        }
    }

    #[test]
    fn unknown_compiler_error_lists_the_known_ones() {
        let err = spec_error("msvc@19");
        assert!(err.contains("gcc") && err.contains("clang"), "{err}");
    }

    #[test]
    fn toolchain_id_requires_an_exact_version() {
        assert_eq!(id("gcc@13.3.0").to_string(), "gcc@13.3.0");
        assert_eq!(id("gcc@11.4.0-local").version().to_string(), "11.4.0-local");
        for input in ["gcc@13", "clang@18.1", "system"] {
            let err = input.parse::<ToolchainId>().unwrap_err().to_string();
            assert!(err.contains(&format!("`{input}`")), "{err}");
            assert!(err.contains("exact"), "{input}: {err}");
        }
        assert!(ToolchainId::new(Kind::Gcc, "13".parse().unwrap()).is_none());
    }

    #[test]
    fn versions_order_numerically_then_unsuffixed_first() {
        let mut versions: Vec<Version> = [
            "13.10.0",
            "13.2.0",
            "9.5.0",
            "13.2.0-local",
            "13.2.0-abc",
            "14.1.0",
        ]
        .iter()
        .map(|v| v.parse().unwrap())
        .collect();
        versions.sort();
        let sorted: Vec<String> = versions.iter().map(Version::to_string).collect();
        assert_eq!(
            sorted,
            [
                "9.5.0",
                "13.2.0",
                "13.2.0-abc",
                "13.2.0-local",
                "13.10.0",
                "14.1.0"
            ]
        );
    }

    #[test]
    fn ids_sort_by_kind_then_version() {
        let mut ids = [id("clang@18.1.8"), id("gcc@14.2.0"), id("gcc@9.5.0")];
        ids.sort();
        let sorted: Vec<String> = ids.iter().map(ToString::to_string).collect();
        assert_eq!(sorted, ["gcc@9.5.0", "gcc@14.2.0", "clang@18.1.8"]);
    }

    fn installed() -> Vec<ToolchainId> {
        [
            "gcc@12.4.0",
            "gcc@13.2.0",
            "gcc@13.3.0",
            "gcc@13.10.1",
            "gcc@11.4.0-local",
            "gcc@11.5.0-local",
            "gcc@11.4.0-opt",
            "clang@18.1.8",
            "clang@13.0.1",
        ]
        .iter()
        .map(|s| id(s))
        .collect()
    }

    fn select(s: &str) -> Option<String> {
        let candidates = installed();
        spec(s).select(&candidates).map(ToString::to_string)
    }

    #[test]
    fn partial_versions_select_the_highest_match() {
        assert_eq!(select("gcc@13").as_deref(), Some("gcc@13.10.1"));
        assert_eq!(select("gcc@13.3").as_deref(), Some("gcc@13.3.0"));
        assert_eq!(select("gcc@13.2").as_deref(), Some("gcc@13.2.0"));
        assert_eq!(select("clang@18").as_deref(), Some("clang@18.1.8"));
    }

    #[test]
    fn partial_versions_match_whole_components_only() {
        // `gcc@1` must not match 12.x or 13.x.
        assert_eq!(select("gcc@1"), None);
        assert_eq!(select("gcc@13.1"), None);
    }

    #[test]
    fn exact_versions_select_only_themselves() {
        assert_eq!(select("gcc@13.3.0").as_deref(), Some("gcc@13.3.0"));
        assert_eq!(select("gcc@13.3.1"), None);
    }

    #[test]
    fn selection_respects_the_kind() {
        // clang@13.0.1 is installed, but gcc@13.0 is not.
        assert_eq!(select("gcc@13.0"), None);
        assert_eq!(select("clang@13").as_deref(), Some("clang@13.0.1"));
    }

    #[test]
    fn suffixes_are_part_of_the_identity() {
        assert_eq!(
            select("gcc@11"),
            None,
            "linked toolchains are never picked implicitly"
        );
        assert_eq!(select("gcc@11-local").as_deref(), Some("gcc@11.5.0-local"));
        assert_eq!(select("gcc@11.4-opt").as_deref(), Some("gcc@11.4.0-opt"));
        assert_eq!(select("gcc@12-local"), None);
        assert_eq!(select("gcc@13-local"), None);
    }

    #[test]
    fn system_selects_nothing_from_the_store() {
        assert_eq!(select("system"), None);
    }

    mod system {
        use std::ffi::OsString;
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        use super::*;

        fn executable(path: &Path, mode: u32) -> PathBuf {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "#!/bin/sh\n").unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
            path.to_owned()
        }

        fn path_var(dirs: &[PathBuf]) -> OsString {
            std::env::join_paths(dirs).unwrap()
        }

        #[test]
        fn finds_the_first_executable_on_path() {
            let dir = tempfile::tempdir().unwrap();
            let store = dir.path().join("store");
            let first = executable(&dir.path().join("usr/bin/cc"), 0o755);
            executable(&dir.path().join("opt/bin/cc"), 0o755);
            let var = path_var(&[
                dir.path().join("empty"),
                dir.path().join("usr/bin"),
                dir.path().join("opt/bin"),
            ]);
            assert_eq!(find_system("cc", Some(&var), &store), Some(first));
        }

        #[test]
        fn skips_non_executables_and_directories() {
            let dir = tempfile::tempdir().unwrap();
            let store = dir.path().join("store");
            executable(&dir.path().join("a/cc"), 0o644);
            fs::create_dir_all(dir.path().join("b/cc")).unwrap();
            let good = executable(&dir.path().join("c/cc"), 0o700);
            let var = path_var(&[
                dir.path().join("a"),
                dir.path().join("b"),
                dir.path().join("c"),
            ]);
            assert_eq!(find_system("cc", Some(&var), &store), Some(good));
        }

        #[test]
        fn skips_cxvm_managed_directories() {
            let dir = tempfile::tempdir().unwrap();
            let store = dir.path().join("store");
            let in_store = store.join("toolchains/gcc/13.3.0/bin");
            let in_project = dir.path().join("proj/.cxvm/current/bin");
            executable(&in_store.join("cc"), 0o755);
            executable(&in_project.join("cc"), 0o755);
            let system = executable(&dir.path().join("usr/bin/cc"), 0o755);
            let var = path_var(&[in_store, in_project, dir.path().join("usr/bin")]);
            assert_eq!(find_system("cc", Some(&var), &store), Some(system));
        }

        #[test]
        fn skips_relative_and_empty_entries() {
            let dir = tempfile::tempdir().unwrap();
            let var = OsString::from(":.:bin");
            assert_eq!(find_system("cc", Some(&var), dir.path()), None);
        }

        #[test]
        fn nothing_found_without_path() {
            assert_eq!(find_system("cc", None, Path::new("/store")), None);
        }
    }

    #[test]
    fn exact_spec_converts_from_an_id() {
        assert_eq!(Spec::from(id("gcc@13.3.0")), spec("gcc@13.3.0"));
    }
}
