use std::path::PathBuf;

use super::{log, paths, scangcc, scanmsvc, selfcheck, toolchain};

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

fn section(out: &mut String, title: &str, body: &str) {
    out.push_str(&format!("[{title}]\n"));
    if body.trim().is_empty() {
        out.push_str("  （空）\n");
    } else {
        out.push_str(body);
        if !body.ends_with('\n') {
            out.push('\n');
        }
    }
    out.push('\n');
}

/// 出问题时让用户贴这一段，比问“你装了什么”快得多。
pub(crate) fn snapshot() -> String {
    let mut out = String::new();
    out.push_str("Nuitka++ 诊断快照\n");
    out.push_str(&format!("生成时间（Unix 秒）：{}\n", now_seconds()));
    out.push_str(&format!(
        "程序位置：{}\n",
        std::env::current_exe()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "未知".to_string())
    ));
    out.push_str(&format!(
        "安装目录：{}\n",
        paths::base()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "未知".to_string())
    ));
    out.push_str(&format!(
        "日志文件：{}\n\n",
        log::path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "未知".to_string())
    ));

    let python = match super::select_python() {
        Ok(item) => format!(
            "  {}（来源 {}）\n  {}\n",
            item.version,
            item.source,
            item.path.display()
        ),
        Err(error) => format!("  失败：{error}\n"),
    };
    section(&mut out, "Python", &python);

    let nuitkas = super::select_nuitka();
    let mut nuitka_text = String::new();
    for item in &nuitkas {
        nuitka_text.push_str(&format!(
            "  Nuitka {}（解释器 Python {}）\n",
            item.version, item.python_version
        ));
    }
    section(&mut out, "Nuitka", &nuitka_text);

    let python_version = nuitkas.first().map(|item| item.python_version);
    section(
        &mut out,
        "MSVC",
        &scanmsvc::report(&scanmsvc::scan(), python_version),
    );
    section(&mut out, "gcc", &scangcc::report(&scangcc::scan()));

    let tool = python_version.and_then(toolchain::resolve);
    section(
        &mut out,
        "选中的编译器",
        &match &tool {
            Some(tool) => format!("  {}（{}）\n", tool.label, tool.kind_label()),
            None => "  没有找到可用的编译器\n".to_string(),
        },
    );

    let check = selfcheck::load(&paths::base().unwrap_or_default());
    section(
        &mut out,
        "编译自检",
        &match check {
            Some(outcome) => format!(
                "  {}\n  编译器：{}\n  时间：{}\n  详情：{}\n",
                if outcome.ok { "通过" } else { "未通过" },
                outcome.compiler,
                outcome.at,
                outcome.detail
            ),
            None => "  还没有跑过自检\n".to_string(),
        },
    );

    let mut path_text = String::new();
    if let Some(value) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&value) {
            let has_cl = dir.join("cl.exe").is_file();
            let has_gcc = dir.join("gcc.exe").is_file();
            if has_cl || has_gcc {
                path_text.push_str(&format!(
                    "  {}（{}{}{}）\n",
                    dir.display(),
                    if has_cl { "cl.exe" } else { "" },
                    if has_cl && has_gcc { "、" } else { "" },
                    if has_gcc { "gcc.exe" } else { "" }
                ));
            }
        }
    }
    section(&mut out, "PATH 里的编译器", &path_text);

    let log_tail = log::path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| {
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.len().saturating_sub(30);
            lines[start..].join("\n")
        })
        .unwrap_or_default();
    section(&mut out, "日志尾部", &log_tail);

    out
}

/// 写到安装目录，返回文件路径。
pub(crate) fn export() -> Result<PathBuf, String> {
    let base = paths::base().ok_or_else(|| "找不到安装目录".to_string())?;
    std::fs::create_dir_all(&base).map_err(|error| format!("创建目录失败：{error}"))?;
    let file = base.join(format!("diagnostics-{}.txt", now_seconds()));
    std::fs::write(&file, snapshot()).map_err(|error| format!("写入诊断文件失败：{error}"))?;
    log::info(&format!("诊断快照已导出：{}", file.display()));
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_has_all_sections() {
        let text = snapshot();
        for title in [
            "[Python]",
            "[Nuitka]",
            "[MSVC]",
            "[gcc]",
            "[选中的编译器]",
            "[编译自检]",
            "[PATH 里的编译器]",
        ] {
            assert!(text.contains(title), "缺少 {title}");
        }
    }

    #[test]
    fn section_marks_empty_body() {
        let mut out = String::new();
        section(&mut out, "空", "   ");
        assert!(out.contains("（空）"));
    }
}
