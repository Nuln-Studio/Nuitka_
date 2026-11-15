use std::path::Path;
use std::time::Instant;

use super::bootstrap::PrepareEvent;
use super::python_discovery::PyVersion;
use super::{fetch, log, scanmsvc};

/// 微软官方的 Build Tools 引导安装器（固定短链，只有几 MB，真正的组件由它自己下）
const BOOTSTRAPPER_URL: &str = "https://aka.ms/vs/17/release/vs_buildtools.exe";
const WORKLOAD: &str = "Microsoft.VisualStudio.Workload.VCTools";

pub(crate) struct Outcome {
    pub need_reboot: bool,
    pub method: String,
    /// 装完扫到的工具集版本，空表示没扫到
    pub toolset: String,
}

/// 安装 MSVC 生成工具。
///
/// 不是「静默安装」而是「无人值守安装」：用 ShellExecute 的 runas 提权起官方安装器，
/// UAC 必须由用户点一次，许可条款由安装器自己显示；程序负责跟进度、看退出码、装完重扫。
pub(crate) fn install<F: FnMut(PrepareEvent)>(
    cache: &Path,
    python: PyVersion,
    mut progress: F,
) -> Result<Outcome, String> {
    progress(PrepareEvent::stage(
        "准备安装 C++ 编译工具",
        "微软官方安装器 · 约 2–3 GB · 需要管理员权限",
    ));

    let exe = cache.join("vs_buildtools.exe");
    ensure_bootstrapper(&exe, &mut progress)?;

    let params = format!("--passive --wait --norestart --add {WORKLOAD} --includeRecommended");
    progress(PrepareEvent::stage(
        "正在安装 C++ 编译工具",
        "系统会弹一次权限确认，请点「是」；安装器窗口里能看到进度",
    ));
    log::info("开始安装 MSVC：官方安装器 · runas 提权");

    let code = run_elevated(&exe, &params, &mut progress)?;
    log::info(&format!("MSVC 安装器退出码：{code}"));

    let need_reboot = code == 3010;
    match code {
        0 | 3010 => {}
        1602 => return Err("安装被取消".to_string()),
        _ => return Err(format!("安装器退出码 {code}")),
    }

    // 装完不能只看退出码：真扫到能用的工具链才算成功
    progress(PrepareEvent::stage(
        "检查新装上的工具链",
        "重新扫描 VS 安装目录",
    ));
    let msvcs = scanmsvc::scan();
    let toolset = scanmsvc::best(&msvcs, python)
        .map(|item| item.toolset.to_string())
        .unwrap_or_default();
    if toolset.is_empty() && !need_reboot {
        return Err("安装已结束，但没扫到能用于该 Python 的工具链，可能需要重启".to_string());
    }

    Ok(Outcome {
        need_reboot,
        method: "官方安装器".to_string(),
        toolset,
    })
}

/// 把引导安装器下到缓存目录；已有就直接用。
fn ensure_bootstrapper<F: FnMut(PrepareEvent)>(exe: &Path, progress: &mut F) -> Result<(), String> {
    let ready = exe.metadata().map(|meta| meta.len() > 1024).unwrap_or(false);
    if ready {
        progress(PrepareEvent::log("复用已下载的安装器"));
        return Ok(());
    }
    progress(PrepareEvent::stage("下载微软安装器", BOOTSTRAPPER_URL));
    fetch::download_to(BOOTSTRAPPER_URL, exe, 0, |bytes, total, speed| {
        progress(PrepareEvent::download(
            "正在下载微软安装器",
            "vs_buildtools.exe",
            "微软官方",
            bytes,
            total,
            speed,
        ));
    })?;
    if !exe.is_file() {
        return Err("安装器下载后不存在，请检查网络".to_string());
    }
    Ok(())
}

/// 用 runas 提权启动安装器，等它退出后拿退出码。
///
/// winget 也支持装这个包，但它在非交互环境里 ShellExecute 会直接失败（0x8A150006），
/// 所以这里始终走「官方引导安装器 + 自己提权」这一条路。
fn run_elevated<F: FnMut(PrepareEvent)>(
    exe: &Path,
    params: &str,
    progress: &mut F,
) -> Result<i32, String> {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, WAIT_OBJECT_0};
        use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
        use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        use windows::core::PCWSTR;

        let file = wide(&exe.to_string_lossy());
        let verb = wide("runas");
        let params_w = wide(params);
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS,
            lpVerb: PCWSTR(verb.as_ptr()),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(params_w.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        if let Err(error) = unsafe { ShellExecuteExW(&mut info) } {
            let code = error.code().0 as u32 & 0xFFFF;
            if code == ERROR_CANCELLED.0 {
                return Err("需要管理员权限，权限确认被取消或拒绝".to_string());
            }
            return Err(format!("无法启动安装器：{error}"));
        }
        let process = info.hProcess;
        if process.is_invalid() {
            return Err("安装器没有返回进程句柄".to_string());
        }

        let started = Instant::now();
        loop {
            match unsafe { WaitForSingleObject(process, 2000) } {
                WAIT_OBJECT_0 => break,
                result if result == windows::Win32::Foundation::WAIT_FAILED => {
                    let _ = unsafe { CloseHandle(process) };
                    return Err("等待安装器失败".to_string());
                }
                _ => {
                    let secs = started.elapsed().as_secs();
                    progress(PrepareEvent::stage(
                        "正在安装 C++ 编译工具",
                        format!(
                            "已等待 {:02}:{:02}，安装器窗口里能看到进度",
                            secs / 60,
                            secs % 60
                        ),
                    ));
                }
            }
        }

        let mut code = 0u32;
        if let Err(error) = unsafe { GetExitCodeProcess(process, &mut code) } {
            let _ = unsafe { CloseHandle(process) };
            return Err(format!("读取安装器退出码失败：{error}"));
        }
        let _ = unsafe { CloseHandle(process) };
        Ok(code as i32)
    }
    #[cfg(not(windows))]
    {
        let _ = (exe, params, progress);
        Err("这个平台不需要安装 MSVC".to_string())
    }
}

#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
