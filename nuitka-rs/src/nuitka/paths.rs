use std::path::PathBuf;

/// 向导自己的私有目录：%LOCALAPPDATA%\Nuitka++
pub(crate) fn private_root() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|base| PathBuf::from(base).join("Nuitka++"))
}

/// 便携 Python 的安装根：%LOCALAPPDATA%\Nuitka++\python\<版本>\
pub(crate) fn python_dir() -> Option<PathBuf> {
    private_root().map(|root| root.join("python"))
}

/// 下载缓存：%LOCALAPPDATA%\Nuitka++\downloads\
pub(crate) fn downloads_dir() -> Option<PathBuf> {
    private_root().map(|root| root.join("downloads"))
}
