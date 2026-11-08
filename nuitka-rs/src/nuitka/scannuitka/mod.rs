mod probe;

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use super::python_discovery::{PyVersion, Python};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    Full,
    Partial,
    NotSupported,
}

impl Support {
    pub fn label(self) -> &'static str {
        match self {
            Self::Full => "完全支持",
            Self::Partial => "部分支持",
            Self::NotSupported => "不支持",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Nuitka {
    pub version: PyVersion,
    pub interpreter: PathBuf,
    pub python_version: PyVersion,
    pub supported: Vec<PyVersion>,
    pub partial: Vec<PyVersion>,
    pub unsupported: Vec<PyVersion>,
    pub recommended: Option<PyVersion>,
}

#[derive(Debug, Clone, Copy)]
pub struct Requirement {
    pub minimum: PyVersion,
}

impl Default for Requirement {
    fn default() -> Self {
        Self {
            minimum: PyVersion::new(2, 0, 0),
        }
    }
}

impl Nuitka {
    pub fn meets(&self, requirement: &Requirement) -> bool {
        self.version >= requirement.minimum
    }

    pub fn support_for(&self, version: PyVersion) -> Support {
        if in_list(&self.supported, version) {
            Support::Full
        } else if in_list(&self.partial, version) {
            Support::Partial
        } else {
            Support::NotSupported
        }
    }

    pub fn interpreter_support(&self) -> Support {
        self.support_for(self.python_version)
    }

    pub fn report(&self, requirement: &Requirement) -> String {
        let mut out = String::new();
        let verdict = if self.meets(requirement) {
            "满足"
        } else {
            "不满足"
        };
        out.push_str(&format!(
            "Nuitka {}（要求 >= {}：{}）\n",
            self.version, requirement.minimum, verdict
        ));
        out.push_str(&format!(
            "  解释器: {} / Python {}（{}）\n",
            self.interpreter.display(),
            self.python_version,
            self.interpreter_support().label()
        ));
        out.push_str(&format!("  支持 Python: {}\n", join(&self.supported)));
        out.push_str(&format!("  部分支持: {}\n", join_or_none(&self.partial)));
        out.push_str(&format!(
            "  尚未支持: {}\n",
            join_or_none(&self.unsupported)
        ));
        out.push_str(&format!(
            "  推荐版本: {}\n",
            self.recommended.map_or_else(
                || "未知".to_string(),
                |version| format!("{}.{}", version.major, version.minor)
            )
        ));
        out
    }
}

impl fmt::Display for Nuitka {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Nuitka {} [{} / Python {}]",
            self.version,
            self.interpreter.display(),
            self.python_version
        )
    }
}

fn in_list(list: &[PyVersion], version: PyVersion) -> bool {
    list.iter()
        .any(|item| item.major == version.major && item.minor == version.minor)
}

fn join(list: &[PyVersion]) -> String {
    let mut out = String::new();
    for (index, version) in list.iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        out.push_str(&format!("{}.{}", version.major, version.minor));
    }
    out
}

fn join_or_none(list: &[PyVersion]) -> String {
    if list.is_empty() {
        "无".to_string()
    } else {
        join(list)
    }
}

pub fn report(items: &[Nuitka], requirement: &Requirement) -> String {
    let mut out = String::new();
    for item in items {
        out.push_str(&item.report(requirement));
    }
    out
}

pub fn scan(interpreters: &[Python]) -> Vec<Nuitka> {
    let timeout = Duration::from_secs(5);
    let mut out = Vec::new();
    for python in interpreters {
        let Some(record) = probe::probe(&python.executable, timeout) else {
            continue;
        };
        out.push(Nuitka {
            version: record.version,
            interpreter: python.executable.clone(),
            python_version: record.python_version,
            supported: record.supported,
            partial: record.partial,
            unsupported: record.unsupported,
            recommended: record.recommended,
        });
    }
    out.sort_by(|a, b| {
        b.version
            .cmp(&a.version)
            .then_with(|| a.interpreter.cmp(&b.interpreter))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Nuitka {
        Nuitka {
            version: PyVersion::new(4, 1, 3),
            interpreter: PathBuf::from(r"D:\Python3.12\python.exe"),
            python_version: PyVersion::new(3, 12, 0),
            supported: vec![
                PyVersion::new(3, 8, 0),
                PyVersion::new(3, 12, 0),
                PyVersion::new(3, 13, 0),
            ],
            partial: vec![PyVersion::new(3, 14, 0)],
            unsupported: vec![],
            recommended: Some(PyVersion::new(3, 13, 0)),
        }
    }

    #[test]
    fn classifies_support() {
        let nuitka = sample();
        assert_eq!(nuitka.support_for(PyVersion::new(3, 12, 4)), Support::Full);
        assert_eq!(
            nuitka.support_for(PyVersion::new(3, 14, 1)),
            Support::Partial
        );
        assert_eq!(
            nuitka.support_for(PyVersion::new(3, 6, 9)),
            Support::NotSupported
        );
        assert_eq!(nuitka.interpreter_support(), Support::Full);
    }

    #[test]
    fn checks_minimum_requirement() {
        let nuitka = sample();
        assert!(nuitka.meets(&Requirement::default()));
        assert!(!nuitka.meets(&Requirement {
            minimum: PyVersion::new(5, 0, 0)
        }));
    }

    #[test]
    fn report_lists_every_group() {
        let text = sample().report(&Requirement::default());
        assert!(text.contains("Nuitka 4.1.3"));
        assert!(text.contains("满足"));
        assert!(text.contains("3.12"));
        assert!(text.contains("3.14"));
        assert!(text.contains("尚未支持: 无"));
        assert!(text.contains("推荐版本: 3.13"));
    }

    #[test]
    fn display_is_single_line() {
        let text = sample().to_string();
        assert!(!text.contains('\n'));
        assert!(text.starts_with("Nuitka 4.1.3"));
    }
}
