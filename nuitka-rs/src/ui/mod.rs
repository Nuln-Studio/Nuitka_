use tauri::{AppHandle, Emitter, Manager};

use crate::nuitka;
use crate::nuitka::bootstrap::{EnvPlan, plan};
use crate::nuitka::build;
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
async fn plan_env(base: String) -> Result<EnvPlan, String> {
    tauri::async_runtime::spawn_blocking(move || plan(std::path::Path::new(&base)))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn prepare_env(app: AppHandle, base: String) -> Result<EnvStatus, String> {
    let dir = std::path::PathBuf::from(&base);
    let handle = app.clone();

    let prepared = tauri::async_runtime::spawn_blocking(move || {
        nuitka::bootstrap::prepare(&dir, |event| {
            let _ = handle.emit("env-progress", event);
        })
    })
    .await
    .map_err(|error| error.to_string())?;

    match prepared {
        Ok(ready) => {
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

#[tauri::command]
fn preview_command(request: build::Request) -> String {
    build::command_line(&request)
}

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
    std::process::Command::new("explorer.exe")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("无法打开目录：{error}"))
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
            export_diagnostics,
            preview_command,
            start_build,
            cancel_build,
            open_log,
            open_path,
            plan_env,
            prepare_env,
            enter_main
        ])
        .run(tauri::generate_context!())
        .expect("启动失败");
}
