use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn describe(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let text = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    text.trim().to_string()
}

fn curl() -> Command {
    crate::util::command("curl.exe")
}

/// HEAD 一次拿远端大小；拿不到就返回 0，界面会退化成不定长进度。
pub(crate) fn content_length(url: &str) -> u64 {
    let Ok(output) = curl()
        .arg("-L")
        .arg("--fail")
        .arg("-sI")
        .arg("--max-time")
        .arg("15")
        .arg(url)
        .output()
    else {
        return 0;
    };
    if !output.status.success() {
        return 0;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut total = 0;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("content-length")
            && let Ok(parsed) = value.trim().parse::<u64>()
        {
            total = parsed;
        }
    }
    total
}

/// 下载 + 校验。下载期间按 200ms 左右采样文件大小，回报字节数、总大小和滑动速度。
pub(crate) fn download_checked_with<F>(
    url: &str,
    dest: &Path,
    expected: Option<&str>,
    total: u64,
    progress: F,
) -> Result<(), String>
where
    F: FnMut(u64, u64, f64),
{
    download_with_progress(url, dest, total, progress)?;
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

/// 不校验哈希的下载入口，给 zip 编译器包用；只校验最终字节数。
pub(crate) fn download_to<F>(url: &str, dest: &Path, total: u64, progress: F) -> Result<(), String>
where
    F: FnMut(u64, u64, f64),
{
    download_with_progress(url, dest, total, progress)?;
    if total > 0 {
        let size = std::fs::metadata(dest).map(|meta| meta.len()).unwrap_or(0);
        if size != total {
            let _ = std::fs::remove_file(dest);
            return Err(format!(
                "下载大小不完整：期望 {total} 字节，实际 {size} 字节"
            ));
        }
    }
    Ok(())
}

fn download_with_progress<F>(
    url: &str,
    dest: &Path,
    total: u64,
    mut progress: F,
) -> Result<(), String>
where
    F: FnMut(u64, u64, f64),
{
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("创建下载目录失败：{error}"))?;
    }

    let initial = std::fs::metadata(dest).map(|meta| meta.len()).unwrap_or(0);
    let mut child = curl()
        .arg("-L")
        .arg("--fail")
        .arg("--retry")
        .arg("3")
        .arg("--connect-timeout")
        .arg("20")
        .arg("--speed-time")
        .arg("30")
        .arg("--speed-limit")
        .arg("1024")
        .arg("-C")
        .arg("-")
        .arg("-sS")
        .arg("-o")
        .arg(dest)
        .arg(url)
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("无法启动 curl.exe：{error}"))?;

    let mut last_bytes = initial;
    let mut last_tick = Instant::now();
    let mut speed = 0.0;
    let mut sampled = false;

    let status = loop {
        match child.try_wait() {
            Ok(Some(done)) => break done,
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("等待 curl 失败：{error}"));
            }
        }

        let bytes = std::fs::metadata(dest)
            .map(|meta| meta.len())
            .unwrap_or(last_bytes);
        let now = Instant::now();
        let elapsed = now.duration_since(last_tick).as_secs_f64();
        if elapsed >= 0.2 {
            let instant = bytes.saturating_sub(last_bytes) as f64 / elapsed;
            speed = if sampled {
                speed * 0.65 + instant * 0.35
            } else {
                instant
            };
            sampled = true;
            progress(bytes, total, speed);
            last_bytes = bytes;
            last_tick = now;
        }

        std::thread::sleep(Duration::from_millis(100));
    };

    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    let bytes = std::fs::metadata(dest).map(|meta| meta.len()).unwrap_or(0);
    progress(bytes, total, speed);

    if status.success() {
        Ok(())
    } else {
        Err(format!("下载失败：{}", stderr.trim()))
    }
}

/// 用系统 tar（Windows 上是 bsdtar）解 zip；winlibs 缓存目录直接用这个展开。
pub(crate) fn extract_zip<F>(
    archive: &Path,
    dest: &Path,
    on_line: F,
) -> Result<(), String>
where
    F: FnMut(String),
{
    std::fs::create_dir_all(dest)
        .map_err(|error| format!("创建解压目录失败：{error}"))?;
    let archive = archive.to_string_lossy().to_string();
    let dest = dest.to_string_lossy().to_string();
    run_streaming(
        Path::new("tar.exe"),
        &["-xf", &archive, "-C", &dest],
        &[],
        on_line,
    )?;
    Ok(())
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
        let hex: String = line
            .chars()
            .filter(|item| item.is_ascii_hexdigit())
            .collect();
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
    let source = if inner.is_dir() {
        inner
    } else {
        staging.clone()
    };

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

/// 跑命令，边跑边把输出按行回调出去；界面详情里显示的就是这些原始行。
pub(crate) fn run_streaming<F>(
    program: &Path,
    args: &[&str],
    envs: &[(&str, &Path)],
    mut on_line: F,
) -> Result<String, String>
where
    F: FnMut(String),
{
    let mut command = crate::util::command(program);
    command.args(args);
    for (key, value) in envs {
        command.env(key, value);
    }

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let log_path = std::env::temp_dir().join(format!(
        "nuitka-rs-stream-{}-{stamp}.log",
        std::process::id()
    ));
    let log =
        std::fs::File::create(&log_path).map_err(|error| format!("创建临时日志失败：{error}"))?;
    let log_err = log
        .try_clone()
        .map_err(|error| format!("复制临时日志句柄失败：{error}"))?;

    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err))
        .spawn()
        .map_err(|error| {
            let _ = std::fs::remove_file(&log_path);
            format!("无法启动 {}：{error}", program.display())
        })?;

    let mut offset = 0u64;
    let mut pending = String::new();
    let status = loop {
        drain_log(&log_path, &mut offset, &mut pending, &mut on_line);
        match child.try_wait() {
            Ok(Some(done)) => break done,
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_file(&log_path);
                return Err(format!("等待进程失败：{error}"));
            }
        }
        std::thread::sleep(Duration::from_millis(120));
    };
    drain_log(&log_path, &mut offset, &mut pending, &mut on_line);
    if !pending.trim().is_empty() {
        on_line(pending.trim_end().to_string());
    }

    let text = std::fs::read_to_string(&log_path).unwrap_or_default();
    let _ = std::fs::remove_file(&log_path);
    if status.success() {
        Ok(text.trim().to_string())
    } else if text.trim().is_empty() {
        Err("命令执行失败，未返回错误信息".to_string())
    } else {
        Err(text.trim().to_string())
    }
}

fn drain_log<F>(path: &Path, offset: &mut u64, pending: &mut String, on_line: &mut F)
where
    F: FnMut(String),
{
    let Ok(mut file) = std::fs::File::open(path) else {
        return;
    };
    let Ok(meta) = file.metadata() else {
        return;
    };
    if meta.len() <= *offset {
        return;
    }
    if file.seek(SeekFrom::Start(*offset)).is_err() {
        return;
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return;
    }
    *offset += bytes.len() as u64;
    pending.push_str(&String::from_utf8_lossy(&bytes));

    loop {
        let Some(index) = pending
            .char_indices()
            .find(|(_, ch)| *ch == '\n' || *ch == '\r')
            .map(|(index, _)| index)
        else {
            break;
        };
        let line = pending[..index].trim_end().to_string();
        pending.drain(..=index);
        if !line.trim().is_empty() {
            on_line(line);
        }
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
