# python_discovery 组件

`src/nuitka/` 是组件集，本目录是其中一个组件：**发现并验证本机可用的 Python 解释器**。代码不写注释，本文件承担全部说明。

## 1. 在项目里的位置

```
src/
  main.rs                       入口（未改，只多打印了来源）
  error.rs                      共享错误与 PythonCandidate / SourceKind
  ui/                           UI 层（只放界面相关代码）
  nuitka/
    mod.rs                      组件集入口，声明 pub(crate) mod python_discovery
    nuitka_pull.rs              占位（原样保留）
    env_check.rs                适配层：把组件结果转成 PythonCandidate
    python_discovery/           ← 本组件
      mod.rs  version.rs  probe.rs  util.rs
      sources/mod.rs  sources/registry.rs
```

调用链：`main` → `nuitka::select_python()` → `env_check::select_python()` → `python_discovery::scan()`。

`env_check.rs` 只剩下适配职责：选策略、挑一个、转成项目既有的 `PythonCandidate`。扫描逻辑全部在组件里。

## 2. 对外 API

| 项目 | 签名 | 用途 |
| --- | --- | --- |
| `scan` | `fn scan(policy: &Policy) -> Vec<Python>` | 扫描并按优先级排序 |
| `probe` | `fn probe(path: &Path) -> Option<Python>` | 只验证一个指定路径（GUI 手选/拖入） |
| `discover` | `fn discover(providers: &[&dyn Provider], prober: &dyn Probe, policy: &Policy) -> Vec<Python>` | 换来源或换探针（测试/特殊部署） |
| `default_providers` | `fn default_providers() -> &'static [&'static dyn Provider]` | 默认来源表，零分配 |

```rust
pub struct Python {
    pub version: PyVersion,
    pub implementation: String,
    pub bits: u8,
    pub executable: PathBuf,
    pub prefix: PathBuf,
    pub base_prefix: PathBuf,
    pub source: Source,
}
```

## 3. 探针协议

每个候选执行一次：

```
<exe> -I -X utf8 -c "<单行脚本>"
```

十个字段用 `U+001F` 连接后打印：

| 位置 | 字段 | 来源 |
| --- | --- | --- |
| 0-2 | major / minor / micro | `sys.version_info[0..3]` |
| 3 | `final` / `candidate` / `beta` / `alpha` | `sys.version_info[3]` |
| 4 | 预发布序号 | `sys.version_info[4]` |
| 5 | 实现 | `platform.python_implementation()` |
| 6 | 位数 | `struct.calcsize('P') * 8` |
| 7 | 真实可执行文件 | `os.path.realpath(sys.executable)` |
| 8-9 | prefix / base_prefix | `sys.prefix` / `sys.base_prefix` |

- `-I` 隔离模式：忽略 `PYTHONPATH` 与用户 site，不执行 `sitecustomize`，结果可复现。
- `-X utf8`：Windows 管道默认走本地代码页，中文路径会乱码。
- 分隔符而非 JSON：零依赖、零转义；`U+001F` 不会出现在路径里。
- 不用 `--version`：只给版本，拿不到实现、位数、prefix，也区分不了真解释器和同名程序。
- 判定：正常退出 + 正好 10 个字段。Store 存根、`cmd.exe`、损坏安装都在这里被拒。
- stderr 用 `Stdio::null()`：不解析也不显示，省一个管道和一个读线程。
- 超时是绝对截止时间，覆盖「等 stdout EOF」与「等进程退出」两段，超时即 kill + wait。
- 子进程带 `CREATE_NO_WINDOW`，GUI 场景不闪黑框。
- 并发度 `min(可用核数, 8, 候选数)`，本机全量约 58ms。

## 4. 来源矩阵

| 来源 | 覆盖 | 说明 |
| --- | --- | --- |
| `Registry` | python.org 安装包、Store 版、Anaconda/Miniconda、PyPy | `HKCU` + `HKLM`（64/32 视图）下 `SOFTWARE\Python` 按 vendor 泛化遍历，再读 `App Paths\python.exe` |
| `Path` | 绿色版、手动解压、用户自建 | `python.exe` / `python3.exe`，容忍引号与空段 |
| `KnownDirectory` | 新版 python.org 安装管理器、conda 全家、uv、pyenv-win、scoop、激活中的 venv | 组合 `%LOCALAPPDATA%`、`%ProgramFiles%`、`%ProgramFiles(x86)%`、`%SystemDrive%`、`%USERPROFILE%`、`%ProgramData%`、`%APPDATA%` |
| `Explicit` | `probe(path)` | 只验证，不扫描 |

关键目录：

```
%LOCALAPPDATA%\Programs\Python\Python3*\python.exe
%LOCALAPPDATA%\Python\pythoncore-*\python.exe      ← 新版安装管理器
%ProgramFiles%\Python3*\python.exe
%APPDATA%\uv\python\*\python.exe
%USERPROFILE%\.pyenv\pyenv-win\versions\*\python.exe
%USERPROFILE%\{miniconda3,anaconda3,miniforge3,mambaforge}[\envs\*]\python.exe
%VIRTUAL_ENV% / %CONDA_PREFIX%
```

**不用 `py.exe`**：它自身依赖注册表，输出是人读格式且版本没有 patch 段。绕开之后信息反而更全（`InstallPath` + `ExecutablePath`）。

**过滤存根**：`\AppData\Local\Microsoft\WindowsApps\` 下的别名、包名带 `pythonmanager_` 的路径一律跳过，避免拉起 Microsoft Store。

## 5. 去重与排序

去重两层：采集后按规范化路径去重（小写、统一分隔符、去 `\\?\`、去尾斜杠）；探针后按解释器**自报的 realpath** 去重。第二层能收敛别名、junction、8.3 短名，同时保留两个不同的 venv。

排序键：版本降序 → CPython 优先 → venv 优先 → 64 位优先 → 路径字典序。`3.10 > 3.9` 为数值比较，`3.13.0rc2 > 3.13.0rc1 > 3.13.0b9` 由 `Release` 枚举序保证。

## 6. 分配与 clone 策略

原则：能用引用就不复制，能用一次分配就不做两次。生产代码 `.clone()` 为 0，剩余全在 `#[cfg(test)]`。

| 原写法 | 现写法 | 省掉 |
| --- | --- | --- |
| 比较器里 `a.identity()` | `util::cmp_path` 逐字节折叠 | 每次比较一个 String（O(n log n) 次） |
| `executable.clone()` 收集路径 | `Vec<&Path>` 借用 | 每候选一次 PathBuf 复制 |
| `Vec<Box<dyn Provider>>` | `&'static [&'static dyn Provider]` | 每次扫描的装箱与堆分配 |
| `candidates() -> Vec` | `collect(&mut Vec)` 共享缓冲 | 每来源一个中间 Vec |
| `Sender::clone()` + stderr 线程 | `Stdio::null()` | 一个线程 + 一次 clone |
| `replace` 后再 `strip_prefix` | 先算前缀偏移，一次构造 | 一次中间 String |
| `entry.path().join(x)` | `path.push(x)` | 每条目一次 PathBuf |
| `to_string_lossy().starts_with` | `as_encoded_bytes().starts_with` | 每条目一次 Cow |
| `expand_env` 总构造 String | 无 `%` 返回 `Cow::Borrowed` | PATH 每条目一次分配 |

## 7. 与项目既有代码的衔接

`env_check.rs` 现在只做四件事：

1. 用**宽松策略**扫描一次（`minimum = 0.0.0`、允许预发布），拿到全集；
2. 全集为空 → `NuitkaRsError::PythonNotFound`；
3. 用项目策略挑一个：支持下限 `>= 3.8.0`，优先推荐 `>= 3.11.0`，拒绝预发布 → `PythonCandidate`；
4. 一个都不满足 → `NoSuitablePython { required, available }`，`available` 是拼好的可读字符串。

`Python` → `PythonCandidate` 的映射不复制路径：直接移动 `executable` 字段。版本从组件的 `PyVersion` 转成项目既有 `semver::Version`，预发布转成 `rc1` / `b1` / `a2` 形式。

原先 `env_check.rs` 里的 stdin 交互选择没有搬过来——按「GUI 与 CI 分模式」的方向，选择权应当交给调用方（GUI 弹窗、CI 取 `vec[0]`）。需要 CLI 交互时在 `main` 里加，不要塞回组件。

同时删掉了 `error.rs` 里随着旧实现一起失效的变体：`PyLauncherNotFound`、`PyLauncherFailed`、`RegexCompile`、`SelectIndexOutOfRange`、`Io`、`SemVer`、`Other`。`NoSuitablePython` 的 `available` 由 `Vec<Version>` 改为已格式化的 `String`，避免把 `semver::Version` 的 Debug 结构体打给用户。

## 8. Tauri

- 组件无 `println!`、无全局状态、无 `static mut`，并发调用安全。
- `scan()` 是同步阻塞的（几十毫秒到数秒），在 command 里丢进 `spawn_blocking`：

```rust
#[tauri::command]
async fn list_python() -> Vec<Python> {
    tauri::async_runtime::spawn_blocking(|| python_discovery::scan(&Default::default()))
        .await
        .unwrap_or_default()
}
```

- 过 IPC 需要序列化，serde 由调用方引入（组件保持零依赖）：

```rust
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Python { /* ... */ }
```

## 9. 验证

在本项目内实测：

| 场景 | 结果 |
| --- | --- |
| `cargo build` | 零警告 |
| `cargo clippy --all-targets -- -D warnings` | 通过 |
| `cargo test` | 30/30 |
| `cargo run` | `找到Python：版本 3.12.0，来源 Registry，路径：D:\Python3.12\python.exe` |
| venv 在 PATH 上 | 两条结果，venv 那条标 `venv` 且排前 |
| WindowsApps 存根 | 采集阶段被过滤 |
| 低版本（`--min` 语义） | `NoSuitablePython`，`available` 为可读串 |

## 10. 已知限制

- 注册表部分仅 Windows；其它平台只走 PATH + 已知目录。
- 低于 Python 3.7 的解释器会被判为不可用（`-I -X utf8` 要求），不会提示「找到但太旧」。
- 不递归扫描 venv 目录树，只认 `VIRTUAL_ENV` / `CONDA_PREFIX` 和 PATH 上的 venv。
- 不探测 WSL、远程、容器内的解释器。
- 无缓存，每次全量扫描；GUI 频繁调用时可在调用侧加短 TTL 缓存。

## 11. 文件地图

| 文件 | 职责 |
| --- | --- |
| `mod.rs` | 组件入口：`Python`、`Policy`、`scan` / `probe` / `discover`、排序 |
| `version.rs` | `PyVersion` / `Release`：解析、完整序、显示 |
| `probe.rs` | 探针脚本、`Record` 解析、超时捕获、并发 |
| `util.rs` | 路径规范化、零分配比较、去引号、`%VAR%` 展开、存根识别 |
| `sources/mod.rs` | `Provider` 抽象、`harvest`、PATH 源、已知目录源 |
| `sources/registry.rs` | 注册表源（`advapi32` FFI，唯一的 unsafe 岛） |
