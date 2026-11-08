use std::borrow::Cow;
use std::cmp::Ordering;
use std::path::Path;

pub fn normalize(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let body = raw
        .strip_prefix(r"\\?\")
        .or_else(|| raw.strip_prefix(r"\??\"))
        .unwrap_or(&raw);
    let trimmed = body.trim_end_matches('\\');
    let body = if trimmed.len() > 3 { trimmed } else { body };
    let mut out = String::with_capacity(body.len());
    for character in body.chars() {
        out.push(match character {
            '/' => '\\',
            other => other.to_ascii_lowercase(),
        });
    }
    out
}

pub fn cmp_path(left: &Path, right: &Path) -> Ordering {
    let left = left.to_string_lossy();
    let right = right.to_string_lossy();
    let left = left.as_bytes();
    let right = right.as_bytes();
    let shared = left.len().min(right.len());
    for index in 0..shared {
        let a = fold(left[index]);
        let b = fold(right[index]);
        if a != b {
            return a.cmp(&b);
        }
    }
    left.len().cmp(&right.len())
}

fn fold(byte: u8) -> u8 {
    match byte {
        b'/' => b'\\',
        other => other.to_ascii_lowercase(),
    }
}

pub fn strip_quotes(raw: &str) -> &str {
    let text = raw.trim();
    if text.len() >= 2 && text.starts_with('"') && text.ends_with('"') {
        &text[1..text.len() - 1]
    } else {
        text
    }
}

pub fn expand_env(raw: &str) -> Cow<'_, str> {
    if !raw.contains('%') {
        return Cow::Borrowed(raw);
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 1..];
        let Some(end) = tail.find('%') else {
            out.push_str(&rest[start..]);
            return Cow::Owned(out);
        };
        let name = &tail[..end];
        let valid = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        let value = if valid {
            std::env::var(name).ok()
        } else {
            None
        };
        match value {
            Some(value) => out.push_str(&value),
            None => {
                out.push('%');
                out.push_str(name);
                out.push('%');
            }
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    Cow::Owned(out)
}

pub fn is_stub(path: &Path) -> bool {
    let text = normalize(path);
    text.contains(r"\appdata\local\microsoft\windowsapps\") || text.contains("pythonmanager_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn normalize_plain_path() {
        assert_eq!(
            normalize(Path::new(r"C:\Python312\python.exe")),
            r"c:\python312\python.exe"
        );
    }

    #[test]
    fn normalize_strips_verbatim_prefix() {
        assert_eq!(
            normalize(Path::new(r"\\?\C:\Python312\python.exe")),
            r"c:\python312\python.exe"
        );
        assert_eq!(normalize(Path::new(r"\??\C:\Python312")), r"c:\python312");
    }

    #[test]
    fn normalize_unifies_separators_and_case() {
        assert_eq!(
            normalize(Path::new(r"C:/Python312/python.exe")),
            normalize(Path::new(r"c:\python312\python.exe"))
        );
    }

    #[test]
    fn normalize_trims_trailing_separator() {
        assert_eq!(normalize(Path::new(r"C:\Python312\")), r"c:\python312");
        assert_eq!(normalize(Path::new(r"C:\")), r"c:\");
    }

    #[test]
    fn path_order_ignores_case_and_separator() {
        let left = PathBuf::from(r"C:/Python312/python.exe");
        let right = PathBuf::from(r"c:\python311\python.exe");
        assert_eq!(cmp_path(&left, &right), Ordering::Greater);
        assert_eq!(
            cmp_path(&left, &PathBuf::from(r"C:/Python312/PYTHON.EXE")),
            Ordering::Equal
        );
    }

    #[test]
    fn quotes_are_stripped() {
        assert_eq!(
            strip_quotes(r#""C:\Program Files\Python\python.exe""#),
            r"C:\Program Files\Python\python.exe"
        );
        assert_eq!(strip_quotes(r"C:\Python"), r"C:\Python");
        assert_eq!(strip_quotes(r#"""#), r#"""#);
    }

    #[test]
    fn env_vars_are_expanded() {
        unsafe { std::env::set_var("PYD_TEST_ROOT", r"C:\Root") };
        assert_eq!(expand_env(r"%PYD_TEST_ROOT%\Python"), r"C:\Root\Python");
        assert_eq!(expand_env("plain"), "plain");
        assert_eq!(expand_env(r"C:\50%"), r"C:\50%");
        assert_eq!(expand_env(r"%PYD_UNKNOWN_VAR%\x"), r"%PYD_UNKNOWN_VAR%\x");
        assert!(matches!(expand_env("plain"), Cow::Borrowed(_)));
    }

    #[test]
    fn store_alias_is_detected() {
        assert!(is_stub(Path::new(
            r"C:\Users\me\AppData\Local\Microsoft\WindowsApps\python.exe"
        )));
        assert!(!is_stub(Path::new(r"D:\Python3.12\python.exe")));
    }
}
