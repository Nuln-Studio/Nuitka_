use tauri::{AppHandle, Emitter, Manager};

use crate::nuitka;
use crate::nuitka::bootstrap::{
    EnvPlan, PythonOptions, plan, python_options as build_python_options,
};
use crate::nuitka::build;
use crate::nuitka::settings::{self as config, Form, Imported, Loaded, Settings};
use crate::nuitka::{EnvStatus, Report};

#[tauri::command]
async fn scan() -> Result<Report, String> {
    tauri::async_runtime::spawn_blocking(nuitka::report)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn env_status() -> Result<EnvStatus, String> {
    tauri::async_runtime::spawn_blocking(nuitka::status)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn pick_dir(window: tauri::WebviewWindow, title: String) -> Option<String> {
    nuitka::dialog::pick_folder(hwnd_of(&window), &title)
}

#[tauri::command]
fn pick_script(window: tauri::WebviewWindow, title: String) -> Option<String> {
    nuitka::dialog::pick_file(hwnd_of(&window), &title, "Python 脚本", "*.py")
}

#[tauri::command]
fn pick_icon(window: tauri::WebviewWindow, title: String) -> Option<String> {
    nuitka::dialog::pick_file(hwnd_of(&window), &title, "图标文件", "*.ico")
}

#[tauri::command]
fn pick_python(window: tauri::WebviewWindow, title: String) -> Option<String> {
    nuitka::dialog::pick_file(hwnd_of(&window), &title, "可执行文件", "*.exe")
}

#[tauri::command]
fn read_config() -> Loaded {
    config::load()
}

#[tauri::command]
fn write_settings(payload: Settings) -> Result<(), String> {
    config::save_settings(&payload)
}

#[tauri::command]
fn write_form(form: Form) -> Result<(), String> {
    config::save_form(&form)
}

/// 导出配置：带绝对路径、Python 版本和编译器。
#[tauri::command]
fn export_config(
    window: tauri::WebviewWindow,
    form: Form,
    python: String,
    compiler: String,
) -> Result<Option<String>, String> {
    let stem = std::path::Path::new(&form.script)
        .file_stem()
        .map(|item| item.to_string_lossy().to_string())
        .filter(|item| !item.trim().is_empty())
        .unwrap_or_else(|| "nuitka-rs".to_string());
    let default_name = format!("{stem}-配置.json");
    let Some(path) = nuitka::dialog::pick_save_file(hwnd_of(&window), "导出配置", &default_name)
    else {
        return Ok(None);
    };
    config::export_to(std::path::Path::new(&path), &form, &python, &compiler)?;
    nuitka::log::write(&format!("已导出配置：{path}"));
    Ok(Some(path))
}

/// 导入配置：路径落不到盘的项不拦截，交给前端提示用户重选。
#[tauri::command]
fn import_config(window: tauri::WebviewWindow) -> Result<Option<Imported>, String> {
    let Some(path) = nuitka::dialog::pick_file(hwnd_of(&window), "导入配置", "Nuitka++ 配置", "*.json")
    else {
        return Ok(None);
    };
    let imported = config::import_from(std::path::Path::new(&path))?;
    nuitka::log::write(&format!("已导入配置：{path}"));
    Ok(Some(imported))
}

/// 把窗口句柄交给对话框当宿主，否则对话框会被主窗口盖住。
#[cfg(windows)]
fn hwnd_of(window: &tauri::WebviewWindow) -> isize {
    window
        .hwnd()
        .map(|handle| handle.0 as isize)
        .unwrap_or_default()
}

#[cfg(not(windows))]
fn hwnd_of(_window: &tauri::WebviewWindow) -> isize {
    0
}

#[tauri::command]
async fn export_diagnostics() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(|| {
        nuitka::diagnostics::export().map(|path| path.display().to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn python_options() -> Result<PythonOptions, String> {
    tauri::async_runtime::spawn_blocking(build_python_options)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn plan_env(base: String, python: String) -> Result<EnvPlan, String> {
    tauri::async_runtime::spawn_blocking(move || {
        plan(std::path::Path::new(&base), &python)
    })
    .await
    .map_err(|error| error.to_string())
}

#[tauri::command]
async fn prepare_env(app: AppHandle, base: String, python: String) -> Result<EnvStatus, String> {
    let dir = std::path::PathBuf::from(&base);
    let handle = app.clone();
    let python_line = python.clone();

    let prepared = tauri::async_runtime::spawn_blocking(move || {
        nuitka::bootstrap::prepare(&dir, &python_line, |event| {
            let _ = handle.emit("env-progress", event);
        })
    })
    .await
    .map_err(|error| error.to_string())?;

    match prepared {
        Ok(ready) => {
            let _ = nuitka::paths::set_python(&python);
            let _ = app.emit(
                "env-progress",
                nuitka::bootstrap::PrepareEvent::stage(
                    "环境准备完成",
                    format!(
                        "Nuitka {} · 自检{}",
                        ready.nuitka,
                        if ready.verified {
                            "通过".to_string()
                        } else {
                            format!("未通过：{}", ready.detail)
                        }
                    ),
                ),
            );
            tauri::async_runtime::spawn_blocking(nuitka::status)
                .await
                .map_err(|error| error.to_string())
        }
        Err(error) => {
            nuitka::log::error(&format!("准备环境失败：{error}"));
            Err(error)
        }
    }
}

/// 装 MSVC 生成工具：无人值守安装，UAC 与许可由系统安装器自己处理。
#[tauri::command]
async fn install_msvc(app: AppHandle, python: String) -> Result<String, String> {
    let handle = app.clone();
    let version = nuitka::bootstrap::parse_python_line(&python)
        .unwrap_or(nuitka::python_discovery::PyVersion::new(3, 13, 0));
    let cache = nuitka::paths::private_root()
        .unwrap_or_else(std::env::temp_dir)
        .join("cache");
    tauri::async_runtime::spawn_blocking(move || {
        nuitka::msvc_install::install(&cache, version, |event| {
            let _ = handle.emit("env-progress", event);
        })
        .map(|outcome| {
            if outcome.need_reboot {
                "安装完成，重启程序后生效。".to_string()
            } else if outcome.toolset.is_empty() {
                "安装完成，但还没扫到工具链，重启程序后重试。".to_string()
            } else {
                format!("安装完成，工具集 {}（{}）", outcome.toolset, outcome.method)
            }
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
fn preview_command(request: build::Request) -> String {
    build::command_line(&request)
}
/// 单文件打包前先问一句：这个 Python 里有没有 zstandard。
#[tauri::command]
async fn compression_ready(python: String) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || {
        build::compression_ready(std::path::Path::new(&python))
    })
    .await
    .map_err(|error| error.to_string())
}

/// 缺件时就地补装，不用把整个「准备环境」重跑一遍。
#[tauri::command]
async fn install_compression(python: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        nuitka::bootstrap::install_zstandard(std::path::Path::new(&python), |line| {
            nuitka::log::write(&line);
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 设置页「扫描」：列出本机所有 Python，附带各自装了多少库。
/// 库数少的基本就是纯净解释器，打出来的 exe 最容易缺库，所以单独标出来。
#[derive(serde::Serialize)]
struct PythonOption {
    path: String,
    version: String,
    source: String,
    packages: usize,
    /// Nuitka++ 自己下载的那个：只该用来编译，别指望它带着业务依赖
    managed: bool,
    hint: String,
}

#[tauri::command]
async fn scan_pythons() -> Result<Vec<PythonOption>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let policy = nuitka::python_discovery::Policy::default();
        let mut list: Vec<PythonOption> = nuitka::python_discovery::scan(&policy)
            .into_iter()
            .map(|python| {
                let packages = nuitka::deps::package_count(&python.executable);
                let managed = nuitka::deps::managed_python(&python.executable);
                let hint = if managed {
                    "Nuitka++ 自行下载，仅用于编译，不推荐用来打包带依赖的项目".to_string()
                } else if packages == 0 {
                    "一个第三方库都没有，打包带依赖的项目会缺库".to_string()
                } else {
                    String::new()
                };
                PythonOption {
                    path: python.executable.display().to_string(),
                    version: python.version.to_string(),
                    source: python.source.tag().to_string(),
                    packages,
                    managed,
                    hint,
                }
            })
            .collect();
        // 库多的排前面：用户通常就是用这个开发环境跑的脚本
        list.sort_by_key(|item| std::cmp::Reverse(item.packages));
        list
    })
    .await
    .map_err(|error| error.to_string())
}

/// 启动打包。缺库这件事不再靠静态检查预判（按名字猜容易猜错，代价是让人去装不存在的包），
/// 而是由 `build::run` 在产物生成后试运行一次，真缺库了再顺着报错指回 Python 环境。
#[tauri::command]
async fn start_build(app: AppHandle, request: build::Request) -> Result<build::Outcome, String> {
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut tracker = build::Tracker::default();
        build::run(&request, |line| {
            // 界面不显示日志，但完整输出要落盘，出错时让用户去看
            nuitka::log::write(line);
            if let Some(progress) = tracker.feed(line) {
                let _ = handle.emit("build-progress", progress);
            }
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
fn open_log() -> Result<(), String> {
    let path = nuitka::log::path().ok_or_else(|| "找不到日志文件位置".to_string())?;
    if !path.is_file() {
        return Err(format!("日志文件还没有生成：{}", path.display()));
    }
    std::process::Command::new("explorer.exe")
        .arg(format!("/select,{}", path.display()))
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("无法打开日志：{error}"))
}

#[tauri::command]
fn cancel_build() -> Result<(), String> {
    build::cancel()
}

#[tauri::command]
fn open_path(path: String) -> Result<(), String> {
    let dir = std::path::Path::new(path.trim());
    if !dir.exists() {
        return Err(format!("目录不存在：{}", dir.display()));
    }
    crate::util::command("explorer.exe")
        .arg(dir)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("无法打开目录：{error}"))
}

/// 「关于」里的官网入口。这个命令本质上是替用户开浏览器，
/// 所以只认项目自己的站点：不设白名单就等于留了个任意 URL 启动器。
const SITES: &[&str] = &[
    "https://www.nulnstudio.com/",
    "https://github.com/Nuln-Studio/",
    "https://gitee.com/nuln-studio/",
];

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    let trimmed = url.trim();
    if !SITES.iter().any(|prefix| trimmed.starts_with(prefix)) {
        return Err(format!("这个链接不在白名单里：{trimmed}"));
    }
    // 交给 url.dll 而不是 `cmd /c start`：后者会把 URL 里的 & 当成命令分隔符
    crate::util::command("rundll32.exe")
        .arg("url.dll,FileProtocolHandler")
        .arg(trimmed)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("打不开链接：{error}"))
}

/// 读回用户选的 `.ico`，编码成 data URI 给界面画预览。
///
/// 意图上只服务预览：限 1 MB、必须是 `.ico`，不做通用文件读取。
#[tauri::command]
fn icon_preview(path: String) -> Result<String, String> {
    let file = std::path::Path::new(path.trim());
    if !file.is_file() {
        return Err(format!("图标文件不存在：{}", file.display()));
    }
    let is_ico = file
        .extension()
        .map(|ext| ext.to_string_lossy().eq_ignore_ascii_case("ico"))
        .unwrap_or(false);
    if !is_ico {
        return Err(format!("图标必须是 .ico 文件：{}", file.display()));
    }
    let size = std::fs::metadata(file)
        .map_err(|error| format!("读不到图标信息：{error}"))?
        .len();
    if size > 1024 * 1024 {
        return Err("图标文件超过 1 MB，不像是 .ico".to_string());
    }
    let bytes = std::fs::read(file).map_err(|error| format!("读取图标失败：{error}"))?;
    Ok(crate::util::base64(&bytes))
}

#[tauri::command]
fn enter_main(window: tauri::Window) -> Result<(), String> {
    window
        .set_resizable(true)
        .map_err(|error| error.to_string())?;
    window
        .set_size(tauri::LogicalSize::new(1080.0, 760.0))
        .map_err(|error| error.to_string())?;
    let _ = window.center();
    Ok(())
}

/// GUI 模式下没有控制台，panic 默认什么都看不到，至少留条记录到日志里。
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        nuitka::log::error(&format!("未处理的 panic：{info}"));
    }));
}

pub fn run() {
    install_panic_hook();

    if !nuitka::instance::acquire() {
        nuitka::instance::notify_already_running();
        return;
    }

    tauri::Builder::default()
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_theme(Some(tauri::Theme::Light));
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            scan,
            env_status,
            pick_dir,
            pick_script,
            pick_icon,
            icon_preview,
            pick_python,
            read_config,
            write_settings,
            write_form,
            export_config,
            import_config,
            export_diagnostics,
            preview_command,
            compression_ready,
            install_compression,
            scan_pythons,
            start_build,
            cancel_build,
            open_log,
            open_path,
            open_url,
            python_options,
            plan_env,
            prepare_env,
            install_msvc,
            enter_main
        ])
        .run(tauri::generate_context!())
        .expect("启动失败");
}
