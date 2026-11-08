use semver::Version;
use thiserror::Error;

use std::path::PathBuf;

#[derive(Debug, Error)]
pub enum NuitkaRsError {
    #[error("没有找到任何Python解释器")]
    PythonNotFound,
    #[error("没有找到满足最低版本要求({required})的Python，可用版本：{available}")]
    NoSuitablePython {
        required: Version,
        available: String,
    },
}

pub type Result<T> = std::result::Result<T, NuitkaRsError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    PathEnv,
    Registry,
    KnownDirectory,
    Explicit,
}

impl std::fmt::Display for SourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::PathEnv => "PATH",
            Self::Registry => "Registry",
            Self::KnownDirectory => "KnownDir",
            Self::Explicit => "Explicit",
        };
        f.write_str(text)
    }
}

#[derive(Debug, Clone)]
pub struct PythonCandidate {
    pub path: PathBuf,
    pub version: Version,
    pub source: SourceKind,
}
