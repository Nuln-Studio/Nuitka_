use std::path::Path;
use std::process::Stdio;

/// 模块名 → pip 包名。产物试运行时从报错里认出模块名，给用户的可执行命令得是 pip 认识的名字。
pub(crate) fn pip_name(module: &str) -> String {
    let name = match module {
        "yaml" => "PyYAML",
        "PIL" => "Pillow",
        "cv2" => "opencv-python",
        "bs4" => "beautifulsoup4",
        "sklearn" => "scikit-learn",
        "Crypto" => "pycryptodome",
        "win32api" | "win32con" | "win32gui" => "pywin32",
        "dotenv" => "python-dotenv",
        "dateutil" => "python-dateutil",
        "serial" => "pyserial",
        "OpenGL" => "PyOpenGL",
        "fitz" => "PyMuPDF",
        "docx" => "python-docx",
        "pptx" => "python-pptx",
        "nacl" => "PyNaCl",
        "jwt" => "PyJWT",
        "pymysql" => "PyMySQL",
        "MySQLdb" => "mysqlclient",
        "wx" => "wxPython",
        "gtts" => "gTTS",
        _ => module,
    };
    name.to_string()
}

/// 本工具下载的 Python 都放在 `<base>/python/<x.y>/python.exe` 下面。
/// 它只装了编译要用的东西，脚本的业务依赖一个都没有，所以不能拿它当「用户的开发环境」。
pub(crate) fn managed_python(python: &Path) -> bool {
    crate::nuitka::paths::python_dir()
        .map(|root| {
            python
                .canonicalize()
                .unwrap_or_else(|_| python.to_path_buf())
                .starts_with(root)
        })
        .unwrap_or(false)
}

/// 数一个解释器里装了多少分发包。
///
/// 走 `importlib.metadata` 而不是 `pip list`：省掉 pip 自己的启动开销，
/// 设置页扫描多个解释器时才不会一卡一秒。
const COUNT_SCRIPT: &str = r#"
import sys
try:
    from importlib.metadata import distributions
    names = set()
    for dist in distributions():
        name = dist.metadata['Name'] if dist.metadata else None
        if name:
            names.add(name.lower())
    print(len(names))
except Exception:
    print(0)
"#;

pub(crate) fn package_count(python: &Path) -> usize {
    let result = crate::util::command(python)
        .args(["-c", COUNT_SCRIPT])
        .stdin(Stdio::null())
        .env("PYTHONIOENCODING", "utf-8")
        .output();
    let Ok(output) = result else { return 0 };
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}
