/// `owner` 是主窗口句柄，传 0 表示没有宿主。
/// 有宿主的对话框才会稳定压在主窗口之上，否则会被窗口盖住。
#[cfg(windows)]
pub(crate) fn pick_folder(owner: isize, title: &str) -> Option<String> {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::Shell::{
        SHBrowseForFolderW, SHGetPathFromIDListW, BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS,
        BROWSEINFOW,
    };

    let title_wide = wide(title);
    let mut display = [0u16; 260];

    let info = BROWSEINFOW {
        hwndOwner: HWND(owner as *mut core::ffi::c_void),
        pidlRoot: std::ptr::null_mut(),
        pszDisplayName: PWSTR(display.as_mut_ptr()),
        lpszTitle: PCWSTR(title_wide.as_ptr()),
        ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
        lpfn: None,
        lParam: LPARAM(0),
        iImage: 0,
    };

    unsafe {
        let pidl = SHBrowseForFolderW(&info);
        if pidl.is_null() {
            return None;
        }
        let mut path = [0u16; 260];
        let ok = SHGetPathFromIDListW(pidl as *const _, &mut path).as_bool();
        windows::Win32::System::Com::CoTaskMemFree(Some(pidl as *const core::ffi::c_void));
        if !ok {
            return None;
        }
        Some(from_wide(&path))
    }
}

/// 打开文件选择框。`pattern` 形如 `*.py`，`label` 是筛选器里的说明文字。
#[cfg(windows)]
pub(crate) fn pick_file(owner: isize, title: &str, label: &str, pattern: &str) -> Option<String> {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST,
        OPENFILENAMEW,
    };

    let title_wide = wide(title);
    let filter_wide = wide(&format!("{label} ({pattern})\0{pattern}\0所有文件\0*.*"));

    let mut buffer = [0u16; 1024];
    let mut open = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: HWND(owner as *mut core::ffi::c_void),
        lpstrFilter: PCWSTR(filter_wide.as_ptr()),
        lpstrFile: PWSTR(buffer.as_mut_ptr()),
        nMaxFile: buffer.len() as u32,
        lpstrTitle: PCWSTR(title_wide.as_ptr()),
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_EXPLORER,
        ..Default::default()
    };

    unsafe {
        if !GetOpenFileNameW(&mut open).as_bool() {
            return None;
        }
    }
    Some(from_wide(&buffer))
}

#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn from_wide(buffer: &[u16]) -> String {
    let length = buffer.iter().position(|unit| *unit == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..length])
}

#[cfg(not(windows))]
pub(crate) fn pick_folder(_owner: isize, _title: &str) -> Option<String> {
    None
}

#[cfg(not(windows))]
pub(crate) fn pick_file(_owner: isize, _title: &str, _label: &str, _pattern: &str) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_terminates_with_nul() {
        let buffer = wide("abc");
        assert_eq!(buffer, vec![97, 98, 99, 0]);
    }

    #[test]
    fn from_wide_stops_at_first_nul() {
        let buffer = [104u16, 105, 0, 120];
        assert_eq!(from_wide(&buffer), "hi");
    }
}
