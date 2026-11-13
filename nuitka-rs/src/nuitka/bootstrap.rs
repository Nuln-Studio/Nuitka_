use std::path::{Path, PathBuf};

use crate::nuitka::python_discovery::PyVersion;
use crate::nuitka::{fetch, lock, log, paths, selfcheck, toolchain};

const PYTHON_LINE: &str = "3.12";
const PYTHON_VERSION: PyVersion = PyVersion::new(3, 12, 0);
const MIRROR: &str = "https://pypi.tuna.tsinghua.edu.cn/simple";
const MIRROR_BASE: &str =
    "https://mirror.nju.edu.cn/github-release/astral-sh/python-build-standalone/LatestRelease/";
const RELEASE_API: &str =
    "https://api.github.com/repos/astral-sh/python-build-standalone/releases/latest";

pub(crate) struct Ready {
    pub python: PathBuf,
    pub nuitka: String,
    pub verified: bool,
    pub detail: String,
}

/// 准备便携 Python + Nuitka，最后真编一个程序验证这条链路是通的。
pub(crate) fn prepare<F: FnMut(&str)>(
    base: &Path,
    mut progress: F,
) -> Result<Ready, String> {
    // 同一个安装目录只允许一个流程在跑
    let _guard = lock::acquire(base)?;
    paths::set_base(base)?;

    let python_home = base.join("python").join(PYTHON_LINE);
    let python = python_home.join("python.exe");

    log::info(&format!("开始准备环境：{}", base.display()));

    if !python.is_file() {
        progress("解析便携 Python 下载地址");
        let archive = ensure_archive(base, &mut progress)?;
        progress("解压便携 Python");
        fetch::install(&archive, &python_home)?;
    }

    progress("检查 Python 版本");
    let version = fetch::run(&python, &["--version"])?;
    log::info(&format!("Python：{version}"));

    // 已经装好的就别再走网络，不然“验证一次”会卡在 pip 上
    let nuitka_ready = fetch::run(&python, &["-m", "nuitka", "--version"]).is_ok();
    if nuitka_ready {
        progress("检测到已安装的 Nuitka，跳过安装");
    } else {
        progress("安装 pip");
        let _ = fetch::run(&python, &["-m", "ensurepip", "--upgrade"]);
        let _ = fetch::run(&python, &["-m", "pip", "install", "--upgrade", "pip", "-i", MIRROR]);

        progress("安装 Nuitka（含 onefile 压缩组件）");
        fetch::run(
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
        )?;
    }

    ensure_zstandard(&python, &mut progress);

    let tool = toolchain::resolve(PYTHON_VERSION);
    let raw = if tool.is_some() {
        progress("检测到可用编译器，跳过下载");
        fetch::run(&python, &["-m", "nuitka", "--version"])?
    } else {
        progress("准备编译器（首次会下载 winlibs，约 200 MB）");
        let cache = base.join("nuitka-cache");
        match fetch::run_with_env(
            &python,
            &["-m", "nuitka", "--version", "--assume-yes-for-downloads"],
            &[("NUITKA_CACHE_DIR", cache.as_path())],
        ) {
            Ok(output) => output,
            Err(error) => {
                log::error(&format!("winlibs 预下载失败，稍后由 Nuitka 自行下载：{error}"));
                progress("编译器稍后由 Nuitka 自行下载");
                fetch::run(&python, &["-m", "nuitka", "--version"])?
            }
        }
    };
    let summary = first_line(&raw).to_string();

    progress("挑选编译器");
    if let Some(tool) = &tool {
        log::info(&format!(
            "选中编译器：{}（{}）",
            tool.label,
            tool.kind_label()
        ));
    }

    let outcome = selfcheck::run(&python, base, tool.as_ref(), &mut progress);

    progress(&format!("环境就绪：{version} / Nuitka {summary}"));
    log::info(&format!(
        "环境就绪：{version} / Nuitka {summary} / 自检{}",
        if outcome.ok { "通过" } else { "未通过" }
    ));

    Ok(Ready {
        python,
        nuitka: summary,
        verified: outcome.ok,
        detail: outcome.detail,
    })
}

/// 拿到一个校验过、可以解压的压缩包。
fn ensure_archive<F: FnMut(&str)>(base: &Path, progress: &mut F) -> Result<PathBuf, String> {
    let downloads = base.join("downloads");
    let candidates = candidates();
    if candidates.is_empty() {
        return Err("没有拿到可用的下载地址，请检查网络或 NUITKA_RS_PBS_URL".to_string());
    }

    let mut last_error = String::from("没有找到可用下载源");
    for (url, name) in candidates {
        let archive = downloads.join(&name);
        let expected = expected_hash(&url, &name);

        if archive.is_file() && reusable(&archive, expected.as_deref()) {
            log::info(&format!("复用已下载的压缩包：{}", archive.display()));
            return Ok(archive);
        }

        log::info(&format!("下载 Python：{url}"));
        progress("下载便携 Python（约 24 MB）");
        match fetch::download_checked(&url, &archive, expected.as_deref()) {
            Ok(()) => return Ok(archive),
            Err(error) => {
                log::error(&format!("下载失败（{url}）：{error}"));
                last_error = error;
            }
        }
    }

    Err(last_error)
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
fn candidates() -> Vec<(String, String)> {
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

    if let Ok(pair) = mirror_asset() {
        out.push(pair);
    }
    if let Ok(pair) = github_asset() {
        out.push(pair);
    }

    out
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text).trim()
}

/// onefile 没有 zstandard 也能跑，只是不压缩、产物大一圈。装不上就算了。
fn ensure_zstandard<F: FnMut(&str)>(python: &Path, progress: &mut F) {
    if fetch::run(python, &["-c", "import zstandard"]).is_ok() {
        return;
    }
    progress("补齐 onefile 压缩组件（zstandard）");
    match fetch::run(
        python,
        &["-m", "pip", "install", "--upgrade", "zstandard", "-i", MIRROR],
    ) {
        Ok(_) => log::info("zstandard 已安装"),
        Err(error) => log::error(&format!("zstandard 安装失败，onefile 将不压缩：{error}")),
    }
}

fn mirror_asset() -> Result<(String, String), String> {
    let base = std::env::var("NUITKA_RS_PBS_BASE").unwrap_or_else(|_| MIRROR_BASE.to_string());
    let html = curl_text(&base)?;
    pick_asset(&html, &base).ok_or_else(|| "镜像目录里没找到 3.12 的 Windows 便携包".to_string())
}

fn pick_asset(html: &str, base: &str) -> Option<(String, String)> {
    let mut position = 0;

    while let Some(found) = html[position..].find("href=\"") {
        let start = position + found + 6;
        let end = html[start..].find('"')?;
        let href = &html[start..start + end];
        position = start + end;

        let decoded = decode(href);
        let matched = decoded.starts_with("cpython-3.12.")
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

fn github_asset() -> Result<(String, String), String> {
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
        let matched = name.starts_with("cpython-3.12.")
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
    text.replace("%2B", "+").replace("%2b", "+").replace("%20", " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
        <a href="../">Parent directory</a>
        <a href="cpython-3.12.16%2B20261110-x86_64-pc-windows-msvc-install_only_stripped.tar.gz">x</a>
        <a href="cpython-3.12.16%2B20261110-aarch64-pc-windows-msvc-install_only_stripped.tar.gz">y</a>
        <a href="cpython-3.12.16%2B20261110-x86_64-unknown-linux-gnu-install_only_stripped.tar.gz">z</a>
    "#;

    #[test]
    fn picks_windows_x86_64_asset() {
        let (url, name) = pick_asset(SAMPLE, "https://mirror.example/base/").expect("asset");
        assert_eq!(
            name,
            "cpython-3.12.16+20261110-x86_64-pc-windows-msvc-install_only_stripped.tar.gz"
        );
        assert!(url.starts_with("https://mirror.example/base/cpython-3.12.16%2B"));
    }

    #[test]
    fn ignores_other_platforms() {
        let only_arm = r#"<a href="cpython-3.12.16%2B20261110-aarch64-pc-windows-msvc-install_only_stripped.tar.gz">y</a>"#;
        assert!(pick_asset(only_arm, "https://mirror.example/").is_none());
    }

    #[test]
    fn decode_restores_plus_and_space() {
        assert_eq!(decode("a%2Bb%20c"), "a+b c");
    }
}
