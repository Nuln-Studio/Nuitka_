use tauri::{AppHandle, Emitter};

use crate::nuitka;
use crate::nuitka::Report;

#[tauri::command]
async fn scan() -> Result<Report, String> {
    tauri::async_runtime::spawn_blocking(nuitka::report)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn prepare_env(app: AppHandle) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let ready = nuitka::bootstrap::prepare(|stage| {
            let _ = app.emit("env-progress", stage);
        })?;
        Ok::<String, String>(format!(
            "解释器：{}\n{}",
            ready.python.display(),
            ready.nuitka
        ))
    })
    .await
    .map_err(|error| error.to_string())?
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![scan, prepare_env])
        .run(tauri::generate_context!())
        .expect("启动失败");
}
