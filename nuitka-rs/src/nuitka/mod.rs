mod env_check;
pub(crate) mod bootstrap;
pub(crate) mod fetch;
pub(crate) mod paths;
pub(crate) mod python_discovery;
pub(crate) mod scangcc;
pub(crate) mod scanmsvc;
pub(crate) mod scannuitka;

use crate::error::PythonCandidate;
use crate::error::Result;

pub(crate) use scannuitka::{Nuitka, Requirement};

#[derive(serde::Serialize)]
pub(crate) struct Report {
    pub python: String,
    pub nuitka: String,
    pub compilers: String,
    pub recommendation: String,
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
    Report {
        python,
        nuitka,
        compilers,
        recommendation,
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
