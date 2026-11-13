use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// 日志写到程序同目录的 nuitka-rs.log，界面上不展示。
pub(crate) fn path() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    Some(executable.parent()?.join("nuitka-rs.log"))
}

pub(crate) fn write(line: &str) {
    let Some(file_path) = path() else {
        return;
    };
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0);
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file_path)
    {
        let _ = writeln!(file, "[{seconds}] {line}");
    }
}

pub(crate) fn info(line: &str) {
    write(&format!("INFO  {line}"));
}

pub(crate) fn error(line: &str) {
    write(&format!("ERROR {line}"));
}
