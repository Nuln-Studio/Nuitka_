use std::path::{Path, PathBuf};

/// 固定私有目录：%LOCALAPPDATA%\Nuitka++
pub(crate) fn private_root() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|base| PathBuf::from(base).join("Nuitka++"))
}

pub(crate) fn config_file() -> Option<PathBuf> {
    private_root().map(|root| root.join("config.json"))
}

fn read_config() -> Option<serde_json::Value> {
    let file = config_file()?;
    let text = std::fs::read_to_string(file).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_config(value: &serde_json::Value) -> Result<(), String> {
    let file = config_file().ok_or_else(|| "找不到 LOCALAPPDATA".to_string())?;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("创建配置目录失败：{error}"))?;
    }
    let text = serde_json::to_string_pretty(value)
        .map_err(|error| format!("编码配置失败：{error}"))?;
    std::fs::write(&file, text).map_err(|error| format!("写入配置失败：{error}"))
}

/// 用户选择的安装根目录；没选过就用默认私有目录。
pub(crate) fn base() -> Option<PathBuf> {
    if let Some(value) = read_config() {
        if let Some(dir) = value.get("install_dir").and_then(|item| item.as_str()) {
            if !dir.trim().is_empty() {
                return Some(PathBuf::from(dir));
            }
        }
    }
    private_root()
}

/// 用户选过的目标 Python 小版本，例如 "3.13"。
pub(crate) fn selected_python() -> Option<String> {
    read_config()?
        .get("python")
        .and_then(|item| item.as_str())
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
}

pub(crate) fn set_python(line: &str) -> Result<(), String> {
    let mut value = read_config().unwrap_or_else(|| serde_json::json!({}));
    if !value.is_object() {
        value = serde_json::json!({});
    }
    value["python"] = serde_json::Value::String(line.to_string());
    write_config(&value)
}

pub(crate) fn python_dir() -> Option<PathBuf> {
    base().map(|root| root.join("python"))
}

pub(crate) fn set_base(dir: &Path) -> Result<(), String> {
    let mut value = read_config().unwrap_or_else(|| serde_json::json!({}));
    if !value.is_object() {
        value = serde_json::json!({});
    }
    value["install_dir"] = serde_json::Value::String(dir.to_string_lossy().to_string());
    write_config(&value)
}
