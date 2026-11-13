use std::path::PathBuf;
use std::process::Command;

use crate::nuitka::{fetch, paths};

const PYTHON_LINE: &str = "3.12";
const MIRROR: &str = "https://pypi.tuna.tsinghua.edu.cn/simple";
const MIRROR_BASE: &str =
    "https://mirror.nju.edu.cn/github-release/astral-sh/python-build-standalone/LatestRelease/";
const RELEASE_API: &str =
    "https://api.github.com/repos/astral-sh/python-build-standalone/releases/latest";

pub(crate) struct Ready {
    pub python: PathBuf,
    pub nuitka: String,
}

/// M1：准备便携 Python + Nuitka，返回可用的解释器和 Nuitka 版本。
pub(crate) fn prepare<F: FnMut(&str)>(mut progress: F) -> Result<Ready, String> {
    let python_home = paths::python_dir()
        .ok_or_else(|| "找不到 LOCALAPPDATA".to_string())?
        .join(PYTHON_LINE);
    let downloads = paths::downloads_dir().ok_or_else(|| "找不到下载目录".to_string())?;
    let python = python_home.join("python.exe");

    if !python.is_file() {
        progress("解析便携 Python 下载地址");
        let (url, name) = release_asset()?;
        let archive = downloads.join(name);
        if !archive.is_file() {
            progress("下载便携 Python（约 24 MB）");
            fetch::download(&url, &archive)?;
        }
        progress("解压便携 Python");
        fetch::install(&archive, &python_home)?;
    }

    progress("检查 Python 版本");
    let version = fetch::run(&python, &["--version"])?;

    progress("安装 pip");
    let _ = fetch::run(&python, &["-m", "ensurepip", "--upgrade"]);
    let _ = fetch::run(&python, &["-m", "pip", "install", "--upgrade", "pip", "-i", MIRROR]);

    progress("安装 Nuitka（清华镜像）");
    fetch::run(
        &python,
        &["-m", "pip", "install", "--upgrade", "nuitka", "-i", MIRROR],
    )?;

    progress("验证 nuitka --version");
    let raw = fetch::run(&python, &["-m", "nuitka", "--version"])?;
    let summary = first_line(&raw).to_string();

    progress(&format!("环境就绪：{version} / Nuitka {summary}"));
    Ok(Ready {
        python,
        nuitka: summary,
    })
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text).trim()
}

/// 优先用环境变量直链，其次国内镜像，最后回退 GitHub API。
fn release_asset() -> Result<(String, String), String> {
    if let Ok(url) = std::env::var("NUITKA_RS_PBS_URL") {
        let url = url.trim().to_string();
        if !url.is_empty() {
            let name = url
                .rsplit('/')
                .next()
                .filter(|name| !name.is_empty())
                .ok_or_else(|| format!("无法从地址里取出文件名：{url}"))?
                .to_string();
            return Ok((url, name));
        }
    }

    match mirror_asset() {
        Ok(pair) => Ok(pair),
        Err(mirror_error) => github_asset()
            .map_err(|github_error| format!("镜像失败：{mirror_error}；GitHub 回退也失败：{github_error}")),
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
    let output = Command::new("curl.exe")
        .arg("-L")
        .arg("--fail")
        .arg("--silent")
        .arg("--show-error")
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
}
