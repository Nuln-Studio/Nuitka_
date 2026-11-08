use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use super::Python;
use super::sources::Source;
use super::version::{PyVersion, Release};

const RECORD_SEPARATOR: char = '\u{1f}';

const PROBE_SOURCE: &str = concat!(
    "import sys,struct,platform,os;",
    "v=sys.version_info;",
    "print(chr(31).join(map(str,[",
    "v[0],v[1],v[2],v[3],v[4],",
    "platform.python_implementation(),",
    "struct.calcsize('P')*8,",
    "os.path.realpath(sys.executable),",
    "sys.prefix,sys.base_prefix])))"
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub version: PyVersion,
    pub implementation: String,
    pub bits: u8,
    pub executable: PathBuf,
    pub prefix: PathBuf,
    pub base_prefix: PathBuf,
}

impl Record {
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim_end_matches(['\r', '\n', '\0', ' ']);
        let fields: Vec<&str> = raw.split(RECORD_SEPARATOR).collect();
        if fields.len() != 10 {
            return None;
        }
        let major: u8 = fields[0].trim().parse().ok()?;
        let minor: u8 = fields[1].trim().parse().ok()?;
        let micro: u8 = fields[2].trim().parse().ok()?;
        let release = Release::parse(fields[3])?;
        let serial_text = fields[4].trim();
        let serial: u8 = if serial_text.is_empty() {
            0
        } else {
            serial_text.parse().ok()?
        };
        let implementation = fields[5].trim();
        if implementation.is_empty() {
            return None;
        }
        let bits: u8 = fields[6].trim().parse().ok()?;
        let executable = PathBuf::from(fields[7].trim());
        let prefix = PathBuf::from(fields[8].trim());
        let base_prefix = PathBuf::from(fields[9].trim());
        if executable.as_os_str().is_empty() || prefix.as_os_str().is_empty() {
            return None;
        }
        Some(Self {
            version: PyVersion {
                major,
                minor,
                micro,
                release,
                serial,
            },
            implementation: implementation.to_string(),
            bits,
            executable,
            prefix,
            base_prefix,
        })
    }

    pub fn into_python(self, source: Source) -> Python {
        Python {
            version: self.version,
            implementation: self.implementation,
            bits: self.bits,
            executable: self.executable,
            prefix: self.prefix,
            base_prefix: self.base_prefix,
            source,
        }
    }
}

pub trait Probe: Sync {
    fn probe(&self, executable: &Path) -> Option<Record>;
}

pub struct ProcessProbe {
    timeout: Duration,
}

impl ProcessProbe {
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }
}

pub fn run_capture(executable: &Path, args: &[&str], timeout: Duration) -> Option<(bool, Vec<u8>)> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    capture(command, timeout).map(|(status, stdout)| (status.success(), stdout))
}

impl Default for ProcessProbe {
    fn default() -> Self {
        Self::new(Duration::from_secs(5))
    }
}

impl Probe for ProcessProbe {
    fn probe(&self, executable: &Path) -> Option<Record> {
        if !executable.is_file() {
            return None;
        }
        let args = ["-I", "-X", "utf8", "-c", PROBE_SOURCE];
        let (success, stdout) = run_capture(executable, &args, self.timeout)?;
        if !success {
            return None;
        }
        Record::parse(&String::from_utf8_lossy(&stdout))
    }
}

fn capture(mut command: Command, timeout: Duration) -> Option<(ExitStatus, Vec<u8>)> {
    let deadline = Instant::now() + timeout;
    let mut child = command.spawn().ok()?;
    let mut stdout_pipe = child.stdout.take()?;
    let (sender, receiver) = mpsc::channel();
    let reader = thread::spawn(move || {
        let mut buffer = Vec::with_capacity(256);
        let _ = stdout_pipe.read_to_end(&mut buffer);
        let _ = sender.send(buffer);
    });
    let remaining = deadline.saturating_duration_since(Instant::now());
    let buffer = match receiver.recv_timeout(remaining) {
        Ok(buffer) => buffer,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return None;
        }
    };
    let _ = reader.join();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some((status, buffer)),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(_) => return None,
        }
    }
}

pub fn probe_many(items: &[&Path], probe: &dyn Probe) -> Vec<Option<Record>> {
    if items.is_empty() {
        return Vec::new();
    }
    let workers = thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(1)
        .clamp(1, 8)
        .min(items.len());
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<Record>>> = items.iter().map(|_| Mutex::new(None)).collect();
    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    if index >= items.len() {
                        break;
                    }
                    let record = probe.probe(items[index]);
                    if let Ok(mut slot) = slots[index].lock() {
                        *slot = record;
                    }
                }
            });
        }
    });
    slots
        .into_iter()
        .map(|slot| slot.into_inner().unwrap_or(None))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(parts: [&str; 10]) -> String {
        parts.join("\u{1f}")
    }

    #[test]
    fn parses_full_record() {
        let raw = line([
            "3",
            "12",
            "4",
            "final",
            "0",
            "CPython",
            "64",
            r"D:\Python3.12\python.exe",
            r"D:\Python3.12",
            r"D:\Python3.12",
        ]);
        let record = Record::parse(&(raw + "\r\n")).expect("record");
        assert_eq!(record.version, PyVersion::new(3, 12, 4));
        assert_eq!(record.implementation, "CPython");
        assert_eq!(record.bits, 64);
        assert_eq!(
            record.executable,
            PathBuf::from(r"D:\Python3.12\python.exe")
        );
    }

    #[test]
    fn parses_prerelease_record() {
        let raw = line([
            "3",
            "13",
            "0",
            "candidate",
            "2",
            "CPython",
            "64",
            r"D:\py\python.exe",
            r"D:\py",
            r"D:\py",
        ]);
        let record = Record::parse(&raw).expect("record");
        assert_eq!(record.version.to_string(), "3.13.0rc2");
    }

    #[test]
    fn rejects_wrong_field_count() {
        assert!(Record::parse("3\u{1f}12\u{1f}4").is_none());
        assert!(Record::parse("").is_none());
    }

    #[test]
    fn rejects_bad_numbers() {
        let raw = line([
            "x",
            "12",
            "4",
            "final",
            "0",
            "CPython",
            "64",
            r"D:\py\python.exe",
            r"D:\py",
            r"D:\py",
        ]);
        assert!(Record::parse(&raw).is_none());
    }

    #[test]
    fn probe_source_stays_single_line() {
        assert!(!PROBE_SOURCE.contains('\n'));
        assert!(!PROBE_SOURCE.contains('"'));
    }
}
