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
    /// 告诉 Nuitka 用哪个后端：不指定它会自己挑，Windows 上默认挑 MinGW
    pub nuitka_args: Vec<String>,
    /// 实际会被调用的编译器可执行文件
    pub binary: Option<PathBuf>,
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

    /// 日志里用：认到的是哪个版本、最后会执行哪个 exe
    pub(crate) fn describe(&self) -> String {
        match &self.binary {
            Some(binary) => format!("{}（{}）", self.label, binary.display()),
            None => self.label.clone(),
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
                let binary = find_in_path(&env, "cl.exe");
                log::info(&format!(
                    "使用 {label}，VC 环境来自 {}，cl.exe={}",
                    script.display(),
                    binary
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "未找到".to_string())
                ));
                return Some(Toolchain {
                    kind: Kind::Msvc,
                    label,
                    env,
                    path: None,
                    // 版本写死：--msvc=latest 认不出新版本 VS，写死 14.50 反而能过
                    nuitka_args: vec![format!(
                        "--msvc={}.{}",
                        msvc.toolset.major, msvc.toolset.minor
                    )],
                    binary,
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
        nuitka_args: vec!["--mingw64".to_string()],
        binary: Some(gcc.path.clone()),
    })
}

/// MSVC 失败时的退路：Nuitka 自带缓存的 winlibs。
pub(crate) fn fallback_gcc() -> Option<Toolchain> {
    resolve_gcc()
}

/// 从 vcvars 给出的 PATH 里找出实际会被调用的那个编译器。
fn find_in_path(env: &[(String, String)], exe: &str) -> Option<PathBuf> {
    let path = env
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("PATH"))
        .map(|(_, value)| value.clone())?;

    for dir in path.split(';') {
        if dir.trim().is_empty() {
            continue;
        }
        let candidate = Path::new(dir).join(exe);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// 运行 vcvars64.bat 并把结果环境抓下来。
#[cfg(windows)]
fn vcvars_environment(script: &Path) -> Result<Vec<(String, String)>, String> {
    use std::os::windows::process::CommandExt;

    // 必须用 raw_arg：cmd 不认 Rust 默认的引号转义，会把带空格的路径拆坏
    let command_line = format!("call \"{}\" >nul 2>&1 && set", script.display());
    let output = crate::util::command("cmd.exe")
        .args(["/d", "/s", "/c"])
        .raw_arg(&command_line)
        .output()
        .map_err(|error| format!("无法启动 cmd.exe：{error}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if stderr.is_empty() {
            format!("cmd 退出码 {:?}", output.status.code())
        } else {
            stderr
        };
        return Err(detail);
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

#[cfg(not(windows))]
fn vcvars_environment(_script: &Path) -> Result<Vec<(String, String)>, String> {
    Err("只有 Windows 需要 VC 环境".to_string())
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
            nuitka_args: vec!["--msvc=14.50".to_string()],
            binary: None,
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
        assert!(envs
            .iter()
            .any(|(key, value)| key == "INCLUDE" && value.as_deref() == Some("C:\\x")));
        assert!(envs
            .iter()
            .any(|(key, value)| key == "PATH" && value.as_deref() == Some("C:\\bin")));
    }

    /// 排查用：`cargo test diag_print_toolchain -- --ignored --nocapture`
    #[test]
    #[ignore = "诊断用，打印本机的编译器探测结果"]
    fn diag_print_toolchain() {
        use super::scangcc;
        use super::scanmsvc;

        println!("roots: {:?}", scanmsvc::roots());
        let toolsets: Vec<String> = scanmsvc::scan()
            .iter()
            .map(|item| format!("{} [{}] sdk={} env={}", item.toolset, item.install, item.sdk, item.env_ready))
            .collect();
        println!("msvc: {toolsets:?}");
        let gccs: Vec<String> = scangcc::scan()
            .iter()
            .map(|item| format!("{} {} -> {}", item.version, item.path.display(), item.status().label()))
            .collect();
        println!("gcc: {gccs:?}");
        let tool = resolve(PyVersion::new(3, 12, 16));
        println!("resolved: {:?}", tool.as_ref().map(|item| item.label.clone()));
        for root in scanmsvc::roots() {
            let script = root
                .join("VC")
                .join("Auxiliary")
                .join("Build")
                .join("vcvars64.bat");
            println!("script {} exists={}", script.display(), script.is_file());
            match vcvars_environment(&script) {
                Ok(env) => println!("  env ok, {} vars", env.len()),
                Err(error) => println!("  env error: {error}"),
            }
        }
        if let Some(tool) = &tool {
            println!("kind: {}", tool.kind_label());
            println!("has INCLUDE: {}", tool.env.iter().any(|(key, _)| key == "INCLUDE"));
        }
    }
}
