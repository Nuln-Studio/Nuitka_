use std::path::{Path, PathBuf};

use crate::nuitka::python_discovery::PyVersion;
use crate::nuitka::{fetch, lock, log, paths, scannuitka, selfcheck, toolchain};

pub(crate) const PYTHON_CHOICES: &[(&str, &str, bool)] = &[
    ("3.11", "Python 3.11", false),
    ("3.12", "Python 3.12", false),
    ("3.13", "Python 3.13", false),
    ("3.14", "Python 3.14", true),
];
const DEFAULT_PYTHON_LINE: &str = "3.12";
const NUITKA_LATEST: &str = "4.3.5";
const MIRROR: &str = "https://pypi.tuna.tsinghua.edu.cn/simple";
const MIRROR_BASE: &str =
    "https://mirror.nju.edu.cn/github-release/astral-sh/python-build-standalone/LatestRelease/";
const RELEASE_API: &str =
    "https://api.github.com/repos/astral-sh/python-build-standalone/releases/latest";
const WINLIBS_FALLBACK_SIZE: u64 = 267_135_426;
const WINLIBS_DISK_SIZE: u64 = 773_815_574;
const MIB: u64 = 1024 * 1024;

pub(crate) fn parse_python_line(line: &str) -> Option<PyVersion> {
    let (major, minor) = line.trim().split_once('.')?;
    let major: u8 = major.parse().ok()?;
    let minor: u8 = minor.parse().ok()?;
    PYTHON_CHOICES
        .iter()
        .any(|(value, _, _)| *value == line.trim())
        .then(|| PyVersion::new(major, minor, 0))
}

pub(crate) fn selected_python_line() -> String {
    paths::selected_python()
        .filter(|line| parse_python_line(line).is_some())
        .unwrap_or_else(|| DEFAULT_PYTHON_LINE.to_string())
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct PythonChoice {
    pub version: String,
    pub label: String,
    pub support: String,
    pub recommended: bool,
    pub nuitka: String,
    pub compiler: String,
    pub warning: String,
    pub can_prepare: bool,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct PythonOptions {
    pub selected: String,
    pub versions: Vec<PythonChoice>,
}

pub(crate) fn python_options() -> PythonOptions {
    let msvcs = crate::nuitka::scanmsvc::scan();
    let gccs = crate::nuitka::scangcc::scan();
    let versions = PYTHON_CHOICES
        .iter()
        .map(|(line, label, recommended)| {
            let version = parse_python_line(line).unwrap_or(PyVersion::new(3, 12, 0));
            let msvc = crate::nuitka::scanmsvc::best(&msvcs, version);
            let gcc = crate::nuitka::scangcc::best(&gccs);
            let (compiler, warning) = if let Some(item) = msvc {
                (format!("MSVC {}", item.toolset), String::new())
            } else if let Some(item) = gcc {
                (
                    format!("gcc {}", item.version),
                    "本机 MSVC 不满足该 Python，准备时使用 Nuitka 自带的 winlibs。"
                        .to_string(),
                )
            } else {
                (
                    "winlibs（准备时下载）".to_string(),
                    "本机没有匹配的 MSVC/GCC，准备时下载 winlibs 编译器。".to_string(),
                )
            };
            PythonChoice {
                version: line.to_string(),
                label: label.to_string(),
                support: "完全支持".to_string(),
                recommended: *recommended,
                nuitka: format!("Nuitka {NUITKA_LATEST}"),
                compiler,
                warning,
                can_prepare: true,
            }
        })
        .collect();

    PythonOptions {
        selected: selected_python_line(),
        versions,
    }
}

pub(crate) struct Ready {
    pub nuitka: String,
    pub verified: bool,
    pub detail: String,
}

/// 准备环境期间推给界面的结构化进度。
#[derive(Clone, serde::Serialize)]
pub(crate) struct PrepareEvent {
    pub kind: &'static str,
    pub stage: String,
    pub detail: String,
    pub source: String,
    pub bytes: u64,
    pub total: u64,
    pub speed: f64,
}

impl PrepareEvent {
    pub(crate) fn stage(stage: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            kind: "stage",
            stage: stage.into(),
            detail: detail.into(),
            source: String::new(),
            bytes: 0,
            total: 0,
            speed: 0.0,
        }
    }

    pub(crate) fn download(
        stage: impl Into<String>,
        detail: impl Into<String>,
        source: impl Into<String>,
        bytes: u64,
        total: u64,
        speed: f64,
    ) -> Self {
        Self {
            kind: "download",
            stage: stage.into(),
            detail: detail.into(),
            source: source.into(),
            bytes,
            total,
            speed,
        }
    }

    pub(crate) fn log(line: impl Into<String>) -> Self {
        Self {
            kind: "log",
            stage: String::new(),
            detail: line.into(),
            source: String::new(),
            bytes: 0,
            total: 0,
            speed: 0.0,
        }
    }
}

/// 下载前给用户看的估算；不在 plan 阶段真正下载。
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct EnvPlan {
    pub title: String,
    pub python: String,
    pub items: Vec<String>,
    pub download_bytes: u64,
    pub disk_bytes: u64,
    pub full_seconds: u64,
    pub measured_seconds: u64,
    pub eta_seconds: u64,
}

struct WinlibsInfo {
    source: String,
    mirror: String,
    archive: PathBuf,
    total: u64,
}

pub(crate) fn plan(base: &Path, python_line: &str) -> EnvPlan {
    let python_version = parse_python_line(python_line).unwrap_or(PyVersion::new(3, 12, 0));
    let python_home = base.join("python").join(python_line);
    let python = python_home.join("python.exe");
    let has_python = python.is_file();
    let has_nuitka = has_python && fetch::run(&python, &["-m", "nuitka", "--version"]).is_ok();
    let tool = toolchain::resolve(python_version);
    let winlibs_total = if tool.is_none() {
        if has_python {
            winlibs_info(&python)
                .map(|info| {
                    let gcc_ready = info
                        .archive
                        .parent()
                        .map(|dir| dir.join("mingw64").join("bin").join("gcc.exe").is_file())
                        .unwrap_or(false);
                    let cached = gcc_ready
                        || (info.total > 0
                            && std::fs::metadata(&info.archive)
                                .map(|meta| meta.len() == info.total)
                                .unwrap_or(false));
                    if cached {
                        0
                    } else if info.total > 0 {
                        info.total
                    } else {
                        WINLIBS_FALLBACK_SIZE
                    }
                })
                .unwrap_or(WINLIBS_FALLBACK_SIZE)
        } else {
            WINLIBS_FALLBACK_SIZE
        }
    } else {
        0
    };

    let winlibs_needed = tool.is_none() && winlibs_total > 0;
    let mut items = Vec::new();
    let mut download_bytes = 0u64;
    let mut disk_bytes = 0u64;

    items.push(format!(
        "便携 Python  约 24 MB · {}",
        if has_python { "已就绪" } else { "需下载" }
    ));
    items.push(format!(
        "Nuitka 及依赖  约 30 MB · {}",
        if has_nuitka { "已就绪" } else { "需安装" }
    ));
    let compiler = if let Some(tool) = &tool {
        format!("{} · 已就绪", tool.label)
    } else if winlibs_needed {
        "winlibs · 需下载".to_string()
    } else {
        "winlibs · 已就绪".to_string()
    };
    items.push(format!("编译器  {compiler}"));

    if !has_python {
        download_bytes += 24 * MIB;
        disk_bytes += 144 * MIB;
    }
    if !has_nuitka {
        download_bytes += 30 * MIB;
        disk_bytes += 180 * MIB;
    }
    if winlibs_needed {
        download_bytes += winlibs_total;
        disk_bytes += WINLIBS_DISK_SIZE + winlibs_total;
    }

    // 开发者实测：网络好的情况下，从零建完整套约 13 分钟。
    // 再按本机 CPU 核数和本次缺少的组件做本地预估。
    let cores = std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(4);
    let factor = match cores {
        0..=2 => 1.25,
        3..=4 => 1.10,
        5..=8 => 1.00,
        _ => 0.90,
    };
    let measured_seconds = 13 * 60;
    let full_seconds = (measured_seconds as f64 * factor).round() as u64;
    let mut eta_seconds = full_seconds as i64;
    if has_python {
        eta_seconds -= (90.0 * factor).round() as i64;
    }
    if has_nuitka {
        eta_seconds -= (90.0 * factor).round() as i64;
    }
    if !winlibs_needed {
        eta_seconds -= (300.0 * factor).round() as i64;
    }
    let eta_seconds = eta_seconds.max(60) as u64;

    EnvPlan {
        title: "从零建立完整编译环境".to_string(),
        python: python_line.to_string(),
        items,
        download_bytes,
        disk_bytes,
        full_seconds,
        measured_seconds,
        eta_seconds,
    }
}

fn human_bytes(bytes: u64) -> String {
    if bytes >= 1024 * MIB {
        format!("{:.1} GB", bytes as f64 / (1024.0 * MIB as f64))
    } else if bytes >= MIB {
        format!("{:.0} MB", bytes as f64 / MIB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// 从已安装的 Nuitka 里读出 winlibs 的下载地址和缓存位置，避免把版本写死。
fn winlibs_info(python: &Path) -> Option<WinlibsInfo> {
    let script = r#"
import inspect, json, re
from nuitka.utils import Download
src = inspect.getsource(Download.getCachedDownloadedMinGW64)
m = re.search(r'https://github\.com/brechtsanders/winlibs_mingw/releases/download/[^"\']+\.zip', src)
print(json.dumps({"url": m.group(0) if m else "", "cache": Download.getDownloadCacheDir()}))
"#;
    let output = fetch::run(python, &["-c", script]).ok()?;
    let line = output
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with('{'))?;
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let source = value.get("url")?.as_str()?.to_string();
    let cache_dir = PathBuf::from(value.get("cache")?.as_str()?);
    if source.is_empty() {
        return None;
    }

    let file = source.rsplit('/').next()?.to_string();
    let tag = source.rsplit('/').nth(1)?.to_string();
    let archive = cache_dir.join("gcc").join("x86_64").join(&tag).join(&file);
    let mirror = format!("https://downloads.sourceforge.net/project/winlibs-mingw/{tag}/{file}");
    let total = match fetch::content_length(&mirror) {
        0 => fetch::content_length(&source),
        value => value,
    };

    Some(WinlibsInfo {
        source,
        mirror,
        archive,
        total,
    })
}

/// 先自己下 winlibs（SourceForge 镜像 + 断点续传），下完再让 Nuitka 解压。
/// 两个源都失败时直接报错，绝不再交给 Nuitka 内置下载器去卡 7 分钟。
fn ensure_compiler<F: FnMut(PrepareEvent)>(
    python: &Path,
    progress: &mut F,
) -> Result<String, String> {
    progress(PrepareEvent::stage(
        "检查编译器组件",
        "解析 winlibs 下载地址与缓存",
    ));
    let info = winlibs_info(python).ok_or_else(|| {
        "无法解析当前 Nuitka 的 winlibs 下载地址；请检查网络后重试".to_string()
    })?;
    let gcc = info
        .archive
        .parent()
        .map(|dir| dir.join("mingw64").join("bin").join("gcc.exe"))
        .unwrap_or_default();
    if gcc.is_file() {
        progress(PrepareEvent::log(format!(
            "复用已解压的 winlibs：{}",
            gcc.display()
        )));
        return fetch::run(python, &["-m", "nuitka", "--version"]);
    }
    let size = human_bytes(info.total);
    log::info(&format!("下载 winlibs：{}", info.mirror));
    progress(PrepareEvent::stage(
        "下载编译器组件",
        format!("winlibs · {size} · 优先使用 SourceForge 镜像"),
    ));
    progress(PrepareEvent::log(format!("下载源：{}", info.mirror)));

    if let Err(error) = download_winlibs(&info, progress) {
        let _ = std::fs::remove_file(&info.archive);
        log::error(&format!("winlibs 下载失败：{error}"));
        progress(PrepareEvent::log(format!("下载失败：{error}")));
        return Err(format!("编译器组件下载失败：{error}"));
    }

    progress(PrepareEvent::stage(
        "解压编译器组件",
        "winlibs · 首次约 30 秒",
    ));
    let dest = info.archive.parent().ok_or_else(|| {
        "winlibs 缓存路径不完整".to_string()
    })?;
    if let Err(error) = fetch::extract_zip(&info.archive, dest, |line| {
        progress(PrepareEvent::log(line))
    }) {
        let _ = std::fs::remove_file(&info.archive);
        log::error(&format!("winlibs 解压失败：{error}"));
        progress(PrepareEvent::log(format!("解压失败：{error}")));
        return Err(format!("编译器组件解压失败：{error}"));
    }
    progress(PrepareEvent::log("winlibs 解压完成"));

    if !gcc.is_file() {
        let _ = std::fs::remove_file(&info.archive);
        return Err("编译器组件解压后没有找到 gcc.exe，请重试下载".to_string());
    }
    log::info(&format!("winlibs 就绪：{}", gcc.display()));
    fetch::run(python, &["-m", "nuitka", "--version"])
}

fn download_winlibs<F: FnMut(PrepareEvent)>(
    info: &WinlibsInfo,
    progress: &mut F,
) -> Result<(), String> {
    if info.total > 0
        && std::fs::metadata(&info.archive)
            .map(|meta| meta.len() == info.total)
            .unwrap_or(false)
    {
        progress(PrepareEvent::log("复用已下载的 winlibs 压缩包"));
        return Ok(());
    }

    let mut last_error = "没有可用的下载源".to_string();
    for (url, label) in [
        (info.mirror.as_str(), "SourceForge 镜像"),
        (info.source.as_str(), "GitHub"),
    ] {
        progress(PrepareEvent::log(format!("尝试 {label}：{url}")));
        match fetch::download_to(url, &info.archive, info.total, |bytes, total, speed| {
            progress(PrepareEvent::download(
                "正在下载编译器组件",
                "winlibs",
                label,
                bytes,
                total,
                speed,
            ));
        }) {
            Ok(()) => {
                progress(PrepareEvent::log("编译器组件下载完成"));
                return Ok(());
            }
            Err(error) => {
                last_error = error.clone();
                progress(PrepareEvent::log(format!("{label} 失败：{error}")));
            }
        }
    }
    Err(last_error)
}

/// 准备便携 Python + Nuitka，最后真编一个程序验证这条链路是通的。
pub(crate) fn prepare<F: FnMut(PrepareEvent)>(
    base: &Path,
    python_line: &str,
    mut progress: F,
) -> Result<Ready, String> {
    let python_version =
        parse_python_line(python_line).ok_or_else(|| format!("不支持的 Python 版本：{python_line}"))?;
    // 同一个安装目录只允许一个流程在跑
    let _guard = lock::acquire(base)?;
    paths::set_base(base)?;

    let python_home = base.join("python").join(python_line);
    let python = python_home.join("python.exe");

    log::info(&format!("开始准备环境：{}", base.display()));

    if !python.is_file() {
        progress(PrepareEvent::stage(
            "解析便携 Python 下载地址",
            "优先国内镜像，失败后回退 GitHub",
        ));
        let archive = ensure_archive(base, python_line, &mut progress)?;
        progress(PrepareEvent::stage(
            "解压便携 Python",
            archive
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("便携 Python"),
        ));
        fetch::install(&archive, &python_home)?;
    }

    progress(PrepareEvent::stage(
        "检查 Python 版本",
        "执行 python --version",
    ));
    let version = fetch::run(&python, &["--version"])?;
    log::info(&format!("Python：{version}"));

    // 已经装好的还要确认它真的支持当前选的 Python；不支持就升级到最新版
    let nuitka_ready = fetch::run(&python, &["-m", "nuitka", "--version"]).is_ok();
    let nuitka_supports = nuitka_ready
        && scannuitka::supports(&python, python_version).unwrap_or(false);
    if nuitka_ready && nuitka_supports {
        progress(PrepareEvent::stage(
            "检测到已安装的 Nuitka，跳过安装",
            format!("Nuitka 已支持 Python {python_line}"),
        ));
    } else {
        if nuitka_ready {
            progress(PrepareEvent::stage(
                "升级 Nuitka",
                format!("当前 Nuitka 不适配 Python {python_line}，升级到最新版"),
            ));
        }
        progress(PrepareEvent::stage("安装 pip", "ensurepip · 自更新 pip"));
        if let Err(error) =
            fetch::run_streaming(&python, &["-m", "ensurepip", "--upgrade"], &[], |line| {
                progress(PrepareEvent::log(line))
            })
        {
            progress(PrepareEvent::log(format!("ensurepip：{error}")));
        }
        if let Err(error) = fetch::run_streaming(
            &python,
            &["-m", "pip", "install", "--upgrade", "pip", "-i", MIRROR],
            &[],
            |line| progress(PrepareEvent::log(line)),
        ) {
            progress(PrepareEvent::log(format!("pip 自更新：{error}")));
        }

        progress(PrepareEvent::stage(
            "安装 Nuitka（含 onefile 压缩组件）",
            "nuitka[onefile] · 清华 PyPI",
        ));
        fetch::run_streaming(
            &python,
            &[
                "-m",
                "pip",
                "install",
                "--upgrade",
                "nuitka[onefile]",
                "-i",
                MIRROR,
            ],
            &[],
            |line| progress(PrepareEvent::log(line)),
        )?;
    }

    ensure_zstandard(&python, &mut progress);

    progress(PrepareEvent::stage(
        "检测编译器",
        "优先复用已安装的 MSVC，缺失时才用 Nuitka winlibs",
    ));
    let mut tool = toolchain::resolve(python_version);
    let raw = if let Some(tool) = tool.as_ref() {
        progress(PrepareEvent::stage(
            "使用已有编译器，跳过下载",
            format!("{}（{}）", tool.label, tool.kind_label()),
        ));
        fetch::run(&python, &["-m", "nuitka", "--version"])?
    } else {
        let raw = ensure_compiler(&python, &mut progress)?;
        // winlibs 刚解压好，重新扫描一次，让自检/打包拿到 --mingw64 和 PATH
        tool = toolchain::resolve(python_version);
        if let Some(tool) = tool.as_ref() {
            progress(PrepareEvent::log(format!(
                "winlibs 编译器已就绪：{}",
                tool.label
            )));
        } else {
            progress(PrepareEvent::log(
                "winlibs 已解压，但扫描器没有在缓存里找到 gcc.exe",
            ));
        }
        raw
    };
    let summary = first_line(&raw).to_string();

    progress(PrepareEvent::stage("挑选编译器", ""));
    if let Some(tool) = &tool {
        log::info(&format!(
            "选中编译器：{}（{}）",
            tool.label,
            tool.kind_label()
        ));
    }

    let mut stage = |line: &str| progress(PrepareEvent::stage(line, ""));
    let outcome = selfcheck::run(&python, base, tool.as_ref(), &mut stage);
    drop(stage);

    // 自检是真实编译，失败时把 build.log 尾部一起放进“详情”，别只留一句失败
    if !outcome.ok
        && let Ok(text) = std::fs::read_to_string(base.join("selfcheck").join("build.log"))
    {
        let lines: Vec<&str> = text.lines().collect();
        let start = lines.len().saturating_sub(300);
        for line in &lines[start..] {
            progress(PrepareEvent::log(format!("[自检] {line}")));
        }
    }

    progress(PrepareEvent::stage(
        format!("环境就绪：{version} / Nuitka {summary}"),
        "可以进入主界面",
    ));
    log::info(&format!(
        "环境就绪：{version} / Nuitka {summary} / 自检{}",
        if outcome.ok { "通过" } else { "未通过" }
    ));

    Ok(Ready {
        nuitka: summary,
        verified: outcome.ok,
        detail: outcome.detail,
    })
}

/// 拿到一个校验过、可以解压的压缩包。
fn ensure_archive<F: FnMut(PrepareEvent)>(
    base: &Path,
    python_line: &str,
    progress: &mut F,
) -> Result<PathBuf, String> {
    let downloads = base.join("downloads");
    let candidates = candidates(python_line);
    if candidates.is_empty() {
        return Err("没有拿到可用的下载地址，请检查网络或 NUITKA_RS_PBS_URL".to_string());
    }

    let mut last_error = String::from("没有找到可用下载源");
    for (url, name) in candidates {
        let archive = downloads.join(&name);
        let expected = expected_hash(&url, &name);
        let source = host_of(&url);

        if archive.is_file() && reusable(&archive, expected.as_deref()) {
            log::info(&format!("复用已下载的压缩包：{}", archive.display()));
            progress(PrepareEvent::stage(
                "复用已下载的便携 Python",
                format!("{} · {}", name, source),
            ));
            progress(PrepareEvent::log(format!(
                "复用压缩包：{}",
                archive.display()
            )));
            return Ok(archive);
        }

        log::info(&format!("下载 Python：{url}"));
        progress(PrepareEvent::log(format!("Python 下载源：{url}")));
        progress(PrepareEvent::stage(
            "下载便携 Python",
            format!("{} · 约 24 MB · 支持断点续传", name),
        ));
        let total = fetch::content_length(&url);
        progress(PrepareEvent::download(
            "正在下载便携 Python",
            &name,
            &source,
            0,
            total,
            0.0,
        ));
        match fetch::download_checked_with(
            &url,
            &archive,
            expected.as_deref(),
            total,
            |bytes, total, speed| {
                progress(PrepareEvent::download(
                    "正在下载便携 Python",
                    &name,
                    &source,
                    bytes,
                    total,
                    speed,
                ));
            },
        ) {
            Ok(()) => {
                progress(PrepareEvent::log("Python 压缩包下载完成"));
                return Ok(archive);
            }
            Err(error) => {
                log::error(&format!("下载失败（{url}）：{error}"));
                progress(PrepareEvent::stage(
                    "下载源不可用，尝试下一个",
                    format!("{} · {}", source, error),
                ));
                progress(PrepareEvent::log(format!("下载失败：{error}")));
                last_error = error;
            }
        }
    }

    Err(last_error)
}

fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
        .to_string()
}

fn reusable(archive: &Path, expected: Option<&str>) -> bool {
    if let Some(expected) = expected {
        match fetch::sha256(archive) {
            Ok(actual) if actual.eq_ignore_ascii_case(expected) => return true,
            _ => return false,
        }
    }
    fetch::verify_archive(archive).is_ok()
}

/// 优先找官方给的 sha256；没有就只能靠压缩包完整性兜底。
fn expected_hash(url: &str, name: &str) -> Option<String> {
    if let Some(text) = fetch::fetch_text(&format!("{url}.sha256"))
        && let Some(hash) = fetch::pick_hash(&text)
    {
        return Some(hash);
    }

    let directory = url.rsplit_once('/')?.0;
    let sums = fetch::fetch_text(&format!("{directory}/SHA256SUMS"))?;
    fetch::hash_for_file(&sums, name)
}

/// 环境变量直链 → 国内镜像 → GitHub API，按顺序尝试。
fn candidates(python_line: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();

    if let Ok(raw) = std::env::var("NUITKA_RS_PBS_URL") {
        let url = raw.trim().to_string();
        if !url.is_empty() {
            let name = url
                .rsplit('/')
                .next()
                .filter(|item| !item.is_empty())
                .map(|item| item.to_string());
            if let Some(name) = name {
                out.push((url, name));
            }
        }
    }

    if let Ok(pair) = mirror_asset(python_line) {
        out.push(pair);
    }
    if let Ok(pair) = github_asset(python_line) {
        out.push(pair);
    }

    out
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text).trim()
}

/// onefile 没有 zstandard 也能跑，只是不压缩、产物大一圈。装不上就算了。
fn ensure_zstandard<F: FnMut(PrepareEvent)>(python: &Path, progress: &mut F) {
    if fetch::run(python, &["-c", "import zstandard"]).is_ok() {
        return;
    }
    progress(PrepareEvent::stage(
        "补齐 onefile 压缩组件",
        "zstandard · 清华 PyPI",
    ));
    match fetch::run_streaming(
        python,
        &[
            "-m",
            "pip",
            "install",
            "--upgrade",
            "zstandard",
            "-i",
            MIRROR,
        ],
        &[],
        |line| progress(PrepareEvent::log(line)),
    ) {
        Ok(_) => log::info("zstandard 已安装"),
        Err(error) => {
            log::error(&format!("zstandard 安装失败，onefile 将不压缩：{error}"));
            progress(PrepareEvent::log(format!("zstandard 安装失败：{error}")));
        }
    }
}

fn mirror_asset(python_line: &str) -> Result<(String, String), String> {
    let base = std::env::var("NUITKA_RS_PBS_BASE").unwrap_or_else(|_| MIRROR_BASE.to_string());
    let html = curl_text(&base)?;
    pick_asset(&html, &base, python_line).ok_or_else(|| {
        format!("镜像目录里没找到 Python {python_line} 的 Windows 便携包")
    })
}

fn pick_asset(html: &str, base: &str, python_line: &str) -> Option<(String, String)> {
    let mut position = 0;
    let prefix = format!("cpython-{python_line}.");

    while let Some(found) = html[position..].find("href=\"") {
        let start = position + found + 6;
        let end = html[start..].find('"')?;
        let href = &html[start..start + end];
        position = start + end;

        let decoded = decode(href);
        let matched = decoded.starts_with(&prefix)
            && decoded.contains("x86_64-pc-windows-msvc")
            && decoded.contains("install_only_stripped.tar.gz");
        if matched {
            let url = if href.starts_with("http") {
                href.to_string()
            } else {
                format!("{base}{href}")
            };
            return Some((url, decoded));
        }
    }

    None
}

fn github_asset(python_line: &str) -> Result<(String, String), String> {
    let body = curl_text(RELEASE_API)?;
    let json: serde_json::Value =
        serde_json::from_str(&body).map_err(|error| format!("解析 GitHub 返回失败：{error}"))?;
    let assets = json
        .get("assets")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "GitHub 返回里没有 assets".to_string())?;

    for asset in assets {
        let Some(name) = asset.get("name").and_then(|value| value.as_str()) else {
            continue;
        };
        let matched = name.starts_with(&format!("cpython-{python_line}."))
            && name.contains("x86_64-pc-windows-msvc")
            && name.contains("install_only_stripped")
            && name.ends_with(".tar.gz");
        if matched {
            let url = asset
                .get("browser_download_url")
                .and_then(|value| value.as_str())
                .ok_or_else(|| "资源缺少下载地址".to_string())?;
            return Ok((url.to_string(), name.to_string()));
        }
    }

    Err("没有找到匹配的 python-build-standalone 压缩包".to_string())
}

fn curl_text(url: &str) -> Result<String, String> {
    let output = crate::util::command("curl.exe")
        .arg("-L")
        .arg("--fail")
        .arg("--silent")
        .arg("--show-error")
        .arg("--max-time")
        .arg("30")
        .arg(url)
        .output()
        .map_err(|error| format!("无法启动 curl.exe：{error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(format!(
            "访问 {url} 失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn decode(text: &str) -> String {
    text.replace("%2B", "+")
        .replace("%2b", "+")
        .replace("%20", " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "诊断用，会读本机 Nuitka 并访问镜像 HEAD"]
    fn probe_plan_and_winlibs() {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap()
            .join("Nuitka++");
        println!("plan: {:?}", plan(&base, "3.12"));
        let python = base.join("python").join("3.12").join("python.exe");
        if python.is_file() {
            let info = winlibs_info(&python);
            match info {
                Some(info) => println!(
                    "winlibs: total={} archive={} mirror={}",
                    info.total,
                    info.archive.display(),
                    info.mirror
                ),
                None => println!("winlibs: none"),
            }
        }
    }

    const SAMPLE: &str = r#"
        <a href="../">Parent directory</a>
        <a href="cpython-3.12.16%2B20261110-x86_64-pc-windows-msvc-install_only_stripped.tar.gz">x</a>
        <a href="cpython-3.12.16%2B20261110-aarch64-pc-windows-msvc-install_only_stripped.tar.gz">y</a>
        <a href="cpython-3.12.16%2B20261110-x86_64-unknown-linux-gnu-install_only_stripped.tar.gz">z</a>
    "#;

    #[test]
    fn picks_windows_x86_64_asset() {
        let (url, name) = pick_asset(SAMPLE, "https://mirror.example/base/", "3.12")
            .expect("asset");
        assert_eq!(
            name,
            "cpython-3.12.16+20261110-x86_64-pc-windows-msvc-install_only_stripped.tar.gz"
        );
        assert!(url.starts_with("https://mirror.example/base/cpython-3.12.16%2B"));
    }

    #[test]
    fn ignores_other_platforms() {
        let only_arm = r#"<a href="cpython-3.12.16%2B20261110-aarch64-pc-windows-msvc-install_only_stripped.tar.gz">y</a>"#;
        assert!(pick_asset(only_arm, "https://mirror.example/", "3.12").is_none());
    }

    #[test]
    fn decode_restores_plus_and_space() {
        assert_eq!(decode("a%2Bb%20c"), "a+b c");
    }
}
