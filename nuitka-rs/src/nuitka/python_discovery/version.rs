use std::cmp::Ordering;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Release {
    Alpha,
    Beta,
    Candidate,
    Final,
}

impl Release {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "final" => Some(Self::Final),
            "a" | "alpha" => Some(Self::Alpha),
            "b" | "beta" => Some(Self::Beta),
            "c" | "rc" | "candidate" => Some(Self::Candidate),
            _ => None,
        }
    }

    pub fn is_prerelease(self) -> bool {
        self != Self::Final
    }

    pub fn suffix(self) -> &'static str {
        match self {
            Self::Alpha => "a",
            Self::Beta => "b",
            Self::Candidate => "rc",
            Self::Final => "",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PyVersion {
    pub major: u8,
    pub minor: u8,
    pub micro: u8,
    pub release: Release,
    pub serial: u8,
}

impl PyVersion {
    pub const fn new(major: u8, minor: u8, micro: u8) -> Self {
        Self {
            major,
            minor,
            micro,
            release: Release::Final,
            serial: 0,
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        let cut = raw
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(raw.len());
        let (numbers, tail) = raw.split_at(cut);
        let mut parts = numbers.split('.').filter(|p| !p.is_empty());
        let major: u8 = parts.next()?.parse().ok()?;
        let minor: u8 = parts.next().unwrap_or("0").parse().ok()?;
        let micro: u8 = parts.next().unwrap_or("0").parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        let tail = tail.trim();
        if tail.is_empty() {
            return Some(Self::new(major, minor, micro));
        }
        let digit_cut = tail
            .find(|c: char| c.is_ascii_digit())
            .unwrap_or(tail.len());
        let (level, serial) = tail.split_at(digit_cut);
        let release = Release::parse(level)?;
        let serial = if serial.is_empty() {
            0
        } else {
            serial.parse().ok()?
        };
        Some(Self {
            major,
            minor,
            micro,
            release,
            serial,
        })
    }

    pub fn is_prerelease(&self) -> bool {
        self.release.is_prerelease()
    }
}

impl Ord for PyVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        self.major
            .cmp(&other.major)
            .then_with(|| self.minor.cmp(&other.minor))
            .then_with(|| self.micro.cmp(&other.micro))
            .then_with(|| self.release.cmp(&other.release))
            .then_with(|| self.serial.cmp(&other.serial))
    }
}

impl PartialOrd for PyVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for PyVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.micro)?;
        if self.release.is_prerelease() {
            write!(f, "{}{}", self.release.suffix(), self.serial)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(raw: &str) -> PyVersion {
        PyVersion::parse(raw).expect(raw)
    }

    #[test]
    fn parses_short_forms() {
        assert_eq!(v("3"), PyVersion::new(3, 0, 0));
        assert_eq!(v("3.12"), PyVersion::new(3, 12, 0));
        assert_eq!(v("3.12.4"), PyVersion::new(3, 12, 4));
        assert_eq!(v(" 3.12.4 "), PyVersion::new(3, 12, 4));
    }

    #[test]
    fn parses_prereleases() {
        assert_eq!(v("3.13.0rc1").to_string(), "3.13.0rc1");
        assert_eq!(v("3.13.0a2").to_string(), "3.13.0a2");
        assert_eq!(v("3.8.0b4").to_string(), "3.8.0b4");
        assert!(v("3.13.0rc1").is_prerelease());
        assert!(!v("3.13.0").is_prerelease());
    }

    #[test]
    fn rejects_garbage() {
        assert!(PyVersion::parse("").is_none());
        assert!(PyVersion::parse("python").is_none());
        assert!(PyVersion::parse("3.12.4.1").is_none());
        assert!(PyVersion::parse("3.x").is_none());
    }

    #[test]
    fn orders_numerically_not_lexically() {
        assert!(v("3.10.0") > v("3.9.9"));
        assert!(v("3.12.0") > v("3.11.99"));
        assert!(v("3.13.0") > v("3.13.0rc2"));
        assert!(v("3.13.0rc2") > v("3.13.0rc1"));
        assert!(v("3.13.0rc1") > v("3.13.0b9"));
    }
}
