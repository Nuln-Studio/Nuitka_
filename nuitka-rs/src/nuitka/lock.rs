use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// 超过这个时间的锁当作上次异常退出留下的。
const STALE: Duration = Duration::from_secs(2 * 60 * 60);

pub(crate) struct Guard {
    path: PathBuf,
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// 准备环境是重动作：同一台机器上只允许一个流程在跑。
pub(crate) fn acquire(base: &Path) -> Result<Guard, String> {
    let path = base.join(".prepare.lock");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("创建目录失败：{error}"))?;
    }

    match create(&path) {
        Ok(()) => Ok(Guard { path }),
        Err(error) => {
            if !stale(&path) {
                return Err(error);
            }
            let _ = std::fs::remove_file(&path);
            create(&path).map_err(|error| format!("无法加锁：{error}"))?;
            Ok(Guard { path })
        }
    }
}

fn create(path: &Path) -> Result<(), String> {
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => {
            let _ = writeln!(file, "{}", std::process::id());
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err("另一个环境准备流程正在进行，请等它结束".to_string())
        }
        Err(error) => Err(format!("无法创建锁文件：{error}")),
    }
}

fn stale(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| SystemTime::now().duration_since(time).ok())
        .map(|age| age > STALE)
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir()
            .join("nuitka-rs-lock-test")
            .join(name);
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch");
        root
    }

    #[test]
    fn second_acquire_is_refused() {
        let dir = scratch("busy");
        let first = acquire(&dir).expect("first");
        let second = acquire(&dir);
        assert!(second.is_err());
        drop(first);
        assert!(acquire(&dir).is_ok());
    }

    #[test]
    fn drop_releases_the_lock() {
        let dir = scratch("release");
        {
            let _guard = acquire(&dir).expect("guard");
            assert!(dir.join(".prepare.lock").is_file());
        }
        assert!(!dir.join(".prepare.lock").is_file());
    }

    #[test]
    fn old_lock_is_taken_over() {
        let dir = scratch("stale");
        let path = dir.join(".prepare.lock");
        std::fs::write(&path, "0").expect("write");
        let file = std::fs::OpenOptions::new().write(true).open(&path).expect("open");
        file.set_modified(SystemTime::now() - Duration::from_secs(3 * 60 * 60))
            .expect("touch");
        assert!(acquire(&dir).is_ok());
    }
}
