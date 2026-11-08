use std::path::Path;
use std::time::Duration;

use crate::nuitka::python_discovery::{PyVersion, run_capture};

const RECORD_SEPARATOR: char = '\u{1f}';

pub const NUITKA_SOURCE: &str = concat!(
    "import sys\n",
    "try:\n",
    "    import importlib.metadata as md\n",
    "    ver = md.version('Nuitka')\n",
    "except BaseException:\n",
    "    import nuitka.Version as nv\n",
    "    ver = nv.getNuitkaVersion()\n",
    "import nuitka.PythonVersions as pv\n",
    "def q(name):\n",
    "    fn = getattr(pv, name, None)\n",
    "    return fn() if fn else None\n",
    "def j(xs):\n",
    "    return ','.join(xs) if xs else ''\n",
    "p = sys.version_info\n",
    "print(chr(31).join([ver, j(q('getSupportedPythonVersions')), ",
    "j(q('getPartiallySupportedPythonVersions')), ",
    "j(q('getNotYetSupportedPythonVersions')), ",
    "str(q('getRecommendedSupportedVersion') or ''), ",
    "'%d.%d.%d' % (p[0], p[1], p[2])]))\n"
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub version: PyVersion,
    pub supported: Vec<PyVersion>,
    pub partial: Vec<PyVersion>,
    pub unsupported: Vec<PyVersion>,
    pub recommended: Option<PyVersion>,
    pub python_version: PyVersion,
}

impl Record {
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim_end_matches(['\r', '\n', '\0', ' ']);
        let fields: Vec<&str> = raw.split(RECORD_SEPARATOR).collect();
        if fields.len() != 6 {
            return None;
        }
        Some(Self {
            version: PyVersion::parse(fields[0])?,
            supported: parse_list(fields[1]),
            partial: parse_list(fields[2]),
            unsupported: parse_list(fields[3]),
            recommended: PyVersion::parse(fields[4]),
            python_version: PyVersion::parse(fields[5])?,
        })
    }
}

fn parse_list(raw: &str) -> Vec<PyVersion> {
    raw.split(',').filter_map(PyVersion::parse).collect()
}

pub fn probe(executable: &Path, timeout: Duration) -> Option<Record> {
    if !executable.is_file() {
        return None;
    }
    let args = ["-X", "utf8", "-c", NUITKA_SOURCE];
    let (success, stdout) = run_capture(executable, &args, timeout)?;
    if !success {
        return None;
    }
    Record::parse(&String::from_utf8_lossy(&stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line() -> String {
        [
            "4.1.3",
            "2.6,2.7,3.4,3.12,3.13",
            "3.14",
            "",
            "3.13",
            "3.12.0",
        ]
        .join("\u{1f}")
    }

    #[test]
    fn parses_full_record() {
        let record = Record::parse(&(line() + "\r\n")).expect("record");
        assert_eq!(record.version, PyVersion::new(4, 1, 3));
        assert_eq!(record.python_version, PyVersion::new(3, 12, 0));
        assert_eq!(record.supported.len(), 5);
        assert_eq!(record.partial, vec![PyVersion::new(3, 14, 0)]);
        assert!(record.unsupported.is_empty());
        assert_eq!(record.recommended, Some(PyVersion::new(3, 13, 0)));
    }

    #[test]
    fn rejects_wrong_field_count() {
        assert!(Record::parse("4.1.3\u{1f}3.12").is_none());
        assert!(Record::parse("").is_none());
    }

    #[test]
    fn rejects_bad_version() {
        let raw = ["x", "", "", "", "", "3.12.0"].join("\u{1f}");
        assert!(Record::parse(&raw).is_none());
    }

    #[test]
    fn empty_lists_and_recommendation_are_tolerated() {
        let raw = ["4.1.3", "", "", "", "", "3.12.0"].join("\u{1f}");
        let record = Record::parse(&raw).expect("record");
        assert!(record.supported.is_empty());
        assert_eq!(record.recommended, None);
    }

    #[test]
    fn source_has_no_double_quotes() {
        assert!(!NUITKA_SOURCE.contains('"'));
    }
}
