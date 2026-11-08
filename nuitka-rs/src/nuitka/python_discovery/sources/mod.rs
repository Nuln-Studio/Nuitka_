use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use super::util::{expand_env, is_stub, normalize, strip_quotes};

#[cfg(windows)]
mod registry;

#[cfg(windows)]
pub use registry::RegistrySource;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    Path,
    Registry,
    KnownDirectory,
    Explicit,
}

impl Source {
    pub fn tag(self) -> &'static str {
        match self {
            Self::Path => "PATH",
            Self::Registry => "Registry",
            Self::KnownDirectory => "KnownDir",
            Self::Explicit => "Explicit",
        }
    }
}

pub trait Provider: Sync {
    fn source(&self) -> Source;
    fn collect(&self, out: &mut Vec<PathBuf>);
}

#[derive(Debug)]
pub struct Candidate {
    pub executable: PathBuf,
    pub source: Source,
}

pub fn harvest(providers: &[&dyn Provider]) -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut buffer: Vec<PathBuf> = Vec::new();
    for provider in providers {
        let source = provider.source();
        buffer.clear();
        provider.collect(&mut buffer);
        for path in buffer.drain(..) {
            if !path.is_file() || is_stub(&path) {
                continue;
            }
            if !seen.insert(normalize(&path)) {
                continue;
            }
            out.push(Candidate {
                executable: path,
                source,
            });
        }
    }
    out
}

pub struct PathSource;

#[cfg(windows)]
const EXE_NAMES: [&str; 2] = ["python.exe", "python3.exe"];
#[cfg(not(windows))]
const EXE_NAMES: [&str; 2] = ["python3", "python"];

impl Provider for PathSource {
    fn source(&self) -> Source {
        Source::Path
    }

    fn collect(&self, out: &mut Vec<PathBuf>) {
        let Some(raw) = env::var_os("PATH") else {
            return;
        };
        let raw = raw.to_string_lossy();
        let separator = if cfg!(windows) { ';' } else { ':' };
        for part in raw.split(separator) {
            let part = strip_quotes(part);
            if part.is_empty() {
                continue;
            }
            let directory = expand_env(part);
            for name in EXE_NAMES {
                let candidate = Path::new(directory.as_ref()).join(name);
                if candidate.is_file() {
                    out.push(candidate);
                }
            }
        }
    }
}

pub struct KnownDirectorySource;

impl Provider for KnownDirectorySource {
    fn source(&self) -> Source {
        Source::KnownDirectory
    }

    fn collect(&self, out: &mut Vec<PathBuf>) {
        if let Some(local) = var_path("LOCALAPPDATA") {
            push_children(out, &local.join("Programs").join("Python"));
            push_prefixed_children(out, &local.join("Python"), "pythoncore-");
            for name in ["miniconda3", "anaconda3"] {
                push_exe(out, &local.join(name));
                push_children(out, &local.join(name).join("envs"));
            }
            for name in ["miniconda3", "anaconda3"] {
                push_exe(out, &local.join("Programs").join(name));
            }
        }
        if let Some(program_files) = var_path("ProgramFiles") {
            push_prefixed_children(out, &program_files, "Python");
        }
        if let Some(program_files) = var_path("ProgramFiles(x86)") {
            push_prefixed_children(out, &program_files, "Python");
        }
        if let Some(drive) = var_path("SystemDrive") {
            push_prefixed_children(out, &drive, "Python");
        }
        if let Some(profile) = var_path("USERPROFILE") {
            for name in ["miniconda3", "anaconda3", "miniforge3", "mambaforge"] {
                let root = profile.join(name);
                push_exe(out, &root);
                push_children(out, &root.join("envs"));
            }
            push_exe(out, &profile.join("micromamba").join("root"));
            push_exe(
                out,
                &profile
                    .join("scoop")
                    .join("apps")
                    .join("python")
                    .join("current"),
            );
            push_children(
                out,
                &profile.join(".pyenv").join("pyenv-win").join("versions"),
            );
        }
        if let Some(program_data) = var_path("ProgramData") {
            for name in ["miniconda3", "anaconda3"] {
                let root = program_data.join(name);
                push_exe(out, &root);
                push_children(out, &root.join("envs"));
            }
        }
        if let Some(roaming) = var_path("APPDATA") {
            push_children(out, &roaming.join("uv").join("python"));
        }
        for name in ["VIRTUAL_ENV", "CONDA_PREFIX"] {
            if let Some(root) = var_path(name) {
                push_exe(out, &root);
            }
        }
    }
}

fn var_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

fn push_exe(out: &mut Vec<PathBuf>, root: &Path) {
    let mut candidate = root.to_path_buf();
    candidate.push(EXE_NAME);
    push_if_file(out, candidate);
}

fn push_children(out: &mut Vec<PathBuf>, root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let mut candidate = entry.path();
        candidate.push(EXE_NAME);
        push_if_file(out, candidate);
    }
}

fn push_prefixed_children(out: &mut Vec<PathBuf>, root: &Path, prefix: &str) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .as_encoded_bytes()
            .starts_with(prefix.as_bytes())
        {
            continue;
        }
        let mut candidate = entry.path();
        candidate.push(EXE_NAME);
        push_if_file(out, candidate);
    }
}

fn push_if_file(out: &mut Vec<PathBuf>, path: PathBuf) {
    if path.is_file() {
        out.push(path);
    }
}

#[cfg(windows)]
const EXE_NAME: &str = "python.exe";
#[cfg(not(windows))]
const EXE_NAME: &str = "python3";

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join(name);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("scratch");
        root
    }

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent");
        }
        fs::write(path, b"").expect("file");
    }

    #[test]
    fn splits_plain_path_entries() {
        let entries: Vec<&str> = r"C:\A;C:\B".split(';').collect();
        assert_eq!(entries, vec![r"C:\A", r"C:\B"]);
    }

    #[test]
    fn finds_direct_layout() {
        let root = scratch("direct");
        touch(&root.join(EXE_NAME));
        let mut out = Vec::new();
        push_exe(&mut out, &root);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn finds_children() {
        let root = scratch("children");
        touch(&root.join("Python312").join(EXE_NAME));
        touch(&root.join("Python311").join(EXE_NAME));
        touch(&root.join("README.txt"));
        let mut out = Vec::new();
        push_children(&mut out, &root);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn finds_prefixed_children() {
        let root = scratch("prefixed");
        touch(&root.join("pythoncore-3.14-64").join(EXE_NAME));
        touch(&root.join("pythoncore-3.13-64").join(EXE_NAME));
        touch(&root.join("python-3.12").join(EXE_NAME));
        touch(&root.join("bin").join(EXE_NAME));
        let mut out = Vec::new();
        push_prefixed_children(&mut out, &root, "pythoncore-");
        assert_eq!(out.len(), 2);
        assert!(
            out.iter()
                .all(|path| path.to_string_lossy().contains("pythoncore-"))
        );
    }

    #[test]
    fn missing_root_is_not_an_error() {
        let mut out = Vec::new();
        push_children(&mut out, Path::new(r"Z:\definitely\missing"));
        assert!(out.is_empty());
    }

    #[test]
    fn harvest_drops_missing_and_duplicates() {
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

        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let real = manifest.join("Cargo.toml");
        let first = Fixed {
            source: Source::Path,
            paths: vec![real.clone(), manifest.join("missing").join(EXE_NAME)],
        };
        let second = Fixed {
            source: Source::Registry,
            paths: vec![real],
        };
        let providers: [&dyn Provider; 2] = [&first, &second];
        let found = harvest(&providers);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].source, Source::Path);
    }
}
