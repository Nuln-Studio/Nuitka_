use crate::nuitka;

#[tauri::command]
async fn scan() -> String {
    tauri::async_runtime::spawn_blocking(|| {
        let report = nuitka::report();
        format!(
            "{{\"python\":\"{}\",\"nuitka\":\"{}\",\"compilers\":\"{}\",\"recommendation\":\"{}\"}}",
            escape(&report.python),
            escape(&report.nuitka),
            escape(&report.compilers),
            escape(&report.recommendation)
        )
    })
    .await
    .unwrap_or_else(|_| "{}".to_string())
}

fn escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for character in raw.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other if (other as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", other as u32)),
            other => out.push(other),
        }
    }
    out
}

pub(crate) fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![scan])
        .run(tauri::generate_context!())
        .expect("启动失败");
}
