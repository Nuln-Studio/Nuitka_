use std::path::Path;
use std::process::Command;

fn describe(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let text = if stderr.trim().is_empty() { stdout } else { stderr };
    text.trim().to_string()
}

/// 用系统 curl 下载；`-C -` 支持断点续传，`-L` 跟随 GitHub 的跳转。
pub(crate) fn download(url: &str, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("创建下载目录失败：{error}"))?;
    }
    let output = Command::new("curl.exe")
        .arg("-L")
        .arg("--fail")
        .arg("--retry")
        .arg("3")
        .arg("-C")
        .arg("-")
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

/// 用系统 tar 解压，并把压缩包里的 `python/` 目录提到目标位置。
pub(crate) fn install(archive: &Path, target: &Path) -> Result<(), String> {
    let staging = target.with_extension("staging");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|error| format!("创建临时目录失败：{error}"))?;

    let output = Command::new("tar.exe")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(&staging)
        .output()
        .map_err(|error| format!("无法启动 tar.exe：{error}"))?;
    if !output.status.success() {
        return Err(format!("解压失败：{}", describe(&output)));
    }

    let inner = staging.join("python");
    let source = if inner.join("python.exe").is_file() || inner.is_dir() {
        inner
    } else {
        staging.clone()
    };

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("创建安装目录失败：{error}"))?;
    }
    let _ = std::fs::remove_dir_all(target);
    std::fs::rename(&source, target).map_err(|error| format!("移动到安装目录失败：{error}"))?;
    let _ = std::fs::remove_dir_all(&staging);
    Ok(())
}

/// 运行一个命令并返回合并后的输出，失败时把输出一起带回来。
pub(crate) fn run(program: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("无法启动 {}：{error}", program.display()))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(describe(&output))
    }
}
