use semver::Version;

use super::paths;
use super::python_discovery::{self, Policy, PyVersion, Python, Source};
use crate::error::{NuitkaRsError, PythonCandidate, Result, SourceKind};

const MINIMUM: PyVersion = PyVersion::new(3, 8, 0);
pub(crate) const RECOMMENDED: PyVersion = PyVersion::new(3, 11, 0);

pub(crate) fn select_python() -> Result<PythonCandidate> {
    let mut found = python_discovery::scan(&Policy {
        minimum: PyVersion::new(0, 0, 0),
        allow_prerelease: true,
    });
    if found.is_empty() {
        return Err(NuitkaRsError::PythonNotFound);
    }

    // 准备环境时用户明确选过 Python 版本；优先用它，而不是简单取最高版本。
    if let Some(line) = paths::selected_python()
        && let Some((major, minor)) = parse_line(&line)
        && let Some(index) = found
            .iter()
            .position(|python| python.version.major == major && python.version.minor == minor)
    {
        return Ok(into_candidate(found.remove(index)));
    }

    let available: Vec<String> = found
        .iter()
        .map(|python| to_semver(python.version).to_string())
        .collect();
    let supported = Policy {
        minimum: MINIMUM,
        allow_prerelease: false,
    };
    let preferred = Policy {
        minimum: RECOMMENDED,
        allow_prerelease: false,
    };
    let mut fallback = None;
    for python in found {
        if preferred.accepts(&python) {
            return Ok(into_candidate(python));
        }
        if fallback.is_none() && supported.accepts(&python) {
            fallback = Some(python);
        }
    }
    match fallback {
        Some(python) => Ok(into_candidate(python)),
        None => Err(NuitkaRsError::NoSuitablePython {
            required: to_semver(MINIMUM),
            available: available.join("、"),
        }),
    }
}

pub(crate) fn is_recommended(candidate: &PythonCandidate) -> bool {
    candidate.version >= to_semver(RECOMMENDED)
}

fn parse_line(line: &str) -> Option<(u8, u8)> {
    let (major, minor) = line.trim().split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

fn into_candidate(python: Python) -> PythonCandidate {
    PythonCandidate {
        path: python.executable,
        version: to_semver(python.version),
        source: source_kind(python.source),
    }
}

fn source_kind(source: Source) -> SourceKind {
    match source {
        Source::Path => SourceKind::PathEnv,
        Source::Registry => SourceKind::Registry,
        Source::KnownDirectory => SourceKind::KnownDirectory,
        Source::Explicit => SourceKind::Explicit,
    }
}

fn to_semver(version: PyVersion) -> Version {
    let pre = if version.release.is_prerelease() {
        semver::Prerelease::new(&format!("{}{}", version.release.suffix(), version.serial))
            .unwrap_or(semver::Prerelease::EMPTY)
    } else {
        semver::Prerelease::EMPTY
    };
    Version {
        major: version.major as u64,
        minor: version.minor as u64,
        patch: version.micro as u64,
        pre,
        build: semver::BuildMetadata::EMPTY,
    }
}
