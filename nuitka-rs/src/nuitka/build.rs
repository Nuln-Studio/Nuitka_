use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use super::python_discovery::PyVersion;
use super::{log, toolchain};

/// 同一时间只允许一个打包任务，取消时要用这里的 pid。
///
/// 槽里存的是「任务序号 + pid」，不是光存 pid：用户取消后可能立刻开下一个任务，
/// 旧任务收尾时如果直接把槽清空，会把新任务的 pid 抹掉，导致取消按钮失效、
/// 甚至能再开第三个任务并行跑。序号保证每个任务只动自己的槽。
static RUNNING: Mutex<Option<Task>> = Mutex::new(None);
static NEXT_TASK: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Task {
    id: u64,
    pid: u32,
}

/// 一次 `execute` 的结果。`cancelled` 单独拿出来，
/// 因为被杀掉的进程和编译失败在退出码上长得一模一样。
struct RunResult {
    ok: bool,
    failure: String,
    cancelled: bool,
}

fn next_task_id() -> u64 {
    NEXT_TASK.fetch_add(1, Ordering::Relaxed)
}

fn register_task(id: u64, pid: u32) {
    if let Ok(mut slot) = RUNNING.lock() {
        *slot = Some(Task { id, pid });
    }
}

/// 只有槽里还是自己的序号时才清空，别的任务已经接管就什么都不做。
fn clear_task(id: u64) {
    if let Ok(mut slot) = RUNNING.lock()
        && (*slot).map(|task| task.id) == Some(id)
    {
        *slot = None;
    }
}

fn take_task() -> Option<Task> {
    RUNNING.lock().ok().and_then(|mut slot| slot.take())
}

fn running() -> bool {
    RUNNING.lock().map(|slot| slot.is_some()).unwrap_or(false)
}

/// 被用户取消的任务 id。子进程被 taskkill 杀掉后只能拿到「退出码非零」，
/// 光看退出码分不清「编译失败」和「用户不要了」，这里单独记一笔。
static CANCELLED: Mutex<Option<u64>> = Mutex::new(None);

fn mark_cancelled(id: u64) {
    if let Ok(mut slot) = CANCELLED.lock() {
        *slot = Some(id);
    }
}

fn was_cancelled(id: u64) -> bool {
    CANCELLED
        .lock()
        .map(|slot| *slot == Some(id))
        .unwrap_or(false)
}

fn clear_cancelled(id: u64) {
    if let Ok(mut slot) = CANCELLED.lock()
        && *slot == Some(id)
    {
        *slot = None;
    }
}


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
    /// 打包完试运行一次产物，用来抓「编译成功但一启动就缺包」这类问题
    #[serde(default)]
    pub smoke: bool,
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
    arguments_for(request, available_memory_mb())
}

/// 内存作为参数传进来，测试才能固定住「内存多」「内存少」两种世界。
fn arguments_for(request: &Request, free_mb: Option<u64>) -> Vec<String> {
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
    if request.onefile {
        let already_off = request.extra.contains("--onefile-no-compression");
        if compression_unaffordable_for(free_mb) && !already_off {
            // 与其跑到 96% 再摊一句 Allocation error，不如一开始就不压
            args.push("--onefile-no-compression".into());
        } else if let Some(jobs) = free_mb.and_then(|free| jobs_for(free, cores())) {
            args.push(format!("--jobs={jobs}"));
        }
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
    args.extend(extra_tokens(&request.extra));
    args.push(request.script.clone());
    args
}

/// 按空白切附加参数，但 `"` 包起来的部分算一个整体。
///
/// 直接 `split_whitespace` 会把 `--include-data-dir=D:\我的 数据=data` 切成两段，
/// Nuitka 于是收到两个错参数。Windows 上带空格的路径太常见了。
fn extra_tokens(extra: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut started = false;

    for ch in extra.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                started = true;
            }
            ch if ch.is_whitespace() && !in_quotes => {
                if started {
                    tokens.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            ch => {
                current.push(ch);
                started = true;
            }
        }
    }
    if started {
        tokens.push(current);
    }
    tokens
}

/// 只有失败信息落在编译器身上，才值得换后端再编一遍。
///
/// 反例：脚本语法错、缺模块、路径写错——这些换 gcc 也一样失败，
/// 白白多等一整轮完整编译，用户最后看到的还是同一句报错。
fn is_compiler_failure(detail: &str) -> bool {
    const HINTS: &[&str] = &[
        "cl.exe",
        "link.exe",
        "msvc",
        "vcvars",
        "visual studio",
        "工具集",
    ];
    let lower = detail.to_lowercase();
    HINTS.iter().any(|hint| lower.contains(hint))
}

/// 一行输出算不算「值得给用户看的报错」。
///
/// 只认大写的 `FATAL` / `ERROR` 是不够的：Nuitka 最后崩在 Python 异常里时，
/// 真正的原因行长这样——`zstandard.backend_c.ZstdError: zstd compress error: ...`，
/// 里面没有任何大写关键字，于是被漏掉，界面只剩一句「退出码 1」。
fn is_problem_line(line: &str) -> bool {
    if line.contains("FATAL") || line.contains("ERROR") {
        return true;
    }
    let lower = line.to_lowercase();
    lower.contains("error:") || lower.contains("traceback (most recent call last)")
}

/// 单文件压缩阶段内存不够。这不是打包失败，只是压缩器要一次性申请一大块内存没排到。
fn is_compression_oom(detail: &str) -> bool {
    let lower = detail.to_lowercase();
    (lower.contains("zstd") || lower.contains("zstandard"))
        && (lower.contains("allocation")
            || lower.contains("not enough memory")
            || lower.contains("out of memory")
            || lower.contains("内存不足"))
}

/// 产物试运行的观察窗口。单文件产物要先自解压，几秒钟都算正常。
const SMOKE_SECONDS: u64 = 8;

enum Smoke {
    /// 起来了（包括还在跑=GUI 程序启动成功）
    Started,
    /// 一启动就死在缺包上
    Missing(Vec<String>),
}

/// 真跑一次产物。这是唯一能抓「动态导入缺包」的手段：
/// 编译时 Nuitka 只跟静态 import 图，`importlib.import_module("yaml")` 它看不见。
fn smoke_run(target: &str) -> Result<Smoke, String> {
    let mut command = crate::util::command(target);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("无法启动产物：{error}"))?;

    let sink = Arc::new(Mutex::new(String::new()));
    let mut readers = Vec::new();
    if let Some(stream) = child.stdout.take() {
        readers.push(collect_text(stream, sink.clone()));
    }
    if let Some(stream) = child.stderr.take() {
        readers.push(collect_text(stream, sink.clone()));
    }

    let deadline = Instant::now() + Duration::from_secs(SMOKE_SECONDS);
    let mut status = None;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(finished)) => {
                status = Some(finished);
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(150)),
            Err(error) => return Err(format!("无法读取产物状态：{error}")),
        }
    }
    // 到了时限还在跑，说明程序活着；不能把它留在后台挡着用户的文件
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    for reader in readers {
        let _ = reader.join();
    }

    let text = sink.lock().map(|text| text.clone()).unwrap_or_default();
    crate::nuitka::log::write(&format!(
        "产物试运行：{}",
        text.lines().take(20).collect::<Vec<_>>().join(" / ")
    ));

    match status {
        Some(finished) if !finished.success() => {
            let modules = missing_modules(&text);
            if modules.is_empty() {
                // 退出了但看不出缺包：可能是需要命令行参数、或者缺少非 Python 运行库，
                // 这种情况不该把一次成功的编译判成失败
                Ok(Smoke::Started)
            } else {
                Ok(Smoke::Missing(modules))
            }
        }
        _ => Ok(Smoke::Started),
    }
}

/// 从产物输出里抠出 `No module named 'xxx'` 的模块名，顶层去重。
fn missing_modules(text: &str) -> Vec<String> {
    const MARK: &str = "No module named ";
    let mut found: Vec<String> = Vec::new();
    for line in text.lines() {
        let Some(at) = line.find(MARK) else { continue };
        let rest = &line[at + MARK.len()..];
        // 报错文本里模块名一般带引号：No module named 'yaml'
        let rest = rest.trim_start_matches(['\'', '"']);
        let name: String = rest
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '.')
            .collect();
        let top = name.split('.').next().unwrap_or("").to_string();
        if !top.is_empty() && !found.contains(&top) {
            found.push(top);
        }
    }
    found
}

/// 读子进程的每一行，攒进一个共享字符串。
fn collect_text<R: Read + Send + 'static>(stream: R, sink: Arc<Mutex<String>>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut buffer: Vec<u8> = Vec::new();
        loop {
            buffer.clear();
            match reader.read_until(b'\n', &mut buffer) {
                Ok(0) => break,
                Ok(_) => {
                    let line = decode_line(&buffer);
                    if let Ok(mut text) = sink.lock() {
                        text.push_str(&line);
                        text.push('\n');
                    }
                }
                Err(_) => break,
            }
        }
    })
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

/// 低于这个可用内存就干脆不压缩。实测 1 GB 可用内存在 zstd 高压缩级别下也会失败，
/// 所以门槛按「能跑起来」留足余量，不按「理论上够」。
const COMPRESSION_MIN_MB: u64 = 1500;

/// 当前机器的内存够不够跑一轮单文件压缩。
fn compression_unaffordable() -> bool {
    available_memory_mb()
        .map(|free| free < COMPRESSION_MIN_MB)
        .unwrap_or(false)
}

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

fn cores() -> usize {
    std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
}

fn jobs_for(free_mb: u64, cores: usize) -> Option<usize> {
    let affordable = (free_mb / COMPRESSION_WORKER_MB).max(1) as usize;
    (affordable < cores).then_some(affordable)
}

/// 内存够不够跑一轮压缩。拿不到内存数据时按「够」处理：不能因为读不到就替用户决定不压。
fn compression_unaffordable_for(free_mb: Option<u64>) -> bool {
    free_mb.map(|free| free < COMPRESSION_MIN_MB).unwrap_or(false)
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

    if running() {
        return Err("已经有一个打包任务在跑".to_string());
    }

    let version = python_version(&python);
    // 3.13 起 --mingw64 会被 Nuitka 直接拒绝，别等编译到一半才炸
    if request.compiler == "gcc" && !version.map(toolchain::mingw_ok).unwrap_or(true) {
        return Err(
            "Python 3.13 起不能用 winlibs gcc（Nuitka 不支持 --mingw64），请改用 MSVC".to_string(),
        );
    }
    let tool = version.and_then(toolchain::resolve);
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

    // 内存不够时 arguments() 已经自己加了 --onefile-no-compression，
    // 这里把原因说给用户听，否则他只会觉得「怎么这次没压缩」
    if request.onefile
        && compression_unaffordable()
        && !request.extra.contains("--onefile-no-compression")
    {
        let free = available_memory_mb().unwrap_or(0);
        let notice = format!(
            "这次单文件没有压缩：打包时可用内存只有 {free} MB，压缩器起不来（会直接 Allocation error）。\
功能不受影响，但体积会明显偏大。关掉一些程序再打包就能恢复压缩。"
        );
        on_line(&format!("[内存] {}", notice));
        notices.push(notice);
    }

    let mut result = execute(&python, &args, tool.as_ref(), &mut notices, &mut on_line)?;

    if result.cancelled {
        let seconds = started.elapsed().as_secs();
        log::info(&format!("打包已取消（{seconds} 秒）"));
        return Ok(Outcome {
            ok: false,
            target: String::new(),
            seconds,
            detail: "已取消".to_string(),
            notices,
        });
    }

    // MSVC 认不出来时（新版本 VS 很常见），退到 Nuitka 自带的 winlibs 再试一次。
    // 前提是失败真出在编译器上：脚本自己写错了，换后端也是白跑。
    if !result.ok
        && request.compiler.starts_with("msvc")
        && is_compiler_failure(&result.failure)
        && version.map(toolchain::mingw_ok).unwrap_or(true)
        && let Some(gcc) = toolchain::fallback_gcc()
    {
        on_line("[重试] MSVC 编译失败，改用 winlibs gcc 重新编译");
        log::info(&format!("MSVC 失败，改用 {} 重试", gcc.describe()));
        let mut retry = request.clone();
        retry.compiler = "gcc".to_string();
        args = arguments(&retry);
        on_line(&format!("[命令] {}", command_line(&retry)));
        result = execute(&python, &args, Some(&gcc), &mut notices, &mut on_line)?;
        if result.cancelled {
            let seconds = started.elapsed().as_secs();
            log::info(&format!("打包已取消（{seconds} 秒）"));
            return Ok(Outcome {
                ok: false,
                target: String::new(),
                seconds,
                detail: "已取消".to_string(),
                notices,
            });
        }
    }

    // 压缩阶段内存不够导致失败：这不是打包失败，去掉压缩重跑一次就能出 exe。
    // 内存占用量是动态的，预判可能有偏差，所以运行期这次兜底必须留着。
    if !result.ok
        && request.onefile
        && is_compression_oom(&result.failure)
        && !args.iter().any(|arg| arg == "--onefile-no-compression")
    {
        on_line("[重试] 压缩器内存不足，改成不压缩重新打包");
        log::info("压缩阶段内存不足，去掉 --onefile 压缩后重试");
        args.push("--onefile-no-compression".to_string());
        let notice = "这次单文件没有压缩：压缩器申请内存失败（打包时内存不够）。功能不受影响，\
但体积会明显偏大。关掉一些程序再打包就能恢复压缩。"
            .to_string();
        on_line(&format!("[内存] {notice}"));
        if !notices.contains(&notice) {
            notices.push(notice);
        }
        result = execute(&python, &args, tool.as_ref(), &mut notices, &mut on_line)?;
        if result.cancelled {
            let seconds = started.elapsed().as_secs();
            log::info(&format!("打包已取消（{seconds} 秒）"));
            return Ok(Outcome {
                ok: false,
                target: String::new(),
                seconds,
                detail: "已取消".to_string(),
                notices,
            });
        }
    }

    for notice in &notices {
        log::info(&format!("提示：{notice}"));
    }

    let seconds = started.elapsed().as_secs();
    let target = target_of(request, &out_dir)
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let produced = !target.is_empty() && Path::new(&target).exists();

    // 编译通过不等于产物能用：动态导入的库在编译阶段是隐形的，
    // 只有真跑一次才知道。窗口用户可以在设置里关掉，因为 GUI 程序会闪一下。
    if result.ok && produced && request.smoke {
        on_line(&format!("[自检] 试运行产物，最多 {SMOKE_SECONDS} 秒"));
        match smoke_run(&target)? {
            Smoke::Started => on_line("[自检] 产物启动正常"),
            Smoke::Missing(modules) => {
                let packages: Vec<String> =
                    modules.iter().map(|name| crate::nuitka::deps::pip_name(name)).collect();
                let python = clean(&request.python).unwrap_or("python");
                let detail = format!(
                    "打包完成，但产物一启动就失败：缺少 {}。\n\
这类依赖是动态导入的，打包前的静态检查看不到。\n装法：\"{python}\" -m pip install {}",
                    modules.join("、"),
                    packages.join(" ")
                );
                log::error(&format!("产物试运行失败：{}", modules.join("、")));
                return Ok(Outcome {
                    ok: false,
                    target,
                    seconds,
                    detail,
                    notices,
                });
            }
        }
    }

    if result.ok && produced {
        log::info(&format!("打包完成：{target}（{seconds} 秒）"));
        Ok(Outcome {
            ok: true,
            target,
            seconds,
            detail: format!("完成，用时 {seconds} 秒"),
            notices,
        })
    } else {
        let detail = if result.ok {
            format!("编译结束但没有找到产物：{target}")
        } else {
            result.failure
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

/// 起一个 Nuitka 进程，把输出逐行吐出去。
fn execute(
    python: &Path,
    args: &[String],
    tool: Option<&toolchain::Toolchain>,
    notices: &mut Vec<String>,
    on_line: &mut dyn FnMut(&str),
) -> Result<RunResult, String> {
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

    let task_id = next_task_id();
    let mut child = command
        .spawn()
        .map_err(|error| format!("无法启动 Nuitka：{error}"))?;
    register_task(task_id, child.id());

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
    // 兜底用：万一整段输出里没有一行像样的错误，至少把最后一句有用的话带进弹窗，
    // 总比只给用户一个「退出码 1」强
    let mut last_line: Option<String> = None;
    for line in receiver {
        if let Some(notice) = notice_for(&line)
            && !notices.iter().any(|item| item == notice)
        {
            notices.push(notice.to_string());
        }
        if is_problem_line(&line) {
            last_problem = Some(line.clone());
        }
        let trimmed = line.trim();
        if !trimmed.is_empty() && !trimmed.starts_with("Nuitka-Reports") {
            last_line = Some(trimmed.to_string());
        }
        on_line(&line);
    }
    for reader in readers {
        let _ = reader.join();
    }

    let status = child.wait().map_err(|error| error.to_string())?;
    let cancelled = was_cancelled(task_id);
    clear_cancelled(task_id);
    clear_task(task_id);

    if status.success() {
        Ok(RunResult {
            ok: true,
            failure: String::new(),
            cancelled,
        })
    } else {
        let code = status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "未知".to_string());
        let detail = match last_problem.or(last_line) {
            Some(line) => format!("Nuitka 退出码 {code}：{}", line.trim()),
            None => format!("Nuitka 退出码 {code}"),
        };
        Ok(RunResult {
            ok: false,
            failure: detail,
            cancelled,
        })
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
pub(crate) fn cancel() -> Result<(), String> {
    let task = take_task().ok_or_else(|| "当前没有正在进行的打包".to_string())?;
    mark_cancelled(task.id);

    let status = crate::util::command("taskkill.exe")
        .args(["/PID", &task.pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("无法结束进程：{error}"))?;
    log::info(&format!(
        "取消打包，任务 {}，pid {}，taskkill 结果 {}",
        task.id,
        task.pid,
        status.success()
    ));
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
            smoke: false,
            compiler: "msvc:14.50".to_string(),
            extra: "--follow-imports".to_string(),
        }
    }

    /// P0-1 回归：取消后立刻开新任务，旧任务收尾不能抹掉新任务的 pid。
    #[test]
    fn stale_task_cannot_clear_the_new_one() {
        let first = next_task_id();
        register_task(first, 111);
        assert_eq!(take_task(), Some(Task { id: first, pid: 111 })); // cancel 取走旧任务

        let second = next_task_id();
        register_task(second, 222);

        clear_task(first); // 旧任务这时才收尾
        assert_eq!(
            take_task(),
            Some(Task {
                id: second,
                pid: 222
            }),
            "旧任务把新任务的槽清掉了，取消按钮会失效"
        );

        // 正常收尾：自己清自己的
        register_task(second, 222);
        clear_task(second);
        assert!(!running());
    }

    /// P0-2 回归：换后端重试必须由「编译器相关」的失败触发。
    #[test]
    fn only_compiler_failures_trigger_the_gcc_retry() {
        // 真的出在 cl.exe 上：值得换 gcc 再编一遍
        assert!(is_compiler_failure(
            "Nuitka 退出码 2：FATAL: Visual C++ compiler 'cl.exe' was not found on this system"
        ));
        assert!(is_compiler_failure("Nuitka 退出码 1：vcvarsall.bat 返回 1"));
        assert!(is_compiler_failure("MSVC 工具集 14.50 未安装"));

        // 用户自己的脚本写错了：换 gcc 也一样失败，不该重试
        assert!(!is_compiler_failure(
            "Nuitka 退出码 1：FATAL: Error, syntax error in module 'main'"
        ));
        assert!(!is_compiler_failure(
            "Nuitka 退出码 1：FATAL: Error, cannot find 'requests' module"
        ));
        assert!(!is_compiler_failure("Nuitka 退出码 1：文件不存在：D:\\work\\main.py"));
        assert!(!is_compiler_failure(""));
    }

    /// P1-1 回归：带空格的路径不能被空格切断。
    #[test]
    fn extra_args_keep_quoted_paths_whole() {
        let mut request = sample();
        request.extra = "--include-data-dir=\"D:\\我的 数据=data\" --follow-imports".to_string();
        let args = arguments(&request);
        assert!(
            args.contains(&"--include-data-dir=D:\\我的 数据=data".to_string()),
            "引号内的空格被切开了：{args:?}"
        );
        assert!(args.contains(&"--follow-imports".to_string()));
        assert_eq!(
            args.iter().filter(|arg| arg.contains("数据")).count(),
            1,
            "带空格的那条路径被拆成两段"
        );
    }

    #[test]
    fn extra_args_split_on_plain_whitespace() {
        assert_eq!(
            extra_tokens("  --a   --b=c \t"),
            vec!["--a".to_string(), "--b=c".to_string()]
        );
        assert!(extra_tokens("   ").is_empty());
        assert!(extra_tokens("").is_empty());
    }

    /// 报错行不能只看大写关键字：zstd 崩在 Python 异常里时没有 FATAL/ERROR。
    #[test]
    fn problem_lines_include_python_exceptions() {
        assert!(is_problem_line(
            "zstandard.backend_c.ZstdError: zstd compress error: Allocation error : not enough memory"
        ));
        assert!(is_problem_line("FATAL: Error, no 'cl.exe' found"));
        assert!(is_problem_line("Traceback (most recent call last):"));
        assert!(!is_problem_line("scons: warning: MSVC version '14.50' was not found."));
        assert!(!is_problem_line("Nuitka-Scons: Compiled 15 C files using clcache"));
    }

    #[test]
    fn zstd_allocation_failure_is_recognised() {
        assert!(is_compression_oom(
            "Nuitka 退出码 1：zstandard.backend_c.ZstdError: zstd compress error: Allocation error : not enough memory"
        ));
        assert!(is_compression_oom("ZstdError: out of memory"));
        assert!(!is_compression_oom("Nuitka 退出码 1：FATAL: Error, syntax error in module 'main'"));
        assert!(!is_compression_oom("Nuitka 退出码 1：zstd 版本过旧"));
    }

    /// 内存紧张时应该一开始就不压缩，内存宽裕时才按核数压线程。
    #[test]
    fn low_memory_disables_onefile_compression() {
        let tight = arguments_for(&sample(), Some(900));
        assert!(tight.contains(&"--onefile-no-compression".to_string()));
        assert!(!tight.iter().any(|arg| arg.starts_with("--jobs=")));

        let plenty = arguments_for(&sample(), Some(8000));
        assert!(!plenty.contains(&"--onefile-no-compression".to_string()));

        // 读不到内存数据时不能擅自替用户决定不压
        let unknown = arguments_for(&sample(), None);
        assert!(!unknown.contains(&"--onefile-no-compression".to_string()));
    }

    /// 试运行产物时要从输出里认出缺的是哪个模块。
    #[test]
    fn missing_modules_are_extracted_from_product_output() {
        let text = "Traceback (most recent call last):\n\
  File \"main.py\", line 3, in <module>\n\
ModuleNotFoundError: No module named 'yaml'\n\
ModuleNotFoundError: No module named 'yaml.safe_load'\n";
        assert_eq!(missing_modules(text), vec!["yaml".to_string()]);
        assert!(missing_modules("启动主程序失败：No module named 'requests'").contains(&"requests".to_string()));
        assert!(missing_modules("一切正常，服务已启动").is_empty());
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
            smoke: false,
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
            smoke: false,
            compiler: "msvc:14.50".to_string(),
            extra: String::new(),
        };

        let outcome = run(&request, |line| println!("{line}")).expect("run");
        println!("结果：{outcome:?}");
        assert!(outcome.ok, "{}", outcome.detail);
        assert!(Path::new(&outcome.target).is_file(), "产物不存在");
    }
}
