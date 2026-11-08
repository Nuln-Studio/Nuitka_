use std::path::PathBuf;

use super::super::util::expand_env;
use super::{Provider, Source};

const EXE_NAME: &str = "python.exe";

pub struct RegistrySource;

impl Provider for RegistrySource {
    fn source(&self) -> Source {
        Source::Registry
    }

    fn collect(&self, out: &mut Vec<PathBuf>) {
        for view in views() {
            if let Some(python) = win::open(view.root, view.base, view.flags) {
                for vendor_name in win::subkeys(&python) {
                    let Some(vendor) = win::open_child(&python, &vendor_name) else {
                        continue;
                    };
                    collect_key(&vendor, out);
                    for tag_name in win::subkeys(&vendor) {
                        let Some(tag) = win::open_child(&vendor, &tag_name) else {
                            continue;
                        };
                        collect_key(&tag, out);
                    }
                }
            }
            for exe in ["python.exe", "python3.exe"] {
                let sub_key =
                    format!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths\\{exe}");
                if let Some(key) = win::open(view.root, &sub_key, view.flags)
                    && let Some(value) = win::string_value(&key, None)
                {
                    out.push(PathBuf::from(expand_env(&value).as_ref()));
                }
            }
        }
    }
}

fn collect_key(key: &win::Key, out: &mut Vec<PathBuf>) {
    if let Some(value) = win::string_value(key, Some("ExecutablePath")) {
        out.push(PathBuf::from(expand_env(&value).as_ref()));
    }
    if let Some(install) = win::open_child(key, "InstallPath")
        && let Some(value) = win::string_value(&install, None)
    {
        out.push(to_executable(expand_env(&value).as_ref()));
    }
}

fn to_executable(raw: &str) -> PathBuf {
    let path = PathBuf::from(raw.trim());
    if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        path
    } else {
        path.join(EXE_NAME)
    }
}

struct View {
    root: usize,
    base: &'static str,
    flags: u32,
}

fn views() -> [View; 3] {
    [
        View {
            root: win::HKEY_CURRENT_USER,
            base: "SOFTWARE\\Python",
            flags: 0,
        },
        View {
            root: win::HKEY_LOCAL_MACHINE,
            base: "SOFTWARE\\Python",
            flags: win::KEY_WOW64_64KEY,
        },
        View {
            root: win::HKEY_LOCAL_MACHINE,
            base: "SOFTWARE\\Python",
            flags: win::KEY_WOW64_32KEY,
        },
    ]
}

mod win {
    use std::ffi::c_void;
    use std::ptr;

    pub type Hkey = *mut c_void;

    pub const HKEY_CURRENT_USER: usize = 0x8000_0001;
    pub const HKEY_LOCAL_MACHINE: usize = 0x8000_0002;
    pub const KEY_READ: u32 = 0x0002_0019;
    pub const KEY_WOW64_64KEY: u32 = 0x0000_0100;
    pub const KEY_WOW64_32KEY: u32 = 0x0000_0200;
    pub const ERROR_SUCCESS: i32 = 0;
    pub const ERROR_MORE_DATA: i32 = 234;
    pub const ERROR_NO_MORE_ITEMS: i32 = 259;
    pub const REG_SZ: u32 = 1;
    pub const REG_EXPAND_SZ: u32 = 2;

    #[link(name = "advapi32")]
    #[allow(non_snake_case)]
    unsafe extern "system" {
        fn RegOpenKeyExW(
            hkey: Hkey,
            sub_key: *const u16,
            options: u32,
            desired: u32,
            result: *mut Hkey,
        ) -> i32;
        fn RegEnumKeyExW(
            hkey: Hkey,
            index: u32,
            name: *mut u16,
            name_len: *mut u32,
            reserved: *mut u32,
            class: *mut u16,
            class_len: *mut u32,
            last_write: *mut u64,
        ) -> i32;
        fn RegQueryValueExW(
            hkey: Hkey,
            name: *const u16,
            reserved: *mut u32,
            value_type: *mut u32,
            data: *mut u8,
            data_len: *mut u32,
        ) -> i32;
        fn RegCloseKey(hkey: Hkey) -> i32;
    }

    pub struct Key(Hkey);

    impl Drop for Key {
        fn drop(&mut self) {
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        let mut buffer: Vec<u16> = text.encode_utf16().collect();
        buffer.push(0);
        buffer
    }

    pub fn open(root: usize, sub_key: &str, flags: u32) -> Option<Key> {
        let name = wide(sub_key);
        let mut handle: Hkey = ptr::null_mut();
        let status = unsafe {
            RegOpenKeyExW(
                root as Hkey,
                name.as_ptr(),
                0,
                KEY_READ | flags,
                &mut handle,
            )
        };
        if status == ERROR_SUCCESS && !handle.is_null() {
            Some(Key(handle))
        } else {
            None
        }
    }

    pub fn open_child(parent: &Key, sub_key: &str) -> Option<Key> {
        let name = wide(sub_key);
        let mut handle: Hkey = ptr::null_mut();
        let status = unsafe { RegOpenKeyExW(parent.0, name.as_ptr(), 0, KEY_READ, &mut handle) };
        if status == ERROR_SUCCESS && !handle.is_null() {
            Some(Key(handle))
        } else {
            None
        }
    }

    pub fn subkeys(key: &Key) -> Vec<String> {
        let mut out = Vec::new();
        let mut index = 0u32;
        loop {
            let mut buffer = [0u16; 256];
            let mut length = buffer.len() as u32;
            let status = unsafe {
                RegEnumKeyExW(
                    key.0,
                    index,
                    buffer.as_mut_ptr(),
                    &mut length,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            };
            if status == ERROR_NO_MORE_ITEMS {
                break;
            }
            if status != ERROR_SUCCESS {
                break;
            }
            out.push(String::from_utf16_lossy(&buffer[..length as usize]));
            index += 1;
        }
        out
    }

    pub fn string_value(key: &Key, name: Option<&str>) -> Option<String> {
        let wide_name = name.map(wide);
        let name_pointer: *const u16 = match &wide_name {
            Some(value) => value.as_ptr(),
            None => ptr::null(),
        };
        let mut value_type = 0u32;
        let mut size = 0u32;
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                name_pointer,
                ptr::null_mut(),
                &mut value_type,
                ptr::null_mut(),
                &mut size,
            )
        };
        if status != ERROR_SUCCESS && status != ERROR_MORE_DATA {
            return None;
        }
        if value_type != REG_SZ && value_type != REG_EXPAND_SZ {
            return None;
        }
        if size == 0 {
            return None;
        }
        let mut buffer = vec![0u16; size as usize / 2 + 1];
        let mut written = size;
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                name_pointer,
                ptr::null_mut(),
                &mut value_type,
                buffer.as_mut_ptr() as *mut u8,
                &mut written,
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        let characters = (written as usize / 2).min(buffer.len());
        let text = String::from_utf16_lossy(&buffer[..characters]);
        let text = text.trim_end_matches('\0').trim().to_string();
        if text.is_empty() { None } else { Some(text) }
    }
}
