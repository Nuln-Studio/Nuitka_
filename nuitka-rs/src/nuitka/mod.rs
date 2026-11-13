mod env_check;
pub(crate) mod bootstrap;
pub(crate) mod build;
pub(crate) mod diagnostics;
pub(crate) mod dialog;
pub(crate) mod fetch;
pub(crate) mod instance;
pub(crate) mod lock;
pub(crate) mod log;
pub(crate) mod paths;
pub(crate) mod python_discovery;
pub(crate) mod scangcc;
pub(crate) mod scanmsvc;
pub(crate) mod scannuitka;
pub(crate) mod selfcheck;
pub(crate) mod toolchain;

use crate::error::PythonCandidate;
use crate::error::Result;

pub(crate) use scannuitka::{Nuitka, Requirement};

#[derive(serde::Serialize)]
pub(crate) struct Report {
    pub python: String,
    pub nuitka: String,
    pub compilers: String,
    pub recommendation: String,
    pub selfcheck: String,
}

#[derive(serde::Serialize)]
pub(crate) struct EnvStatus {
    pub ready: bool,
    pub verified: bool,
    pub level: String,
    pub python: String,
    pub python_path: String,
    pub nuitka: String,
    pub compiler: String,
    /// "msvc" 或 "gcc"，打包时要据此告诉 Nuitka 用哪个后端
    pub compiler_kind: String,
    pub hint: String,
}

fn shorten(text: &str, limit: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= limit {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(limit).collect();
    out.push('…');
    out
}

/// 启动向导用的精简状态：只给结论，不给日志。
pub(crate) fn status() -> EnvStatus {
    let python = select_python();
    let nuitkas = select_nuitka();
    let python_version = nuitkas.first().map(|item| item.python_version);

    let (python_text, python_path) = match &python {
        Ok(item) => (
            format!("Python {}", item.version),
            item.path.display().to_string(),
        ),
        Err(_) => ("未找到可用的 Python".to_string(), String::new()),
    };
    let nuitka_text = match nuitkas.first() {
        Some(item) => format!("Nuitka {}", item.version),
        None => "未找到 Nuitka".to_string(),
    };

    let msvcs = scanmsvc::scan();
    let gccs = scangcc::scan();
    let msvc = python_version.and_then(|version| scanmsvc::best(&msvcs, version));
    let gcc = scangcc::best(&gccs);
    let compiler_text = msvc
        .map(|item| format!("MSVC {}", item.toolset))
        .or_else(|| gcc.map(|item| format!("gcc {}", item.version)))
        .unwrap_or_else(|| "未找到可用的编译器".to_string());

    let has_python = python.is_ok();
    let has_nuitka = !nuitkas.is_empty();
    let has_compiler = msvc.is_some() || gcc.is_some();
    let recommended = python
        .as_ref()
        .map(|item| env_check::is_recommended(item))
        .unwrap_or(false);

    let check = paths::base().and_then(|base| selfcheck::load(&base));
    let verified = check.as_ref().map(|item| item.ok).unwrap_or(false);

    let (ready, level, hint) = if has_python && has_nuitka && has_compiler {
        if verified && recommended {
            (true, "优秀", "编译自检通过，可以直接开始打包。".to_string())
        } else if verified {
            (true, "可用", "可以打包，但建议换用 3.11 以上的 Python。".to_string())
        } else {
            let hint = match check.as_ref() {
                Some(outcome) if !outcome.detail.trim().is_empty() => {
                    format!("上次编译自检未通过：{}", shorten(&outcome.detail, 80))
                }
                _ => "还没跑过编译自检，建议点「准备环境」验证一次。".to_string(),
            };
            (true, "待验证", hint)
        }
    } else if has_python && has_nuitka {
        (
            false,
            "待完善",
            "还差一个可用的编译器，建议先准备环境。".to_string(),
        )
    } else {
        (
            false,
            "需要准备",
            "缺少 Python 或 Nuitka，建议先准备环境。".to_string(),
        )
    };

    EnvStatus {
        ready,
        verified,
        level: level.to_string(),
        python: python_text,
        python_path,
        nuitka: nuitka_text,
        compiler: compiler_text,
        compiler_kind: if let Some(item) = msvc {
            // 带上版本号：Nuitka 的 --msvc=latest 认不出新版本 VS，写死版本反而能过
            format!("msvc:{}.{}", item.toolset.major, item.toolset.minor)
        } else if gcc.is_some() {
            "gcc".to_string()
        } else {
            String::new()
        },
        hint,
    }
}

pub(crate) fn select_python() -> Result<PythonCandidate> {
    env_check::select_python()
}

pub(crate) fn select_nuitka() -> Vec<Nuitka> {
    let interpreters = python_discovery::scan(&python_discovery::Policy::default());
    scannuitka::scan(&interpreters)
}

pub(crate) fn report() -> Report {
    let python = match select_python() {
        Ok(item) => {
            let note = if env_check::is_recommended(&item) {
                ""
            } else {
                "（低于推荐版本 3.11，不推荐）"
            };
            format!(
                "版本 {}，来源 {}，路径 {}{}",
                item.version,
                item.source,
                item.path.display(),
                note
            )
        }
        Err(error) => format!("查找失败：{error}"),
    };
    let nuitkas = select_nuitka();
    let nuitka = if nuitkas.is_empty() {
        "没有找到 Nuitka".to_string()
    } else {
        scannuitka::report(&nuitkas, &Requirement::default())
    };
    let python_version = nuitkas.first().map(|item| item.python_version);
    let (compilers, recommendation) = compiler_sections(python_version);
    let selfcheck_text = match paths::base().and_then(|base| selfcheck::load(&base)) {
        Some(outcome) => format!(
            "{}，编译器 {}，详情：{}",
            if outcome.ok { "通过" } else { "未通过" },
            outcome.compiler,
            outcome.detail
        ),
        None => "还没有跑过编译自检".to_string(),
    };
    Report {
        python,
        nuitka,
        compilers,
        recommendation,
        selfcheck: selfcheck_text,
    }
}

fn compiler_sections(python: Option<python_discovery::PyVersion>) -> (String, String) {
    let msvcs = scanmsvc::scan();
    let gccs = scangcc::scan();
    if msvcs.is_empty() && gccs.is_empty() {
        return ("未发现 MSVC 或 gcc".to_string(), String::new());
    }
    let mut compilers = String::new();
    if let Some(python) = python {
        compilers.push_str(&format!(
            "判定基准：Python {}，MSVC 需要 >= {}\n",
            python,
            scanmsvc::required(python)
        ));
    }
    compilers.push_str(&scanmsvc::report(&msvcs, python));
    compilers.push_str(&scangcc::report(&gccs));
    let recommendation = match python {
        None => "未找到 Nuitka，跳过需求判定".to_string(),
        Some(python) => scanmsvc::best(&msvcs, python)
            .map(|item| format!("MSVC {}{}", item.toolset, item.status(python).hint()))
            .or_else(|| scangcc::best(&gccs).map(|item| format!("gcc {}", item.version)))
            .unwrap_or_else(|| "无可用编译器".to_string()),
    };
    (compilers, recommendation)
}

#[cfg(test)]
mod tests {
    use super::python_discovery;
    use super::scangcc;
    use super::scanmsvc;
    use super::scannuitka;

    #[test]
    fn each_component_scans_on_its_own() {
        let pythons = python_discovery::scan(&python_discovery::Policy::default());
        assert!(pythons.iter().all(|item| item.executable.is_file()));

        let nuitkas = scannuitka::scan(&pythons);
        assert!(nuitkas.iter().all(|item| item.version.major > 0));

        let msvcs = scanmsvc::scan();
        assert!(msvcs.iter().all(|item| item.path.is_file()));

        let gccs = scangcc::scan();
        assert!(gccs.iter().all(|item| item.path.is_file()));

        assert_eq!(msvcs.len(), 1);
        assert!(!gccs.is_empty());
    }
}
