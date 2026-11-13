use tauri::{AppHandle, Emitter, Manager};

use crate::nuitka;
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
async fn prepare_env(app: AppHandle, base: String) -> Result<EnvStatus, String> {
    let dir = std::path::PathBuf::from(&base);
    let handle = app.clone();

    let prepared = tauri::async_runtime::spawn_blocking(move || {
        nuitka::bootstrap::prepare(&dir, |stage| {
            let _ = handle.emit("env-progress", stage);
        })
    })
    .await
    .map_err(|error| error.to_string())?;

    match prepared {
        Ok(ready) => {
            let _ = app.emit(
                "env-progress",
                format!(
                    "环境已就绪：{}（Nuitka {}，自检{}）",
                    ready.python.display(),
                    ready.nuitka,
                    if ready.verified {
                        "通过".to_string()
                    } else {
                        format!("未通过：{}", ready.detail)
                    }
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

pub fn run() {
    if !nuitka::instance::acquire() {
        nuitka::instance::notify_already_running();
        return;
    }

    tauri::Builder::default()
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_theme(Some(tauri::Theme::Dark));
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
            prepare_env,
            enter_main
        ])
        .run(tauri::generate_context!())
        .expect("启动失败");
}
