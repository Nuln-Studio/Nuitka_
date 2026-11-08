mod probe;
mod sources;
mod util;
mod version;

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

use sources::harvest;

pub use probe::{Probe, ProcessProbe, run_capture};
pub use sources::{Provider, Source};
pub use version::PyVersion;

pub struct Python {
    pub version: PyVersion,
    pub implementation: String,
    pub bits: u8,
    pub executable: PathBuf,
    pub prefix: PathBuf,
    pub base_prefix: PathBuf,
    pub source: Source,
}

impl Python {
    pub fn is_venv(&self) -> bool {
        self.prefix != self.base_prefix
    }

    pub fn is_cpython(&self) -> bool {
        self.implementation.eq_ignore_ascii_case("CPython")
    }

    pub fn identity(&self) -> String {
        util::normalize(&self.executable)
    }
}

impl fmt::Display for Python {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Python {} ({} {}-bit",
            self.version, self.implementation, self.bits
        )?;
        if self.is_venv() {
            write!(f, " venv")?;
        }
        write!(f, ") [{}] {}", self.source.tag(), self.executable.display())
    }
}

pub struct Policy {
    pub minimum: PyVersion,
    pub allow_prerelease: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            minimum: PyVersion::new(3, 8, 0),
            allow_prerelease: false,
        }
    }
}

impl Policy {
    pub fn accepts(&self, python: &Python) -> bool {
        python.version >= self.minimum && (self.allow_prerelease || !python.version.is_prerelease())
    }
}

fn sort(items: &mut [Python]) {
    items.sort_by(|a, b| {
        b.version
            .cmp(&a.version)
            .then_with(|| b.is_cpython().cmp(&a.is_cpython()))
            .then_with(|| b.is_venv().cmp(&a.is_venv()))
            .then_with(|| b.bits.cmp(&a.bits))
            .then_with(|| util::cmp_path(&a.executable, &b.executable))
    });
}

#[cfg(windows)]
static PROVIDERS: &[&dyn Provider] = &[
    &sources::RegistrySource,
    &sources::PathSource,
    &sources::KnownDirectorySource,
];

#[cfg(not(windows))]
static PROVIDERS: &[&dyn Provider] = &[&sources::PathSource, &sources::KnownDirectorySource];

pub fn default_providers() -> &'static [&'static dyn Provider] {
    PROVIDERS
}

pub fn discover(providers: &[&dyn Provider], prober: &dyn Probe, policy: &Policy) -> Vec<Python> {
    let candidates = harvest(providers);
    let paths: Vec<&Path> = candidates
        .iter()
        .map(|candidate| candidate.executable.as_path())
        .collect();
    let records = probe::probe_many(&paths, prober);
    let mut seen: HashSet<String> = HashSet::with_capacity(candidates.len());
    let mut out = Vec::with_capacity(candidates.len());
    for (candidate, record) in candidates.iter().zip(records) {
        let Some(record) = record else {
            continue;
        };
        let python = record.into_python(candidate.source);
        if !policy.accepts(&python) {
            continue;
        }
        if !seen.insert(python.identity()) {
            continue;
        }
        out.push(python);
    }
    sort(&mut out);
    out
}

pub fn scan(policy: &Policy) -> Vec<Python> {
    discover(default_providers(), &ProcessProbe::default(), policy)
}

#[allow(dead_code)]
pub fn probe(path: &Path) -> Option<Python> {
    if !path.is_file() {
        return None;
    }
    ProcessProbe::default()
        .probe(path)
        .map(|record| record.into_python(Source::Explicit))
}

#[cfg(test)]
mod tests {
    use super::probe::Record;
    use super::*;

    struct Fixed {
        source: Source,
        paths: Vec<PathBuf>,
    }

    impl Provider for Fixed {
        fn source(&self) -> Source {
            self.source
        }

        fn collect(&self, out: &mut Vec<PathBuf>) {
            out.extend(self.paths.iter().cloned());
        }
    }

    struct Mapped {
        records: Vec<(PathBuf, Record)>,
    }

    impl Probe for Mapped {
        fn probe(&self, executable: &Path) -> Option<Record> {
            self.records
                .iter()
                .find(|(path, _)| path == executable)
                .map(|(_, record)| record.clone())
        }
    }

    fn fixture() -> (PathBuf, PathBuf, PathBuf) {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        (
            manifest.join("Cargo.toml"),
            manifest.join("src").join("main.rs"),
            manifest.join(file!()),
        )
    }

    fn record(version: PyVersion, implementation: &str, executable: &Path) -> Record {
        Record {
            version,
            implementation: implementation.to_string(),
            bits: 64,
            executable: executable.to_path_buf(),
            prefix: executable.parent().unwrap_or(Path::new(".")).to_path_buf(),
            base_prefix: executable.parent().unwrap_or(Path::new(".")).to_path_buf(),
        }
    }

    #[test]
    fn duplicates_collapse_and_source_comes_from_first_provider() {
        let (first_path, _, _) = fixture();
        let first = Fixed {
            source: Source::Path,
            paths: vec![first_path.clone()],
        };
        let second = Fixed {
            source: Source::Registry,
            paths: vec![first_path.clone()],
        };
        let providers: [&dyn Provider; 2] = [&first, &second];
        let prober = Mapped {
            records: vec![(
                first_path.clone(),
                record(PyVersion::new(3, 12, 4), "CPython", &first_path),
            )],
        };
        let found = discover(&providers, &prober, &Policy::default());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].source, Source::Path);
    }

    #[test]
    fn results_are_sorted_by_version_then_implementation() {
        let (a, b, c) = fixture();
        let source = Fixed {
            source: Source::Path,
            paths: vec![b.clone(), a.clone(), c.clone()],
        };
        let providers: [&dyn Provider; 1] = [&source];
        let prober = Mapped {
            records: vec![
                (a.clone(), record(PyVersion::new(3, 12, 4), "CPython", &a)),
                (b.clone(), record(PyVersion::new(3, 11, 9), "CPython", &b)),
                (c.clone(), record(PyVersion::new(3, 12, 4), "PyPy", &c)),
            ],
        };
        let found = discover(&providers, &prober, &Policy::default());
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].version, PyVersion::new(3, 12, 4));
        assert!(found[0].is_cpython());
        assert!(!found[1].is_cpython());
        assert_eq!(found[1].version, PyVersion::new(3, 12, 4));
        assert_eq!(found[2].version, PyVersion::new(3, 11, 9));
    }

    #[test]
    fn failed_probes_are_dropped() {
        let (a, b, _) = fixture();
        let source = Fixed {
            source: Source::Path,
            paths: vec![a.clone(), b],
        };
        let providers: [&dyn Provider; 1] = [&source];
        let prober = Mapped {
            records: vec![(a.clone(), record(PyVersion::new(3, 12, 4), "CPython", &a))],
        };
        let found = discover(&providers, &prober, &Policy::default());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].executable, a);
    }

    #[test]
    fn policy_filters_by_minimum_version() {
        let (a, b, _) = fixture();
        let source = Fixed {
            source: Source::Path,
            paths: vec![a.clone(), b.clone()],
        };
        let providers: [&dyn Provider; 1] = [&source];
        let prober = Mapped {
            records: vec![
                (a.clone(), record(PyVersion::new(3, 12, 4), "CPython", &a)),
                (b.clone(), record(PyVersion::new(3, 7, 9), "CPython", &b)),
            ],
        };
        let found = discover(&providers, &prober, &Policy::default());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].version, PyVersion::new(3, 12, 4));
    }

    #[test]
    fn venv_is_detected_from_prefix_mismatch() {
        let (a, _, _) = fixture();
        let source = Fixed {
            source: Source::Path,
            paths: vec![a.clone()],
        };
        let providers: [&dyn Provider; 1] = [&source];
        let mut custom = record(PyVersion::new(3, 12, 4), "CPython", &a);
        custom.base_prefix = PathBuf::from(r"D:\Python3.12");
        custom.prefix = PathBuf::from(r"D:\venvs\build");
        let prober = Mapped {
            records: vec![(a, custom)],
        };
        let found = discover(&providers, &prober, &Policy::default());
        assert!(found[0].is_venv());
        assert!(found[0].to_string().contains("venv"));
    }

    #[test]
    fn prerelease_is_rejected_by_default_policy() {
        let (a, _, _) = fixture();
        let source = Fixed {
            source: Source::Path,
            paths: vec![a.clone()],
        };
        let providers: [&dyn Provider; 1] = [&source];
        let rc = PyVersion::parse("3.14.0rc1").expect("version");
        let prober = Mapped {
            records: vec![(a.clone(), record(rc, "CPython", &a))],
        };
        assert!(discover(&providers, &prober, &Policy::default()).is_empty());
        let permissive = Policy {
            allow_prerelease: true,
            ..Policy::default()
        };
        assert_eq!(discover(&providers, &prober, &permissive).len(), 1);
    }

    #[test]
    fn default_policy_targets_python_38_without_prereleases() {
        let policy = Policy::default();
        assert_eq!(policy.minimum, PyVersion::new(3, 8, 0));
        assert!(!policy.allow_prerelease);
    }

    #[test]
    fn scan_returns_real_interpreters() {
        for python in scan(&Policy::default()) {
            assert!(python.executable.is_file());
            assert!(!python.implementation.is_empty());
            assert!(matches!(python.bits, 32 | 64));
        }
    }
}
