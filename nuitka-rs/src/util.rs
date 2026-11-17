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

/// 标准 base64 编码（RFC 4648，带 `=` 填充）。
///
/// 只用来把小文件（图标，几 KB～几十 KB）编码成 data URI 给前端画预览，
/// 够用就行，不引第三方库。
pub(crate) fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(triple >> 18) as usize & 0x3f] as char);
        out.push(TABLE[(triple >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(triple >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[triple as usize & 0x3f] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_known_examples() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xff, 0xfe, 0xfd]), "//79");
    }

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
