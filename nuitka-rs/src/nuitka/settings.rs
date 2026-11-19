use std::path::{Path, PathBuf};

/// 程序同级目录：配置和日志放在一起，整个文件夹拷走就带走了整套设置。
pub(crate) fn program_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

pub(crate) fn settings_path() -> Option<PathBuf> {
    program_dir().map(|dir| dir.join("settings.json"))
}

pub(crate) fn form_path() -> Option<PathBuf> {
    program_dir().map(|dir| dir.join("last-form.json"))
}

fn yes() -> bool {
    true
}

fn onefile() -> String {
    "onefile".to_string()
}

fn force() -> String {
    "force".to_string()
}

/// 软件级设置：跟具体项目无关，换项目也沿用。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Settings {
    /// 关闭打包时自动记住上次的项目配置
    #[serde(default = "yes")]
    pub auto_save_last: bool,
    /// 指定解释器的绝对路径；空表示按规则自动挑
    #[serde(default, alias = "python_version")]
    pub python: String,
    /// 指定编译器路径：VS 安装根、winlibs 目录、cl.exe 或 gcc.exe；空表示自动挑
    #[serde(default)]
    pub compiler: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_save_last: true,
            python: String::new(),
            compiler: String::new(),
        }
    }
}

/// 设置里的路径只有真的存在才算数：别的机器导过来的旧路径不该继续生效。
pub(crate) fn python_path() -> Option<PathBuf> {
    usable_path(&read_settings().python)
}

pub(crate) fn compiler_path() -> Option<PathBuf> {
    usable_path(&read_settings().compiler)
}

fn usable_path(value: &str) -> Option<PathBuf> {
    let trimmed = value.trim().trim_matches('"');
    if trimmed.is_empty() {
        return None;
    }
    let path = PathBuf::from(trimmed);
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

/// 上次用过的项目配置，字段名就是导出文件里的键名，别再改名。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Form {
    #[serde(default)]
    pub script: String,
    #[serde(default)]
    pub out_dir: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default = "onefile")]
    pub mode: String,
    #[serde(default = "force")]
    pub console: String,
    #[serde(default = "yes")]
    pub progress: bool,
    #[serde(default = "yes")]
    pub download: bool,
    /// 打包完是否试运行一次产物；关掉它就不会有窗口闪一下
    #[serde(default = "yes")]
    pub smoke: bool,
    /// 打包前是否弹「建议换个 Python」的提醒；嫌烦的人可以关掉，不再每次打包都跳
    #[serde(default = "yes")]
    pub python_hint: bool,
    /// 内存不够时是否自动降级（省掉压缩、按可用内存压并行数）；关掉就完全按请求的参数来
    #[serde(default = "yes")]
    pub degrade: bool,
    #[serde(default)]
    pub extra: String,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            script: String::new(),
            out_dir: String::new(),
            name: String::new(),
            icon: String::new(),
            mode: onefile(),
            console: force(),
            progress: true,
            download: true,
            smoke: true,
            python_hint: true,
            degrade: true,
            extra: String::new(),
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct Loaded {
    pub settings: Settings,
    pub form: Option<Form>,
    pub settings_path: String,
    pub form_path: String,
}

/// 导出文件的载荷：带上解释器和编译器的绝对路径，换机器导入能还原环境偏好。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Bundle {
    #[serde(default)]
    pub tool: String,
    #[serde(default = "bundle_version")]
    pub version: u32,
    /// 导出时刻（Unix 秒）
    #[serde(default)]
    pub saved_unix: String,
    #[serde(default)]
    pub form: Form,
    #[serde(default)]
    pub python: String,
    #[serde(default)]
    pub compiler: String,
}

fn bundle_version() -> u32 {
    1
}

const TOOL: &str = "Nuitka++";
const VERSION: u32 = 1;

#[derive(Debug, serde::Serialize)]
pub(crate) struct Imported {
    pub form: Form,
    pub python: String,
    pub compiler: String,
    /// 落不到盘的路径：不拦截导入，只提示用户重选
    pub missing: Vec<String>,
}

pub(crate) fn load() -> Loaded {
    // 旧版存的是小版本号 / "msvc"，不是路径：回显前先清掉，否则输入框看着像已经选好了
    let mut settings = read_settings();
    if usable_path(&settings.python).is_none() {
        settings.python.clear();
    }
    if usable_path(&settings.compiler).is_none() {
        settings.compiler.clear();
    }
    Loaded {
        settings,
        form: read_form(),
        settings_path: settings_path()
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
        form_path: form_path()
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
    }
}

pub(crate) fn read_settings() -> Settings {
    settings_path()
        .and_then(|path| read_json(&path))
        .unwrap_or_default()
}

pub(crate) fn read_form() -> Option<Form> {
    form_path().and_then(|path| read_json(&path))
}

pub(crate) fn save_settings(settings: &Settings) -> Result<(), String> {
    let path = settings_path().ok_or_else(|| "定位不到程序目录".to_string())?;
    write_json(&path, settings)
}

pub(crate) fn save_form(form: &Form) -> Result<(), String> {
    let path = form_path().ok_or_else(|| "定位不到程序目录".to_string())?;
    write_json(&path, form)
}

pub(crate) fn export_to(
    path: &Path,
    form: &Form,
    python: &str,
    compiler: &str,
) -> Result<(), String> {
    let bundle = Bundle {
        tool: TOOL.to_string(),
        version: VERSION,
        saved_unix: now_text(),
        form: form.clone(),
        python: python.to_string(),
        compiler: compiler.to_string(),
    };
    write_json(path, &bundle)
}

pub(crate) fn import_from(path: &Path) -> Result<Imported, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("读不了这个文件：{error}"))?;
    let bundle: Bundle = serde_json::from_str(&text)
        .map_err(|error| format!("不是合法的配置 JSON：{error}"))?;
    if !bundle.tool.trim().is_empty() && bundle.tool.trim() != TOOL {
        return Err(format!(
            "这个文件不是 Nuitka++ 的配置（来源：{}）",
            bundle.tool
        ));
    }
    if bundle.version > VERSION {
        return Err(format!(
            "配置版本是 v{}，当前程序只认到 v{VERSION}，请升级后再导入",
            bundle.version
        ));
    }

    let mut missing = Vec::new();
    if !bundle.python.trim().is_empty() && !Path::new(&bundle.python).is_file() {
        missing.push("Python 解释器".to_string());
    }
    if !bundle.compiler.trim().is_empty() && !Path::new(&bundle.compiler).exists() {
        missing.push("C 编译器".to_string());
    }
    if !bundle.form.script.trim().is_empty() && !Path::new(&bundle.form.script).is_file() {
        missing.push("入口脚本".to_string());
    }
    if !bundle.form.out_dir.trim().is_empty() && !Path::new(&bundle.form.out_dir).is_dir() {
        missing.push("输出目录".to_string());
    }
    if !bundle.form.icon.trim().is_empty() && !Path::new(&bundle.form.icon).is_file() {
        missing.push("程序图标".to_string());
    }

    Ok(Imported {
        form: bundle.form,
        python: bundle.python,
        compiler: bundle.compiler,
        missing,
    })
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// 先写临时文件再改名：写到一半断电也不会把原配置写坏。
fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|error| format!("编码配置失败：{error}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("创建目录失败：{error}"))?;
    }
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text).map_err(|error| format!("写入配置失败：{error}"))?;
    // Windows 上 rename 不覆盖已有文件，先删旧的再换
    if path.exists() {
        std::fs::remove_file(path).map_err(|error| format!("替换旧配置失败：{error}"))?;
    }
    std::fs::rename(&temp, path).map_err(|error| format!("保存配置失败：{error}"))
}

fn now_text() -> String {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(elapsed) => elapsed.as_secs().to_string(),
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("nuitka-settings-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("建临时目录");
        root
    }

    #[test]
    fn default_settings_keep_auto_save_on() {
        assert!(Settings::default().auto_save_last);
        assert!(Settings::default().python.is_empty());
    }

    #[test]
    fn missing_paths_do_not_count_as_selection() {
        assert_eq!(usable_path(""), None);
        assert_eq!(usable_path("   "), None);
        assert_eq!(usable_path("3.12"), None);
        assert_eq!(usable_path("C:\\nowhere\\python.exe"), None);
        assert!(usable_path(&std::env::temp_dir().display().to_string()).is_some());
    }

    #[test]
    fn legacy_key_still_deserializes() {
        let settings: Settings =
            serde_json::from_str("{\"auto_save_last\":false,\"python_version\":\"3.13\"}")
                .expect("读旧配置");
        assert!(!settings.auto_save_last);
        // 旧版存的是小版本号，不是路径，所以不当作有效选择
        assert_eq!(usable_path(&settings.python), None);
    }

    #[test]
    fn round_trips_form_through_disk() {
        let dir = temp_dir("form");
        let path = dir.join("last-form.json");
        let mut form = Form::default();
        form.script = "C:\\demo\\main.py".to_string();
        form.out_dir = "C:\\demo\\out".to_string();
        form.mode = "standalone".to_string();
        write_json(&path, &form).expect("写表单");
        let read: Form = read_json(&path).expect("读表单");
        assert_eq!(read.script, form.script);
        assert_eq!(read.mode, "standalone");
        // 第二次保存要能覆盖旧文件（Windows 的 rename 不覆盖）
        form.name = "第二次".to_string();
        write_json(&path, &form).expect("覆盖写表单");
        let again: Form = read_json(&path).expect("再读表单");
        assert_eq!(again.name, "第二次");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_then_import_reports_missing_paths() {
        let dir = temp_dir("bundle");
        let path = dir.join("config.json");
        let mut form = Form::default();
        form.script = "C:\\nowhere\\missing.py".to_string();
        form.out_dir = "C:\\nowhere\\out".to_string();
        export_to(&path, &form, "C:\\py\\python.exe", "C:\\VS\\2022").expect("导出");
        let imported = import_from(&path).expect("导入");
        assert_eq!(imported.python, "C:\\py\\python.exe");
        assert_eq!(imported.compiler, "C:\\VS\\2022");
        assert_eq!(
            imported.missing,
            vec![
                "Python 解释器".to_string(),
                "C 编译器".to_string(),
                "入口脚本".to_string(),
                "输出目录".to_string()
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_foreign_config() {
        let dir = temp_dir("foreign");
        let path = dir.join("config.json");
        std::fs::write(&path, "{\"tool\":\"别的工具\",\"form\":{}}").expect("写文件");
        let error = import_from(&path).expect_err("要拒绝");
        assert!(error.contains("不是 Nuitka++"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
