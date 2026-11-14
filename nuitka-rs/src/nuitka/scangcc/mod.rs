use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::python_discovery::{PyVersion, run_capture};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    NuitkaCache,
    Path,
    KnownDir,
}

impl Source {
    pub fn tag(self) -> &'static str {
        match self {
            Self::NuitkaCache => "Nuitka 缓存",
            Self::Path => "PATH",
            Self::KnownDir => "常见目录",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ready,
    Shadowed,
    Conflicts,
    Ignored,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ready => "符合",
            Self::Shadowed => "符合，但会被 PATH 上的 gcc 抢先",
            Self::Conflicts => "会导致 Nuitka 直接报错",
            Self::Ignored => "非 Nuitka 下载，会被忽略",
        }
    }

    pub fn is_usable(self) -> bool {
        matches!(self, Self::Ready)
    }
}

#[derive(Debug, Clone)]
pub struct Gcc {
    pub version: PyVersion,
    pub path: PathBuf,
    pub source: Source,
    pub shadowed: bool,
}

impl Gcc {
    pub fn is_pinned(&self) -> bool {
        self.source == Source::NuitkaCache
    }

    pub fn status(&self) -> Status {
        match self.source {
            Source::NuitkaCache if self.shadowed => Status::Shadowed,
            Source::NuitkaCache => Status::Ready,
            Source::Path => Status::Conflicts,
            Source::KnownDir => Status::Ignored,
        }
    }
}

impl fmt::Display for Gcc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "gcc {} ({})", self.version, self.source.tag())
    }
}

pub fn best(items: &[Gcc]) -> Option<&Gcc> {
    items.iter().find(|item| item.status().is_usable())
}

pub fn report(items: &[Gcc]) -> String {
    let mut out = String::new();
    for item in items {
        out.push_str(&format!("  {item}  {}\n", item.status().label()));
        out.push_str(&format!("     {}\n", item.path.display()));
    }
    out
}

pub fn scan() -> Vec<Gcc> {
    let mut out = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    for (path, source) in candidates() {
        if !path.is_file() {
            continue;
        }
        let identity = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if seen.contains(&identity) {
            continue;
        }
        let Some(version) = read_version(&path) else {
            continue;
        };
        seen.push(identity);
        out.push(Gcc {
            version,
            path,
            source,
            shadowed: false,
        });
    }
    let shadowed = out.iter().any(|item| item.source == Source::Path);
    for item in &mut out {
        item.shadowed = shadowed;
    }
    out.sort_by(|a, b| {
        b.is_pinned()
            .cmp(&a.is_pinned())
            .then_with(|| b.version.cmp(&a.version))
    });
    out
}

fn read_version(executable: &Path) -> Option<PyVersion> {
    let (success, stdout) = run_capture(executable, &["--version"], Duration::from_secs(5))?;
    if !success {
        return None;
    }
    let text = String::from_utf8_lossy(&stdout);
    parse_banner(text.lines().next()?)
}

fn parse_banner(banner: &str) -> Option<PyVersion> {
    banner
        .split_whitespace()
        .rev()
        .map(|token| token.trim_matches(|c: char| !(c.is_ascii_digit() || c == '.')))
        .find(|token| token.contains('.') && token.chars().all(|c| c.is_ascii_digit() || c == '.'))
        .and_then(PyVersion::parse)
}

fn candidates() -> Vec<(PathBuf, Source)> {
    let mut out = Vec::new();
    if let Some(local) = env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        // 旧版 Nuitka 的缓存位置
        out.push((
            local
                .join("Nuitka")
                .join("winlibs")
                .join("mingw64")
                .join("bin")
                .join("gcc.exe"),
            Source::NuitkaCache,
        ));
        // 新版 Nuitka 把 winlibs 解压在 Cache\downloads\gcc\x86_64\<版本> 下
        let cache = local
            .join("Nuitka")
            .join("Nuitka")
            .join("Cache")
            .join("downloads")
            .join("gcc")
            .join("x86_64");
        if let Ok(entries) = fs::read_dir(cache) {
            for entry in entries.flatten() {
                out.push((
                    entry
                        .path()
                        .join("mingw64")
                        .join("bin")
                        .join("gcc.exe"),
                    Source::NuitkaCache,
                ));
            }
        }
    }
    if let Some(path) = env::var_os("PATH") {
        for directory in env::split_paths(&path) {
            out.push((directory.join("gcc.exe"), Source::Path));
        }
    }
    for path in known_dirs() {
        out.push((path, Source::KnownDir));
    }
    out
}

fn known_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(drive) = env::var_os("SystemDrive") {
        let root = PathBuf::from(drive);
        out.push(root.join("mingw64").join("bin").join("gcc.exe"));
        out.push(
            root.join("msys64")
                .join("mingw64")
                .join("bin")
                .join("gcc.exe"),
        );
        out.push(
            root.join("msys64")
                .join("ucrt64")
                .join("bin")
                .join("gcc.exe"),
        );
    }
    if let Some(programs) = env::var_os("ProgramFiles").map(PathBuf::from) {
        out.push(programs.join("mingw64").join("bin").join("gcc.exe"));
        out.push(programs.join("mingw32").join("bin").join("gcc.exe"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(source: Source, shadowed: bool) -> Gcc {
        Gcc {
            version: PyVersion::new(13, 2, 0),
            path: PathBuf::from(r"C:\gcc.exe"),
            source,
            shadowed,
        }
    }

    #[test]
    fn parses_winlibs_banner() {
        let line = "gcc.exe (MinGW-W64 x86_64-ucrt-posix-seh, built by Brecht Sanders, r8) 13.2.0";
        assert_eq!(parse_banner(line), Some(PyVersion::new(13, 2, 0)));
    }

    #[test]
    fn parses_mingw_builds_banner() {
        let line = "gcc.exe (x86_64-win32-seh-rev1, Built by MinGW-Builds project) 16.1.0";
        assert_eq!(parse_banner(line), Some(PyVersion::new(16, 1, 0)));
    }

    #[test]
    fn rejects_banner_without_version() {
        assert!(parse_banner("gcc.exe (MinGW-W64)").is_none());
        assert!(parse_banner("").is_none());
    }

    #[test]
    fn status_separates_pinned_shadowed_and_foreign() {
        assert_eq!(sample(Source::NuitkaCache, false).status(), Status::Ready);
        assert_eq!(sample(Source::NuitkaCache, true).status(), Status::Shadowed);
        assert_eq!(sample(Source::Path, true).status(), Status::Conflicts);
        assert_eq!(sample(Source::KnownDir, false).status(), Status::Ignored);
    }

    #[test]
    fn best_requires_a_pinned_gcc_without_shadowing() {
        let ready = vec![sample(Source::NuitkaCache, false)];
        assert!(best(&ready).is_some());
        let shadowed = vec![
            sample(Source::NuitkaCache, true),
            sample(Source::Path, true),
        ];
        assert!(best(&shadowed).is_none());
    }

    #[test]
    fn candidates_include_the_nuitka_cache() {
        let found = candidates();
        assert!(
            found
                .iter()
                .any(|(_, source)| *source == Source::NuitkaCache)
        );
    }

    #[test]
    fn scanned_paths_exist_and_versions_parse() {
        for gcc in scan() {
            assert!(gcc.path.is_file());
            assert!(gcc.version.major > 0);
        }
    }
}
