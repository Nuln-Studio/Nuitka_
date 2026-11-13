use std::path::{Path, PathBuf};
use std::process::Command;

use super::python_discovery::PyVersion;
use super::{log, scangcc, scanmsvc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Msvc,
    Gcc,
}

#[derive(Debug, Clone)]
pub(crate) struct Toolchain {
    pub kind: Kind,
    pub label: String,
    /// 直接注入子进程的环境变量（VC 的 INCLUDE/LIB/PATH 等）
    pub env: Vec<(String, String)>,
    /// 覆盖 PATH 时用的值
    pub path: Option<String>,
}

impl Toolchain {
    pub(crate) fn apply(&self, command: &mut Command) {
        for (key, value) in &self.env {
            command.env(key, value);
        }
        if let Some(path) = &self.path {
            command.env("PATH", path);
        }
    }

    pub(crate) fn kind_label(&self) -> &'static str {
        match self.kind {
            Kind::Msvc => "MSVC",
            Kind::Gcc => "winlibs gcc",
        }
    }
}

/// 找一套“真的能用”的编译器：优先 MSVC（补上 VC 环境），退化到 Nuitka 的 winlibs。
pub(crate) fn resolve(python: PyVersion) -> Option<Toolchain> {
    resolve_msvc(python).or_else(resolve_gcc)
}

fn resolve_msvc(python: PyVersion) -> Option<Toolchain> {
    let items = scanmsvc::scan();
    let msvc = scanmsvc::best(&items, python)?;
    let label = format!("MSVC {}", msvc.toolset);

    for root in scanmsvc::roots() {
        let script = root
            .join("VC")
            .join("Auxiliary")
            .join("Build")
            .join("vcvars64.bat");
        if !script.is_file() {
            continue;
        }
        match vcvars_environment(&script) {
            Ok(env) => {
                log::info(&format!("使用 {label}，VC 环境来自 {}", script.display()));
                return Some(Toolchain {
                    kind: Kind::Msvc,
                    label,
                    env,
                    path: None,
                });
            }
            Err(error) => {
                log::error(&format!("加载 VC 环境失败（{}）：{error}", script.display()));
            }
        }
    }

    None
}

fn resolve_gcc() -> Option<Toolchain> {
    let items = scangcc::scan();
    let gcc = scangcc::best(&items)?;
    let bin = gcc.path.parent()?.to_path_buf();
    let label = format!("gcc {}", gcc.version);
    log::info(&format!(
        "使用 {label}：{}（PATH 上其它 gcc 会被挡掉）",
        gcc.path.display()
    ));
    let path = clean_path(&bin, &gcc.path);
    Some(Toolchain {
        kind: Kind::Gcc,
        label,
        env: Vec::new(),
        path: Some(path),
    })
}

/// 运行 vcvars64.bat 并把结果环境抓下来。
fn vcvars_environment(script: &Path) -> Result<Vec<(String, String)>, String> {
    let command_line = format!("call \"{}\" >nul 2>&1 && set", script.display());
    let output = Command::new("cmd.exe")
        .args(["/d", "/s", "/c", &command_line])
        .output()
        .map_err(|error| format!("无法启动 cmd.exe：{error}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }

    let text = String::from_utf8_lossy(&output.stdout).to_string();
    let env = parse_environment(&text);
    let has_include = env
        .iter()
        .any(|(key, value)| key.eq_ignore_ascii_case("INCLUDE") && !value.is_empty());
    if has_include {
        Ok(env)
    } else {
        Err("没有拿到 INCLUDE".to_string())
    }
}

fn parse_environment(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        if let Some((key, value)) = line.split_once('=') {
            if key.is_empty() || key.starts_with('=') {
                continue;
            }
            out.push((key.to_string(), value.to_string()));
        }
    }
    out
}

/// 把 PATH 里其它 gcc 去掉，只留挑中的那个。
fn clean_path(bin: &Path, keep: &Path) -> String {
    let keep_id = std::fs::canonicalize(keep).unwrap_or_else(|_| keep.to_path_buf());
    let mut kept: Vec<PathBuf> = Vec::new();

    if let Some(value) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&value) {
            if dir == bin {
                continue;
            }
            let candidate = dir.join("gcc.exe");
            if candidate.is_file() {
                let identity = std::fs::canonicalize(&candidate).unwrap_or(candidate);
                if identity != keep_id {
                    continue;
                }
            }
            kept.push(dir);
        }
    }

    kept.insert(0, bin.to_path_buf());
    kept.iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(";")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
ALLUSERSPROFILE=C:\\ProgramData
INCLUDE=C:\\SDK\\Include;C:\\VC\\Include
LIB=C:\\SDK\\Lib;C:\\VC\\Lib
=::=::\\
";

    #[test]
    fn parses_cmd_set_output() {
        let env = parse_environment(SAMPLE);
        assert!(env.iter().any(|(key, _)| key == "INCLUDE"));
        assert!(env.iter().all(|(key, _)| !key.is_empty()));
        assert!(!env.iter().any(|(key, _)| key.starts_with('=')));
    }

    #[test]
    fn keeps_include_value_intact() {
        let env = parse_environment(SAMPLE);
        let include = env.iter().find(|(key, _)| key == "INCLUDE").expect("include");
        assert_eq!(include.1, "C:\\SDK\\Include;C:\\VC\\Include");
    }

    #[test]
    fn apply_sets_environment_on_command() {
        let tool = Toolchain {
            kind: Kind::Msvc,
            label: "MSVC test".to_string(),
            env: vec![("INCLUDE".to_string(), "C:\\x".to_string())],
            path: Some("C:\\bin".to_string()),
        };
        let mut command = Command::new("cmd.exe");
        tool.apply(&mut command);
        let envs: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().to_string(),
                    value.map(|item| item.to_string_lossy().to_string()),
                )
            })
            .collect();
        assert!(envs.iter().any(|(key, value)| key == "INCLUDE"
            && value.as_deref() == Some("C:\\x")));
        assert!(envs.iter().any(|(key, value)| key == "PATH"
            && value.as_deref() == Some("C:\\bin")));
    }
}
