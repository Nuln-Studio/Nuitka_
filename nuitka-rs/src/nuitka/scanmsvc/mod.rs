use std::cmp::Ordering;
use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::python_discovery::PyVersion;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Toolset {
    pub major: u16,
    pub minor: u16,
    pub build: u32,
}

impl Toolset {
    pub const fn new(major: u16, minor: u16, build: u32) -> Self {
        Self {
            major,
            minor,
            build,
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let mut parts = raw.split('.');
        let major: u16 = parts.next()?.parse().ok()?;
        let minor: u16 = parts.next()?.parse().ok()?;
        let build: u32 = parts.next().unwrap_or("0").parse().ok()?;
        Some(Self {
            major,
            minor,
            build,
        })
    }
}

impl Ord for Toolset {
    fn cmp(&self, other: &Self) -> Ordering {
        self.major
            .cmp(&other.major)
            .then_with(|| self.minor.cmp(&other.minor))
            .then_with(|| self.build.cmp(&other.build))
    }
}

impl PartialOrd for Toolset {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Toolset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.build)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ready,
    NeedEnvironment,
    MissingSdk,
    TooOld,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ready => "符合",
            Self::NeedEnvironment => "符合，但当前命令行没有 VC 环境",
            Self::MissingSdk => "缺少 Windows SDK",
            Self::TooOld => "版本过低",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::NeedEnvironment => "（需在 VS 开发人员命令提示符中运行）",
            _ => "",
        }
    }

    pub fn is_usable(self) -> bool {
        matches!(self, Self::Ready | Self::NeedEnvironment)
    }
}

#[derive(Debug, Clone)]
pub struct Msvc {
    pub toolset: Toolset,
    pub install: String,
    pub host: String,
    pub path: PathBuf,
    pub sdk: bool,
    pub env_ready: bool,
}

impl Msvc {
    pub fn supports(&self, python: PyVersion) -> bool {
        self.toolset >= required(python)
    }

    pub fn status(&self, python: PyVersion) -> Status {
        if !self.supports(python) {
            return Status::TooOld;
        }
        if !self.sdk {
            return Status::MissingSdk;
        }
        if !self.env_ready {
            return Status::NeedEnvironment;
        }
        Status::Ready
    }
}

impl fmt::Display for Msvc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MSVC {} ({}, {})", self.toolset, self.install, self.host)
    }
}

pub fn required(python: PyVersion) -> Toolset {
    if python >= PyVersion::new(3, 11, 0) {
        Toolset::new(14, 3, 0)
    } else {
        Toolset::new(14, 0, 0)
    }
}

pub fn best(items: &[Msvc], python: PyVersion) -> Option<&Msvc> {
    items.iter().find(|item| item.status(python).is_usable())
}

pub fn report(items: &[Msvc], python: Option<PyVersion>) -> String {
    let mut out = String::new();
    for item in items {
        match python {
            Some(python) => out.push_str(&format!("  {item}  {}\n", item.status(python).label())),
            None => out.push_str(&format!("  {item}\n")),
        }
        out.push_str(&format!("     {}\n", item.path.display()));
    }
    out
}

pub fn scan() -> Vec<Msvc> {
    let sdk = windows_sdk();
    let env_ready = environment_ready();
    let mut out = Vec::new();
    let mut seen: Vec<Toolset> = Vec::new();
    for root in installs() {
        collect(&root, sdk, env_ready, &mut out, &mut seen);
    }
    out.sort_by_key(|item| std::cmp::Reverse(item.toolset));
    out
}

pub fn environment_ready() -> bool {
    for name in ["VCINSTALLDIR", "VSCMD_VER", "VSCMD_ARG_TGT_ARCH"] {
        if env::var_os(name).is_some_and(|value| !value.is_empty()) {
            return true;
        }
    }
    env::var_os("PATH").is_some_and(|path| {
        env::split_paths(&path).any(|directory| directory.join("cl.exe").is_file())
    })
}

fn installs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(where_exe) = vswhere()
        && let Ok(output) = Command::new(where_exe)
            .args([
                "-products",
                "*",
                "-requires",
                "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                "-property",
                "installationPath",
            ])
            .output()
    {
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let line = line.trim();
            if !line.is_empty() {
                out.push(PathBuf::from(line));
            }
        }
    }
    for name in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(base) = env::var_os(name).map(PathBuf::from) {
            walk(&base.join("Microsoft Visual Studio"), &mut out);
        }
    }
    out
}

fn vswhere() -> Option<PathBuf> {
    let base = env::var_os("ProgramFiles(x86)").map(PathBuf::from)?;
    let candidate = base
        .join("Microsoft Visual Studio")
        .join("Installer")
        .join("vswhere.exe");
    candidate.is_file().then_some(candidate)
}

fn walk(base: &Path, out: &mut Vec<PathBuf>) {
    let Ok(levels) = fs::read_dir(base) else {
        return;
    };
    for level in levels.flatten() {
        let Ok(editions) = fs::read_dir(level.path()) else {
            continue;
        };
        for edition in editions.flatten() {
            let root = edition.path();
            if root.join("VC").join("Tools").join("MSVC").is_dir() {
                out.push(root);
            }
        }
    }
}

fn collect(root: &Path, sdk: bool, env_ready: bool, out: &mut Vec<Msvc>, seen: &mut Vec<Toolset>) {
    let Ok(entries) = fs::read_dir(root.join("VC").join("Tools").join("MSVC")) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(toolset) = Toolset::parse(&name) else {
            continue;
        };
        if seen.contains(&toolset) {
            continue;
        }
        let Some((path, host)) = find_cl(&entry.path()) else {
            continue;
        };
        seen.push(toolset);
        out.push(Msvc {
            toolset,
            install: describe(root),
            host,
            path,
            sdk,
            env_ready,
        });
    }
}

fn find_cl(toolset: &Path) -> Option<(PathBuf, String)> {
    for host in ["Hostx64", "Hostx86"] {
        let target = if host == "Hostx64" { "x64" } else { "x86" };
        let exe = toolset.join("bin").join(host).join(target).join("cl.exe");
        if exe.is_file() {
            return Some((exe, format!("{host}/{target}")));
        }
    }
    None
}

fn describe(root: &Path) -> String {
    let mut parts: Vec<String> = root
        .components()
        .rev()
        .take(2)
        .map(|part| part.as_os_str().to_string_lossy().to_string())
        .collect();
    parts.reverse();
    parts.join(" ")
}

fn windows_sdk() -> bool {
    for name in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(base) = env::var_os(name).map(PathBuf::from)
            && base
                .join("Windows Kits")
                .join("10")
                .join("Include")
                .is_dir()
        {
            return true;
        }
    }
    false
}

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

    fn sample(toolset: Toolset, sdk: bool, env_ready: bool) -> Msvc {
        Msvc {
            toolset,
            install: "18 BuildTools".to_string(),
            host: "Hostx64/x64".to_string(),
            path: PathBuf::from(r"C:\cl.exe"),
            sdk,
            env_ready,
        }
    }

    #[test]
    fn parses_toolset_directory_name() {
        assert_eq!(
            Toolset::parse("14.50.35717"),
            Some(Toolset::new(14, 50, 35717))
        );
        assert_eq!(Toolset::parse("14.29"), Some(Toolset::new(14, 29, 0)));
    }

    #[test]
    fn keeps_full_build_number() {
        let toolset = Toolset::new(14, 50, 35717);
        assert_eq!(toolset.to_string(), "14.50.35717");
        assert_eq!(Toolset::parse(&toolset.to_string()), Some(toolset));
    }

    #[test]
    fn rejects_non_toolset_names() {
        assert!(Toolset::parse("arm64").is_none());
        assert!(Toolset::parse("").is_none());
    }

    #[test]
    fn orders_by_build_not_just_minor() {
        assert!(Toolset::new(14, 50, 35717) > Toolset::new(14, 50, 30000));
        assert!(Toolset::new(14, 50, 0) > Toolset::new(14, 29, 30133));
    }

    #[test]
    fn requirement_jumps_for_python_311() {
        assert_eq!(required(PyVersion::new(3, 10, 9)), Toolset::new(14, 0, 0));
        assert_eq!(required(PyVersion::new(3, 11, 0)), Toolset::new(14, 3, 0));
        assert_eq!(required(PyVersion::new(3, 12, 4)), Toolset::new(14, 3, 0));
    }

    #[test]
    fn status_reports_the_first_blocking_reason() {
        let python = PyVersion::new(3, 12, 0);
        assert_eq!(
            sample(Toolset::new(14, 2, 0), true, true).status(python),
            Status::TooOld
        );
        assert_eq!(
            sample(Toolset::new(14, 50, 0), false, true).status(python),
            Status::MissingSdk
        );
        assert_eq!(
            sample(Toolset::new(14, 50, 0), true, false).status(python),
            Status::NeedEnvironment
        );
        assert_eq!(
            sample(Toolset::new(14, 50, 0), true, true).status(python),
            Status::Ready
        );
    }

    #[test]
    fn best_skips_unusable_toolsets() {
        let python = PyVersion::new(3, 12, 0);
        let items = vec![
            sample(Toolset::new(14, 50, 0), false, true),
            sample(Toolset::new(14, 40, 0), true, true),
        ];
        let chosen = best(&items, python).expect("candidate");
        assert_eq!(chosen.toolset, Toolset::new(14, 40, 0));
    }

    #[test]
    fn collects_toolsets_from_a_real_install_tree() {
        let root = scratch("msvc-install");
        for version in ["14.50.35717", "14.29.30133"] {
            touch(
                &root
                    .join("VC")
                    .join("Tools")
                    .join("MSVC")
                    .join(version)
                    .join("bin")
                    .join("Hostx64")
                    .join("x64")
                    .join("cl.exe"),
            );
        }
        let mut out = Vec::new();
        let mut seen = Vec::new();
        collect(&root, true, false, &mut out, &mut seen);
        assert_eq!(out.len(), 2);
        assert!(
            out.iter()
                .any(|item| item.toolset == Toolset::new(14, 50, 35717))
        );
        assert!(out.iter().all(|item| item.sdk && !item.env_ready));
        assert!(out.iter().all(|item| item.path.is_file()));
    }

    #[test]
    fn skips_toolsets_without_cl() {
        let root = scratch("msvc-no-cl");
        fs::create_dir_all(
            root.join("VC")
                .join("Tools")
                .join("MSVC")
                .join("14.40.33807"),
        )
        .expect("tree");
        let mut out = Vec::new();
        let mut seen = Vec::new();
        collect(&root, true, true, &mut out, &mut seen);
        assert!(out.is_empty());
    }

    #[test]
    fn describes_install_from_path() {
        let text = describe(Path::new(
            r"C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools",
        ));
        assert_eq!(text, "18 BuildTools");
    }
}
