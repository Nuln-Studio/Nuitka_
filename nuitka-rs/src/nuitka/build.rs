use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::sync::mpsc::{Sender, channel};
use std::time::Instant;

use super::python_discovery::PyVersion;
use super::{log, toolchain};

/// 同一时间只允许一个打包任务，取消时要用这个 pid。
static RUNNING: Mutex<Option<u32>> = Mutex::new(None);

#[derive(Debug, Clone, serde::Deserialize)]
pub(crate) struct Request {
    #[serde(default)]
    pub python: String,
    pub script: String,
    #[serde(default)]
    pub out_dir: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub onefile: bool,
    #[serde(default)]
    pub console: bool,
    #[serde(default)]
    pub show_progress: bool,
    #[serde(default)]
    pub download: bool,
    /// "msvc" 或 "gcc"；不指定的话 Nuitka 会自己挑，往往挑错
    #[serde(default)]
    pub compiler: String,
    #[serde(default)]
    pub extra: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct Outcome {
    pub ok: bool,
    pub target: String,
    pub seconds: u64,
    pub detail: String,
    /// Nuitka 自己降级 / 关掉某项能力时，翻成用户看得懂的一句话，界面据此弹窗。
    pub notices: Vec<String>,
}

/// 只在日志里躺着的已知警告，命中就翻译成面向用户的一句话。
/// 判据必须来自 Nuitka 的真实输出：它会在系统里另找一个带 zstandard 的 Python，
/// 我们自己预检反而会误报。
const NOTICES: &[(&str, &str)] = &[(
    "Onefile mode cannot compress without 'zstandard'",
    "单文件模式这次没有压缩：当前 Python 环境缺 zstandard，Nuitka 把整个 payload 原样塞进了 exe。\
功能不受影响，但体积会明显偏大，首次启动也会稍慢一点。",
)];

fn notice_for(line: &str) -> Option<&'static str> {
    NOTICES
        .iter()
        .find(|(needle, _)| line.contains(needle))
        .map(|(_, text)| *text)
}

/// 实际传给 Nuitka 的参数（不含 python 自身）。
fn arguments(request: &Request) -> Vec<String> {
    let mut args: Vec<String> = vec!["-m".into(), "nuitka".into()];
    args.push(
        if request.onefile {
            "--onefile"
        } else {
            "--standalone"
        }
        .to_string(),
    );
    // 压缩阶段按 CPU 核数开线程，内存小的机器会直接把内存吃爆
    if request.onefile
        && let Some(jobs) = compression_jobs()
    {
        args.push(format!("--jobs={jobs}"));
    }
    args.push(format!(
        "--windows-console-mode={}",
        if request.console { "force" } else { "disable" }
    ));
    if request.show_progress {
        args.push("--show-progress".to_string());
    }
    if request.download {
        args.push("--assume-yes-for-downloads".to_string());
    }
    // 必须显式指定后端：Windows 上 Nuitka 默认走 MinGW，会无视已经验证过的 MSVC
    if let Some(rest) = request.compiler.strip_prefix("msvc") {
        // "msvc:14.50" 里带版本号；没带就退回 latest，再不行用环境变量钉死
        let version = rest.trim_start_matches(':').trim();
        let version = if version.is_empty() {
            std::env::var("NUITKA_RS_MSVC").unwrap_or_else(|_| "latest".to_string())
        } else {
            version.to_string()
        };
        args.push(format!("--msvc={version}"));
    } else if request.compiler == "gcc" {
        args.push("--mingw64".to_string());
    }
    if let Some(dir) = clean(&request.out_dir) {
        args.push(format!("--output-dir={dir}"));
    }
    if let Some(name) = clean(&request.name) {
        args.push(format!("--output-filename={name}"));
    }
    if let Some(icon) = clean(&request.icon) {
        args.push(format!("--windows-icon-from-ico={icon}"));
    }
    for token in request.extra.split_whitespace() {
        if !token.is_empty() {
            args.push(token.to_string());
        }
    }
    args.push(request.script.clone());
    args
}

fn clean(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// zstd level 22 每个压缩线程要几百 MB。Nuitka 默认按 CPU 核数开线程，
/// 内存不够的机器会在压缩阶段直接 `Allocation error : not enough memory`。
const COMPRESSION_WORKER_MB: u64 = 600;

#[cfg(windows)]
fn available_memory_mb() -> Option<u64> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut status = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    unsafe { GlobalMemoryStatusEx(&mut status).ok()? };
    Some(status.ullAvailPhys / (1024 * 1024))
}

#[cfg(not(windows))]
fn available_memory_mb() -> Option<u64> {
    None
}

/// 需要显式压线程数时返回 `--jobs` 的值；内存够用返回 None，保持 Nuitka 默认。
fn compression_jobs() -> Option<usize> {
    let cores = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1);
    jobs_for(available_memory_mb()?, cores)
}

fn jobs_for(free_mb: u64, cores: usize) -> Option<usize> {
    let affordable = (free_mb / COMPRESSION_WORKER_MB).max(1) as usize;
    (affordable < cores).then_some(affordable)
}

/// 单文件压缩要用到 Python 里的 zstandard；没有它 Nuitka 会静默降级成不压缩。
pub(crate) fn compression_ready(python: &Path) -> bool {
    crate::util::command(python)
        .args(["-c", "import zstandard"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn quote(value: &str) -> String {
    if value.contains(' ') {
        format!("\"{value}\"")
    } else {
        value.to_string()
    }
}

/// 给界面看的完整命令行，保证和真正执行的是同一份参数。
pub(crate) fn command_line(request: &Request) -> String {
    let python = clean(&request.python).unwrap_or("python");
    let mut parts = vec![quote(python)];
    for argument in arguments(request) {
        if let Some(rest) = argument
            .strip_prefix("--output-dir=")
            .or_else(|| argument.strip_prefix("--output-filename="))
            .or_else(|| argument.strip_prefix("--windows-icon-from-ico="))
        {
            let flag = argument.split('=').next().unwrap_or_default();
            parts.push(format!("{flag}={}", quote(rest)));
        } else {
            parts.push(quote(&argument));
        }
    }
    parts.join(" ")
}

fn target_of(request: &Request, out_dir: &Path) -> Option<PathBuf> {
    let name = clean(&request.name)
        .map(|item| item.to_string())
        .or_else(|| {
            Path::new(&request.script)
                .file_stem()
                .map(|item| item.to_string_lossy().to_string())
        })?;
    if request.onefile {
        Some(out_dir.join(format!("{name}.exe")))
    } else {
        Some(out_dir.join(format!("{name}.dist")))
    }
}

fn python_version(python: &Path) -> Option<PyVersion> {
    let output = crate::util::command(python)
        .args(["-c", "import sys;print('%d.%d.%d' % sys.version_info[:3])"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    PyVersion::parse(&text)
}

/// 界面用的粗粒度进度：从 Nuitka 的输出里认阶段。
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct Progress {
    pub percent: u8,
    pub stage: String,
}

#[derive(Debug, Default)]
pub(crate) struct Tracker {
    total_modules: usize,
    last_percent: u8,
}

impl Tracker {
    /// 喂一行输出，只有进度真的往前走才返回。
    /// Nuitka 会跑好几遍（PASS 1 / PASS 2），模块计数会重来，所以这里必须单调。
    pub(crate) fn feed(&mut self, line: &str) -> Option<Progress> {
        let measured = self.measure(line)?;
        if measured.percent <= self.last_percent {
            return None;
        }
        self.last_percent = measured.percent;
        Some(measured)
    }

    fn measure(&mut self, line: &str) -> Option<Progress> {
        // 警告/错误行里也会出现 "Onefile"、"linking" 这类词，不能拿来算进度
        if line.contains("WARNING") || line.contains("ERROR") {
            return None;
        }

        if let Some(rest) = line.strip_prefix("Nuitka-Progress: Optimizing module ") {
            let remaining = rest
                .split(", ")
                .find_map(|part| part.strip_suffix(" more modules to go after that."))
                .and_then(|value| value.parse::<usize>().ok())?;
            // 每一遍开头会有 "0 more modules"（比如 __main__），那是个假总数
            if remaining < 2 {
                return None;
            }
            self.total_modules = self.total_modules.max(remaining + 1);
            let done = self.total_modules.saturating_sub(remaining);
            let ratio = done as f32 / self.total_modules as f32;
            return Some(Progress {
                percent: (8.0 + ratio * 50.0).round() as u8,
                stage: "分析依赖模块".to_string(),
            });
        }

        let milestones: &[(&str, u8, &str)] = &[
            ("Starting Python compilation", 4, "读取脚本"),
            ("Completed Python level compilation", 62, "完成 Python 层编译"),
            ("Generating source code", 68, "生成 C 代码"),
            ("Running data composer", 72, "整理常量数据"),
            ("Running C compilation via Scons", 76, "编译 C 代码"),
            ("Backend C compiler", 80, "调用编译器"),
            ("Backend C linking", 88, "链接"),
            ("Onefile C compiler", 92, "打包引导程序"),
            ("Onefile C linking", 95, "生成单文件"),
        ];

        for (needle, percent, stage) in milestones {
            if line.contains(needle) {
                return Some(Progress {
                    percent: *percent,
                    stage: stage.to_string(),
                });
            }
        }
        None
    }
}

/// 跑一次打包，逐行把输出交给回调。
pub(crate) fn run<F: FnMut(&str)>(request: &Request, mut on_line: F) -> Result<Outcome, String> {
    let script = Path::new(request.script.trim());
    if !script.is_file() {
        return Err(format!("入口脚本不存在：{}", script.display()));
    }

    let python = clean(&request.python)
        .map(PathBuf::from)
        .ok_or_else(|| "还没找到可用的 Python，请先准备环境".to_string())?;
    if !python.is_file() {
        return Err(format!("Python 不存在：{}", python.display()));
    }

    let out_dir = match clean(&request.out_dir) {
        Some(dir) => PathBuf::from(dir),
        None => script
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "无法确定输出目录".to_string())?,
    };
    std::fs::create_dir_all(&out_dir).map_err(|error| format!("创建输出目录失败：{error}"))?;

    // 图标路径错的话 Nuitka 会编译到一半才报错，提前拦住
    if let Some(icon) = clean(&request.icon) {
        let path = Path::new(icon);
        if !path.is_file() {
            return Err(format!("图标文件不存在：{}", path.display()));
        }
        let is_ico = path
            .extension()
            .map(|ext| ext.to_string_lossy().eq_ignore_ascii_case("ico"))
            .unwrap_or(false);
        if !is_ico {
            return Err(format!("图标必须是 .ico 文件：{}", path.display()));
        }
    }

    if RUNNING.lock().map(|slot| slot.is_some()).unwrap_or(false) {
        return Err("已经有一个打包任务在跑".to_string());
    }

    let tool = python_version(&python).and_then(toolchain::resolve);
    if let Some(tool) = &tool {
        on_line(&format!("[环境] 使用编译器：{}", tool.describe()));
    }
    on_line(&format!("[命令] {}", command_line(request)));
    log::info(&format!(
        "开始打包：{} / 编译器 {}",
        command_line(request),
        tool.as_ref()
            .map(|item| item.describe())
            .unwrap_or_else(|| "未检测到".to_string())
    ));

    let started = Instant::now();
    let mut args = arguments(request);
    let mut notices: Vec<String> = Vec::new();
    let (mut ok, mut failure) = execute(&python, &args, tool.as_ref(), &mut notices, &mut on_line)?;

    // MSVC 认不出来时（新版本 VS 很常见），退到 Nuitka 自带的 winlibs 再试一次
    if !ok
        && request.compiler.starts_with("msvc")
        && let Some(gcc) = toolchain::fallback_gcc()
    {
        on_line("[重试] MSVC 后端失败，改用 winlibs gcc 重新编译");
        log::info(&format!("MSVC 失败，改用 {} 重试", gcc.describe()));
        let mut retry = request.clone();
        retry.compiler = "gcc".to_string();
        args = arguments(&retry);
        on_line(&format!("[命令] {}", command_line(&retry)));
        let (next_ok, next_failure) =
            execute(&python, &args, Some(&gcc), &mut notices, &mut on_line)?;
        ok = next_ok;
        failure = next_failure;
    }

    for notice in &notices {
        log::info(&format!("提示：{notice}"));
    }

    let seconds = started.elapsed().as_secs();
    let target = target_of(request, &out_dir)
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let produced = !target.is_empty() && Path::new(&target).exists();

    if ok && produced {
        log::info(&format!("打包完成：{target}（{seconds} 秒）"));
        Ok(Outcome {
            ok: true,
            target,
            seconds,
            detail: format!("完成，用时 {seconds} 秒"),
            notices,
        })
    } else {
        let detail = if ok {
            format!("编译结束但没有找到产物：{target}")
        } else {
            failure
        };
        log::error(&format!("打包失败：{detail}"));
        Ok(Outcome {
            ok: false,
            target,
            seconds,
            detail,
            notices,
        })
    }
}

/// 起一个 Nuitka 进程，把输出逐行吐出去，返回 (是否成功, 失败描述)。
fn execute(
    python: &Path,
    args: &[String],
    tool: Option<&toolchain::Toolchain>,
    notices: &mut Vec<String>,
    on_line: &mut dyn FnMut(&str),
) -> Result<(bool, String), String> {
    let mut command = crate::util::command(python);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // 输出统一成 UTF-8：否则中文路径会以 GBK 吐出来，界面变乱码
        .env("PYTHONIOENCODING", "utf-8");
    if let Some(tool) = tool {
        tool.apply(&mut command);
    }

    let mut child = command
        .spawn()
        .map_err(|error| format!("无法启动 Nuitka：{error}"))?;
    if let Ok(mut slot) = RUNNING.lock() {
        *slot = Some(child.id());
    }

    let (sender, receiver) = channel::<String>();
    let mut readers = Vec::new();
    if let Some(stream) = child.stdout.take() {
        readers.push(pump(stream, sender.clone()));
    }
    if let Some(stream) = child.stderr.take() {
        readers.push(pump(stream, sender.clone()));
    }
    drop(sender);

    let mut last_problem: Option<String> = None;
    for line in receiver {
        if let Some(notice) = notice_for(&line)
            && !notices.iter().any(|item| item == notice)
        {
            notices.push(notice.to_string());
        }
        if line.contains("FATAL") || line.contains("ERROR") {
            last_problem = Some(line.clone());
        }
        on_line(&line);
    }
    for reader in readers {
        let _ = reader.join();
    }

    let status = child.wait().map_err(|error| error.to_string())?;
    if let Ok(mut slot) = RUNNING.lock() {
        *slot = None;
    }

    if status.success() {
        Ok((true, String::new()))
    } else {
        let code = status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "未知".to_string());
        let detail = match last_problem {
            Some(line) => format!("Nuitka 退出码 {code}：{}", line.trim()),
            None => format!("Nuitka 退出码 {code}"),
        };
        Ok((false, detail))
    }
}

/// 按字节读行再宽松解码：遇到非 UTF-8 的行不能让整条管道断掉。
fn pump<R: Read + Send + 'static>(stream: R, sender: Sender<String>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut buffer: Vec<u8> = Vec::new();
        loop {
            buffer.clear();
            match reader.read_until(b'\n', &mut buffer) {
                Ok(0) => break,
                Ok(_) => {
                    let line = decode_line(&buffer);
                    if sender.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    })
}

fn decode_line(bytes: &[u8]) -> String {
    let mut slice = bytes;
    while let Some(last) = slice.last() {
        if *last == b'\n' || *last == b'\r' {
            slice = &slice[..slice.len() - 1];
        } else {
            break;
        }
    }
    String::from_utf8_lossy(slice).to_string()
}

/// 取消正在跑的打包（连子进程一起杀）。
pub(crate) fn cancel() -> Result<(), String> {    let pid = RUNNING
        .lock()
        .ok()
        .and_then(|mut slot| slot.take())
        .ok_or_else(|| "当前没有正在进行的打包".to_string())?;

    let status = crate::util::command("taskkill.exe")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("无法结束进程：{error}"))?;
    log::info(&format!("取消打包，pid {pid}，taskkill 结果 {}", status.success()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Request {
        Request {
            python: r"C:\py\python.exe".to_string(),
            script: r"D:\work\main.py".to_string(),
            out_dir: r"D:\out dir".to_string(),
            name: "demo".to_string(),
            icon: String::new(),
            onefile: true,
            console: false,
            show_progress: true,
            download: true,
            compiler: "msvc:14.50".to_string(),
            extra: "--follow-imports".to_string(),
        }
    }

    #[test]
    fn msvc_is_requested_with_exact_version() {
        let args = arguments(&sample());
        assert!(args.contains(&"--msvc=14.50".to_string()));
        assert!(!args.contains(&"--mingw64".to_string()));
    }

    #[test]
    fn bare_msvc_falls_back_to_latest() {
        let mut request = sample();
        request.compiler = "msvc".to_string();
        let args = arguments(&request);
        assert!(args.contains(&"--msvc=latest".to_string()));
    }

    #[test]
    fn gcc_backend_is_requested_explicitly() {
        let mut request = sample();
        request.compiler = "gcc".to_string();
        let args = arguments(&request);
        assert!(args.contains(&"--mingw64".to_string()));
        assert!(!args.iter().any(|item| item.starts_with("--msvc=")));
    }

    #[test]
    fn unknown_compiler_adds_no_flag() {
        let mut request = sample();
        request.compiler = String::new();
        let args = arguments(&request);
        assert!(!args.iter().any(|item| item.starts_with("--msvc=")));
        assert!(!args.contains(&"--mingw64".to_string()));
    }

    #[test]
    fn builds_expected_arguments() {
        let args = arguments(&sample());
        assert_eq!(args[0], "-m");
        assert_eq!(args[1], "nuitka");
        assert!(args.contains(&"--onefile".to_string()));
        assert!(args.contains(&"--windows-console-mode=disable".to_string()));
        assert!(args.contains(&"--show-progress".to_string()));
        assert!(args.contains(&"--assume-yes-for-downloads".to_string()));
        assert!(args.contains(&"--output-dir=D:\\out dir".to_string()));
        assert!(args.contains(&"--output-filename=demo".to_string()));
        assert!(args.contains(&"--follow-imports".to_string()));
        assert_eq!(args.last().unwrap(), r"D:\work\main.py");
    }

    #[test]
    fn standalone_uses_standalone_flag() {
        let mut request = sample();
        request.onefile = false;
        let args = arguments(&request);
        assert!(args.contains(&"--standalone".to_string()));
        assert!(!args.contains(&"--onefile".to_string()));
    }

    #[test]
    fn console_flag_follows_request() {
        let mut request = sample();
        request.console = true;
        assert!(arguments(&request).contains(&"--windows-console-mode=force".to_string()));
    }

    #[test]
    fn command_line_quotes_paths_with_spaces() {
        let line = command_line(&sample());
        assert!(line.contains("\"D:\\out dir\"") || line.contains("--output-dir=\"D:\\out dir\""));
        assert!(line.contains("--windows-console-mode=disable"));
    }

    #[test]
    fn target_follows_output_mode() {
        let out = Path::new(r"D:\out");
        assert_eq!(
            target_of(&sample(), out).unwrap(),
            PathBuf::from(r"D:\out\demo.exe")
        );
        let mut request = sample();
        request.onefile = false;
        assert_eq!(
            target_of(&request, out).unwrap(),
            PathBuf::from(r"D:\out\demo.dist")
        );
    }

    #[test]
    fn target_falls_back_to_script_stem() {
        let mut request = sample();
        request.name = String::new();
        assert_eq!(
            target_of(&request, Path::new(r"D:\out")).unwrap(),
            PathBuf::from(r"D:\out\main.exe")
        );
    }

    #[test]
    fn missing_script_is_rejected() {
        let mut request = sample();
        request.script = r"D:\definitely\missing.py".to_string();
        let result = run(&request, |_| {});
        assert!(result.is_err());
    }

    #[test]
    fn decode_line_keeps_going_on_non_utf8() {
        // GBK 编码的「玩」
        let bytes: Vec<u8> = vec![0xCD, 0xE6, b'.', b'p', b'y', b'\n'];
        let line = decode_line(&bytes);
        assert!(line.ends_with(".py"));
        assert!(!line.contains('\n'));
    }

    #[test]
    fn decode_line_trims_crlf() {
        assert_eq!(decode_line(b"hello\r\n"), "hello");
        assert_eq!(decode_line(b"hello"), "hello");
    }

    #[test]
    fn decode_line_handles_utf8() {
        assert_eq!(decode_line("玩.py".as_bytes()), "玩.py");
    }

    #[test]
    fn tracker_maps_modules_to_percent() {
        let mut tracker = Tracker::default();
        // 100 个模块：剩 99 个 → 完成 1 个
        let first = tracker
            .feed("Nuitka-Progress: Optimizing module '__main__', 99 more modules to go after that.")
            .expect("progress");
        assert_eq!(first.percent, 9);
        let last = tracker
            .feed("Nuitka-Progress: Optimizing module 'x', 2 more modules to go after that.")
            .expect("progress");
        assert_eq!(last.percent, 57);
    }

    #[test]
    fn tracker_recognizes_milestones() {
        let mut tracker = Tracker::default();
        assert_eq!(
            tracker.feed("Nuitka: Starting Python compilation with:").unwrap().percent,
            4
        );
        assert_eq!(
            tracker
                .feed("Nuitka: Completed Python level compilation and optimization.")
                .unwrap()
                .percent,
            62
        );
        assert_eq!(
            tracker.feed("Nuitka: Running C compilation via Scons.").unwrap().percent,
            76
        );
        assert!(tracker.feed("something unrelated").is_none());
    }

    #[test]
    fn tracker_progress_never_regresses_below_milestones() {
        let mut tracker = Tracker::default();
        let a = tracker
            .feed("Nuitka-Progress: Optimizing module 'a', 10 more modules to go after that.")
            .unwrap();
        let b = tracker.feed("Nuitka: Generating source code for C backend compiler.").unwrap();
        assert!(b.percent > a.percent);
    }

    #[test]
    fn second_pass_does_not_move_progress_backwards() {
        let mut tracker = Tracker::default();
        // 第一遍快结束：剩 2 个模块
        let first = tracker
            .feed("Nuitka-Progress: Optimizing module 'a', 2 more modules to go after that.")
            .unwrap();
        assert!(first.percent > 8);
        // 第二遍重新从一大堆模块开始：必须被丢掉
        assert!(
            tracker
                .feed("Nuitka-Progress: Optimizing module 'b', 300 more modules to go after that.")
                .is_none()
        );
    }

    #[test]
    fn warning_lines_do_not_move_progress() {
        let mut tracker = Tracker::default();
        assert!(
            tracker
                .feed("Nuitka-Onefile:WARNING: Onefile mode cannot compress without 'zstandard'")
                .is_none()
        );
        assert!(tracker.feed("Nuitka:ERROR: something broke").is_none());
    }

    #[test]
    fn onefile_milestone_needs_the_real_stage_line() {
        let mut tracker = Tracker::default();
        tracker.feed("Nuitka: Running C compilation via Scons.");
        let done = tracker.feed("Nuitka-Scons: Onefile C compiler: cl (cl 14.50).").unwrap();
        assert_eq!(done.percent, 92);
    }

    #[test]
    fn single_module_passes_are_ignored() {
        let mut tracker = Tracker::default();
        assert!(
            tracker
                .feed("Nuitka-Progress: Optimizing module '__main__', 0 more modules to go after that.")
                .is_none()
        );
    }

    #[test]
    fn repeated_stage_reports_are_deduplicated() {
        let mut tracker = Tracker::default();
        assert!(tracker.feed("Nuitka: Running C compilation via Scons.").is_some());
        assert!(tracker.feed("Nuitka-Scons: Running C compilation via Scons.").is_none());
    }

    #[test]
    fn notice_is_recognized_from_the_real_log_line() {
        // 原样取自 nuitka-rs.log，Nuitka 的换行是硬折行，这里拼成一行
        let line = "Nuitka-Onefile:WARNING: Onefile mode cannot compress without 'zstandard' \
package installed You probably should depend on 'Nuitka[onefile]' rather than 'Nuitka' \
which among other things depends on it.";
        let notice = notice_for(line).expect("必须认出这条警告");
        assert!(notice.contains("没有压缩"));
    }

    #[test]
    fn unrelated_warnings_stay_out_of_notices() {
        assert!(notice_for("scons: warning: MSVC version '14.50' was not found.").is_none());
        assert!(notice_for("Nuitka:ERROR: something broke").is_none());
    }

    #[test]
    fn outcome_exposes_notices_to_the_ui() {
        let outcome = Outcome {
            ok: true,
            target: "demo.exe".to_string(),
            seconds: 1,
            detail: String::new(),
            notices: vec![
                notice_for("Onefile mode cannot compress without 'zstandard'")
                    .unwrap()
                    .to_string(),
            ],
        };
        let text = serde_json::to_string(&outcome).expect("序列化");
        assert!(text.contains("\"notices\""));
        assert!(text.contains("没有压缩"));
    }

    #[test]
    fn memory_shortfall_caps_compression_jobs() {
        // 2.5 GB 可用、16 核：只敢开 4 个压缩线程
        assert_eq!(jobs_for(2500, 16), Some(4));
        // 内存充足就别干预 Nuitka 的默认
        assert_eq!(jobs_for(16000, 16), None);
        // 再紧也得留一个线程，不能算成 0
        assert_eq!(jobs_for(100, 16), Some(1));
    }

    /// 真打一次包，只有本机装好环境才跑得起来：
    /// `cargo test real_build -- --ignored --nocapture`
    #[test]
    #[ignore = "会真的编译一个 exe，需要本机环境"]
    fn real_build_produces_exe() {
        use super::super::paths;

        let base = paths::base().expect("安装目录");
        let python = base.join("python").join("3.12").join("python.exe");
        assert!(python.is_file(), "缺少 {}", python.display());

        let workspace = base.join("build-test");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let script = workspace.join("main.py");
        std::fs::write(&script, "print('hello from nuitka++')\n").expect("script");
        let out = workspace.join("out");

        let request = Request {
            python: python.display().to_string(),
            script: script.display().to_string(),
            out_dir: out.display().to_string(),
            name: "demo".to_string(),
            icon: String::new(),
            onefile: true,
            console: true,
            show_progress: true,
            download: true,
            compiler: "msvc:14.50".to_string(),
            extra: String::new(),
        };

        let mut seen = 0usize;
        let mut tracker = Tracker::default();
        let mut steps: Vec<u8> = Vec::new();
        let outcome = run(&request, |line| {
            seen += 1;
            if seen <= 40 && line.contains("Nuitka") {
                println!("RAW {:?}", line);
            }
            if let Some(progress) = tracker.feed(line) {
                println!("进度 {}%（{}）", progress.percent, progress.stage);
                steps.push(progress.percent);
            }
        })
        .expect("run");

        println!("结果：{outcome:?}，输出 {seen} 行，进度点 {steps:?}");
        assert!(steps.windows(2).all(|pair| pair[0] < pair[1]), "进度不能倒退");
        assert!(outcome.ok, "{}", outcome.detail);
        assert!(Path::new(&outcome.target).is_file(), "产物不存在");
    }

    /// 中文脚本名 + 图标，复现用户那条命令：
    /// `cargo test real_build_chinese -- --ignored --nocapture`
    #[test]
    #[ignore = "会真的编译一个 exe，需要本机环境"]
    fn real_build_with_chinese_name() {
        use super::super::paths;

        let base = paths::base().expect("安装目录");
        let python = base.join("python").join("3.12").join("python.exe");
        assert!(python.is_file(), "缺少 {}", python.display());

        let workspace = base.join("build-test");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let script = workspace.join("玩.py");
        std::fs::write(&script, "print('中文名脚本跑起来了')\n").expect("script");

        let icon = PathBuf::from(r"D:\Hex\project\Nuitka++\Nuitka++.ico");
        assert!(icon.is_file(), "缺少图标 {}", icon.display());

        let request = Request {
            python: python.display().to_string(),
            script: script.display().to_string(),
            out_dir: workspace.join("out-cn").display().to_string(),
            name: "玩".to_string(),
            icon: icon.display().to_string(),
            onefile: true,
            console: true,
            show_progress: true,
            download: true,
            compiler: "msvc:14.50".to_string(),
            extra: String::new(),
        };

        let outcome = run(&request, |line| println!("{line}")).expect("run");
        println!("结果：{outcome:?}");
        assert!(outcome.ok, "{}", outcome.detail);
        assert!(Path::new(&outcome.target).is_file(), "产物不存在");
    }
}
