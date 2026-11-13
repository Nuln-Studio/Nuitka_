use std::ffi::OsStr;
use std::process::Command;

/// 让子进程不要弹控制台窗口。
///
/// 本程序是 GUI 子系统，随便起一个 console 程序（python、curl、vswhere、cl…）
/// 都会在屏幕上闪一下黑框，这里统一用 `CREATE_NO_WINDOW` 压掉。
pub(crate) fn silent(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// 建一个默认不闪黑框的 `Command`。
pub(crate) fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    silent(&mut command);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_runs_without_a_console() {
        let mut child = command("cmd.exe")
            .args(["/c", "exit 0"])
            .spawn()
            .expect("spawn");
        let status = child.wait().expect("wait");
        assert!(status.success());
    }
}
