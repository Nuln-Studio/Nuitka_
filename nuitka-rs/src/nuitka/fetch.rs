use std::path::Path;
use std::process::Command;

fn describe(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let text = if stderr.trim().is_empty() { stdout } else { stderr };
    text.trim().to_string()
}

fn curl() -> Command {
    crate::util::command("curl.exe")
}

/// 用系统 curl 下载；`-C -` 支持断点续传，`-L` 跟随 GitHub 的跳转。
pub(crate) fn download(url: &str, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("创建下载目录失败：{error}"))?;
    }
    let output = curl()
        .arg("-L")
        .arg("--fail")
        .arg("--retry")
        .arg("3")
        .arg("-C")
        .arg("-")
        .arg("-sS")
        .arg("-o")
        .arg(dest)
        .arg(url)
        .output()
        .map_err(|error| format!("无法启动 curl.exe：{error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("下载失败：{}", describe(&output)))
    }
}

/// 下载 + 校验。有 sha256 就比对哈希，没有就至少确认压缩包能正常列出内容。
pub(crate) fn download_checked(url: &str, dest: &Path, expected: Option<&str>) -> Result<(), String> {
    download(url, dest)?;
    match expected {
        Some(expected) => {
            let actual = sha256(dest)?;
            if actual.eq_ignore_ascii_case(expected) {
                Ok(())
            } else {
                let _ = std::fs::remove_file(dest);
                Err(format!("校验不通过：期望 {expected}，实际 {actual}"))
            }
        }
        None => verify_archive(dest),
    }
}

/// 压缩包完整性：能列出清单就说明没被截断。
pub(crate) fn verify_archive(archive: &Path) -> Result<(), String> {
    let output = crate::util::command("tar.exe")
        .arg("-tzf")
        .arg(archive)
        .output()
        .map_err(|error| format!("无法启动 tar.exe：{error}"))?;
    if output.status.success() && !output.stdout.is_empty() {
        Ok(())
    } else {
        let _ = std::fs::remove_file(archive);
        Err(format!("压缩包损坏：{}", describe(&output)))
    }
}

/// 用系统 certutil 算 SHA256，省得引第三方库。
pub(crate) fn sha256(path: &Path) -> Result<String, String> {
    let output = crate::util::command("certutil.exe")
        .arg("-hashfile")
        .arg(path)
        .arg("SHA256")
        .output()
        .map_err(|error| format!("无法启动 certutil.exe：{error}"))?;
    if !output.status.success() {
        return Err(format!("计算哈希失败：{}", describe(&output)));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let hex: String = line.chars().filter(|item| item.is_ascii_hexdigit()).collect();
        if hex.len() == 64 {
            return Ok(hex.to_lowercase());
        }
    }
    Err("没在 certutil 输出里找到哈希".to_string())
}

/// 取一个小文本文件（用于 .sha256 / SHA256SUMS），失败返回 None。
pub(crate) fn fetch_text(url: &str) -> Option<String> {
    let output = curl()
        .arg("-L")
        .arg("--fail")
        .arg("-sS")
        .arg("--max-time")
        .arg("20")
        .arg(url)
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        None
    }
}

/// 从文本里挑出一个 64 位十六进制哈希。
pub(crate) fn pick_hash(text: &str) -> Option<String> {
    for word in text.split(|item: char| !item.is_ascii_alphanumeric()) {
        if word.len() == 64 && word.chars().all(|item| item.is_ascii_hexdigit()) {
            return Some(word.to_lowercase());
        }
    }
    None
}

/// 在 SHA256SUMS 里按文件名找对应哈希。
pub(crate) fn hash_for_file(sums: &str, name: &str) -> Option<String> {
    for line in sums.lines() {
        if !line.contains(name) {
            continue;
        }
        if let Some(hash) = pick_hash(line) {
            return Some(hash);
        }
    }
    None
}

/// 用系统 tar 解压，并把压缩包里的 `python/` 目录提到目标位置。
/// 先解到 staging 再改名，失败不会留下半个安装。
pub(crate) fn install(archive: &Path, target: &Path) -> Result<(), String> {
    verify_archive(archive)?;

    let staging = target.with_extension("staging");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|error| format!("创建临时目录失败：{error}"))?;

    let output = crate::util::command("tar.exe")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(&staging)
        .output()
        .map_err(|error| format!("无法启动 tar.exe：{error}"))?;
    if !output.status.success() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!("解压失败：{}", describe(&output)));
    }

    let inner = staging.join("python");
    let source = if inner.is_dir() { inner } else { staging.clone() };

    if !source.join("python.exe").is_file() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err("解压结果里没有 python.exe".to_string());
    }

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("创建安装目录失败：{error}"))?;
    }
    let _ = std::fs::remove_dir_all(target);
    std::fs::rename(&source, target).map_err(|error| {
        let _ = std::fs::remove_dir_all(&staging);
        format!("移动到安装目录失败：{error}")
    })?;
    let _ = std::fs::remove_dir_all(&staging);
    Ok(())
}

/// 运行一个命令并返回合并后的输出，失败时把输出一起带回来。
pub(crate) fn run(program: &Path, args: &[&str]) -> Result<String, String> {
    run_with_env(program, args, &[])
}

pub(crate) fn run_with_env(
    program: &Path,
    args: &[&str],
    envs: &[(&str, &Path)],
) -> Result<String, String> {
    let mut command = crate::util::command(program);
    command.args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    let output = command
        .output()
        .map_err(|error| format!("无法启动 {}：{error}", program.display()))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(describe(&output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "d2c4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3d4";

    #[test]
    fn picks_hash_from_checksum_file() {
        let text = format!("{HASH}  cpython-3.12.16-x86_64.tar.gz\n");
        assert_eq!(pick_hash(&text), Some(HASH.to_string()));
    }

    #[test]
    fn picks_hash_from_certutil_output() {
        let text = format!("SHA256 hash of file:\n{HASH}\nCertUtil: -hashfile completed\n");
        assert_eq!(pick_hash(&text), Some(HASH.to_string()));
    }

    #[test]
    fn matches_hash_by_file_name() {
        let sums = format!("aaa  other.tar.gz\n{HASH}  target.tar.gz\n");
        assert_eq!(
            hash_for_file(&sums, "target.tar.gz"),
            Some(HASH.to_string())
        );
        assert_eq!(hash_for_file(&sums, "missing.tar.gz"), None);
    }

    #[test]
    fn ignores_short_hex_strings() {
        assert_eq!(pick_hash("abc123"), None);
    }
}
