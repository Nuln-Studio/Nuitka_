/// 单实例：开两个窗口同时准备环境会互相踩目录。
#[cfg(windows)]
pub(crate) fn acquire() -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;

    let name: Vec<u16> = "Local\\Nuitka++-singleton"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        match CreateMutexW(None, false, PCWSTR(name.as_ptr())) {
            Ok(handle) => {
                // 句柄故意不关：进程活着，互斥体就存在
                let _ = handle;
                GetLastError() != ERROR_ALREADY_EXISTS
            }
            // 拿不到互斥体时宁可不拦
            Err(_) => true,
        }
    }
}

#[cfg(windows)]
pub(crate) fn notify_already_running() {
    use windows::Win32::UI::WindowsAndMessaging::{
        MB_ICONINFORMATION, MB_OK, MessageBoxW,
    };
    use windows::core::w;
    unsafe {
        MessageBoxW(
            None,
            w!("Nuitka++ 已经在运行，请切换到已打开的窗口。"),
            w!("Nuitka++"),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

#[cfg(not(windows))]
pub(crate) fn acquire() -> bool {
    true
}

#[cfg(not(windows))]
pub(crate) fn notify_already_running() {}
