use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::{log, toolchain};

/// 自检产物要打印这句话，才算真的跑起来了。
const MARKER: &str = "nuitka-selfcheck-ok";
const COMPILE_LIMIT: Duration = Duration::from_secs(420);
const RUN_LIMIT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Outcome {
    pub ok: bool,
    pub compiler: String,
    pub at: u64,
    pub detail: String,
}

pub(crate) fn file(base: &Path) -> PathBuf {
    base.join("selfcheck.json")
}

pub(crate) fn load(base: &Path) -> Option<Outcome> {
    let text = std::fs::read_to_string(file(base)).ok()?;
    serde_json::from_str(&text).ok()
}

/// 真编一次：写一个最小脚本，用 Nuitka 打成 exe，再运行它看输出。
pub(crate) fn run<F: FnMut(&str)>(
    python: &Path,
    base: &Path,
    tool: Option<&toolchain::Toolchain>,
    mut progress: F,
) -> Outcome {
    let workspace = base.join("selfcheck");
    let _ = std::fs::create_dir_all(&workspace);
    let script = workspace.join("hello.py");
    let _ = std::fs::write(&script, format!("print(\"{MARKER}\")\n"));
    let exe = workspace.join("selfcheck.exe");
    let _ = std::fs::remove_file(&exe);

    let compiler = tool
        .map(|tool| tool.label.clone())
        .unwrap_or_else(|| "未检测到编译器".to_string());

    log::info(&format!("编译自检开始，编译器：{compiler}"));
    progress("编译自检：用 Nuitka 打一个最小程序（首次会慢一些）");

    let mut command = crate::util::command(python);
    command
        .env("PYTHONIOENCODING", "utf-8")
        .arg("-m")
        .arg("nuitka")
        .arg("--onefile")
        .arg("--assume-yes-for-downloads")
        .arg("--windows-console-mode=force")
        .arg(format!("--output-dir={}", workspace.display()))
        .arg("--output-filename=selfcheck");
    if let Some(tool) = tool {
        // 和真正打包用同一套后端参数，否则自检通过、真打包失败
        command.args(&tool.nuitka_args);
    }
    command.arg(&script);
    if let Some(tool) = tool {
        tool.apply(&mut command);
    }

    let build_log = workspace.join("build.log");
    if let Err(error) = run_timed(command, &build_log, COMPILE_LIMIT) {
        let detail = format!("编译失败：{error}；{}", tail(&build_log, 12));
        log::error(&format!("编译自检失败：{detail}"));
        return finish(base, false, compiler, detail);
    }

    if !exe.is_file() {
        let detail = format!("编译报告成功，但没有生成 {}；{}", exe.display(), tail(&build_log, 12));
        log::error(&format!("编译自检失败：{detail}"));
        return finish(base, false, compiler, detail);
    }

    progress("编译自检：运行生成的程序");
    let run_log = workspace.join("run.log");
    let verify = crate::util::command(&exe);
    match run_timed(verify, &run_log, RUN_LIMIT) {
        Ok(()) => {
            let text = std::fs::read_to_string(&run_log).unwrap_or_default();
            if text.contains(MARKER) {
                log::info("编译自检通过");
                progress("编译自检：通过");
                finish(base, true, compiler, "编译并运行通过".to_string())
            } else {
                let detail = format!("程序能运行，但输出不对：{}", text.trim());
                log::error(&format!("编译自检失败：{detail}"));
                finish(base, false, compiler, detail)
            }
        }
        Err(error) => {
            let detail = format!("生成的程序无法运行：{error}；{}", tail(&run_log, 8));
            log::error(&format!("编译自检失败：{detail}"));
            finish(base, false, compiler, detail)
        }
    }
}

fn finish(base: &Path, ok: bool, compiler: String, detail: String) -> Outcome {
    let outcome = Outcome {
        ok,
        compiler,
        at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_secs())
            .unwrap_or(0),
        detail,
    };
    if let Ok(text) = serde_json::to_string_pretty(&outcome) {
        let _ = std::fs::write(file(base), text);
    }
    outcome
}

/// 跑一个命令并等它结束；输出直接写文件，避免管道塞满把子进程卡死。
fn run_timed(mut command: Command, log_path: &Path, limit: Duration) -> Result<(), String> {
    let file = File::create(log_path).map_err(|error| format!("无法创建日志：{error}"))?;
    let mirror = file
        .try_clone()
        .map_err(|error| format!("无法复制日志句柄：{error}"))?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(mirror));

    let mut child = command.spawn().map_err(|error| format!("无法启动：{error}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    return Ok(());
                }
                return Err(match status.code() {
                    Some(code) => format!("退出码 {code}"),
                    None => "进程被终止".to_string(),
                });
            }
            Ok(None) => {
                if started.elapsed() > limit {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("超过 {} 秒仍未结束", limit.as_secs()));
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

fn tail(path: &Path, lines: usize) -> String {
    let Ok(text) = std::fs::read_to_string(path) else {
        return "（没有日志）".to_string();
    };
    let all: Vec<&str> = text.lines().filter(|line| !line.trim().is_empty()).collect();
    let start = all.len().saturating_sub(lines);
    all[start..].join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join("nuitka-rs-selfcheck-test").join(name);
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch");
        root
    }

    #[test]
    fn tail_keeps_the_last_lines() {
        let dir = scratch("tail");
        let path = dir.join("log.txt");
        std::fs::write(&path, "a\nb\nc\nd\n").expect("write");
        assert_eq!(tail(&path, 2), "c | d");
    }

    #[test]
    fn missing_log_is_reported() {
        let dir = scratch("missing");
        assert_eq!(tail(&dir.join("nope.txt"), 3), "（没有日志）");
    }

    #[test]
    fn outcome_round_trips_through_json() {
        let dir = scratch("json");
        let outcome = finish(&dir, true, "MSVC 14.50".to_string(), "通过".to_string());
        let loaded = load(&dir).expect("load");
        assert!(loaded.ok);
        assert_eq!(loaded.compiler, outcome.compiler);
        assert_eq!(loaded.detail, "通过");
    }

    #[test]
    fn failed_command_reports_exit_code() {
        let dir = scratch("exit");
        let log = dir.join("out.log");
        let mut command = Command::new("cmd.exe");
        command.args(["/c", "exit 3"]);
        let result = run_timed(command, &log, Duration::from_secs(10));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("3"));
    }

    #[test]
    fn timeout_kills_the_child() {
        let dir = scratch("timeout");
        let log = dir.join("out.log");
        let started = Instant::now();
        let mut command = Command::new("cmd.exe");
        command.args(["/c", "ping -n 20 127.0.0.1 >nul"]);
        let result = run_timed(command, &log, Duration::from_millis(400));
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// 真编译一次，只有本机装好环境才跑得起来。手动执行：
    /// `cargo test real_selfcheck -- --ignored --nocapture`
    #[test]
    #[ignore = "需要本机已装好 Python + Nuitka"]
    fn real_selfcheck_passes() {
        use super::super::paths;
        use super::super::python_discovery::PyVersion;
        use super::super::toolchain;

        let base = paths::base().expect("安装目录");
        let python = base.join("python").join("3.12").join("python.exe");
        assert!(python.is_file(), "缺少 {}", python.display());

        let tool = toolchain::resolve(PyVersion::new(3, 12, 0));
        println!("编译器：{:?}", tool.as_ref().map(|item| item.label.clone()));

        let outcome = run(&python, &base, tool.as_ref(), |stage| println!("{stage}"));
        println!("结果：{outcome:?}");
        assert!(outcome.ok, "{}", outcome.detail);
    }
}
