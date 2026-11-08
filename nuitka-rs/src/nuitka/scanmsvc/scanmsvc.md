# scanmsvc 组件

`src/nuitka/` 是组件集，本目录是其中一个组件：**扫描本机 MSVC 工具集并判断是否满足 Nuitka 要求**。核心实现只有 `mod.rs` 一个文件。

## 1. 判定依据从哪来

不是拍脑袋，是从已安装的 Nuitka 源码里读出来的（`nuitka/build/SconsCompilerSettings.py`，Nuitka 4.1.3）：

```python
_python311_min_msvc_version = (14, 3)
...
"for Python version %s MSVC %s or later is required, not %s which is too old."
```

对应规则：

| 场景 | 要求 |
| --- | --- |
| Python >= 3.11 | MSVC 工具集 >= **14.3**（VS 2022 17.0+） |
| Python < 3.11 | MSVC 工具集 >= **14.0**（VS 2015+） |

再加上一条 Nuitka 的隐含硬要求：**必须装 Windows SDK**，否则链接阶段失败。这一条无法从版本号看出来，所以单独探测 `%ProgramFiles(x86)%\Windows Kits\10\Include` 是否存在，作为 `sdk` 字段。

`supports()` 只判版本，`usable()` 额外要求 SDK，`ready()` 再额外要求「当前命令行确实处于 MSVC 环境」—— 三个方法分开，调用方可以区分「版本太低」「缺 SDK」「没开开发人员命令提示符」三种失败。

## 1.1 「已安装」不等于「当前命令行可用」

这是实测踩出来的：普通 PowerShell 里

```
VCINSTALLDIR = (空)
VSCMD_VER    = (空)
INCLUDE      = (空)
cl on PATH   = False
```

而 `VsDevCmd.bat` 是存在的。也就是说 MSVC 装得好好的，但**没有初始化环境**，`cl.exe` 既不在 PATH，也没有 `INCLUDE`/`LIB`。Nuitka 的实测行为就是：只有在 `x64 Native Tools Command Prompt for VS` 里才能正常编译。

所以 `ready()` 的判定是：`VCINSTALLDIR` / `VSCMD_VER` / `VSCMD_ARG_TGT_ARCH` 任一非空，或 PATH 上能找到 `cl.exe`。

## 2. 怎么扫

不需要编译、不需要跑 `cl.exe`，纯文件系统：

1. **vswhere**（首选）：`%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe`
   `-products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`
   拿到所有装了 C++ 工具集的安装根目录。
2. **目录兜底**：遍历 `%ProgramFiles%` 和 `%ProgramFiles(x86)%` 下的
   `Microsoft Visual Studio\<年份>\<版本>`，看有没有 `VC\Tools\MSVC`。
3. 对每个安装根，枚举 `VC\Tools\MSVC\<工具集版本>`，取目录名当版本（例如 `14.50.35717`）。
4. 确认 `bin\Hostx64\x64\cl.exe`（或 `Hostx86\x86`）存在，把这个 `cl.exe` 路径作为可执行文件。

版本号只取 `major.minor` 参与比较（`14.50.0`），完整字符串 `toolset` 保留用于显示 —— 因为 `14.50.35717` 的 build 段超出 `u8`，而比较阈值只到 `14.3`。

同一个工具集被多个「年份\版本」目录引用时按 `toolset` 去重。

## 3. 数据结构

```rust
pub struct Msvc {
    pub toolset: Toolset,     // 14.50.35717
    pub install: String,      // "18 BuildTools"
    pub host: String,         // "Hostx64/x64"
    pub path: PathBuf,        // ...\bin\Hostx64\x64\cl.exe
    pub sdk: bool,            // Windows SDK 是否就位
    pub env_ready: bool,      // 当前命令行是否处于 MSVC 环境
}
```

MSVC 工具集的版本号单独定义了 `Toolset { major: u16, minor: u16, build: u32 }`，**没有**复用 `PyVersion`。原因是 `PyVersion` 的三段都是 `u8`，装不下 `14.50.35717` 的 build 段（35717 > 255），只能截断成 `14.50.0` —— 现在阈值只到 `14.3` 还看不出问题，但一旦要比较 build 号就会静默出错。`Toolset` 保留了完整三段，`Ord` 按 major → minor → build 比较，`Display` 原样输出。

### 3.1 两个面：数据面与判定面

组件对外提供两组入口，对应两种宿主：

| 面 | 入口 | 返回 | 谁用 |
| --- | --- | --- | --- |
| 数据面 | `scan()` | `Vec<Msvc>` 原样结构体 | GUI / Tauri，自己决定怎么呈现 |
| 判定面 | `status(python)` / `best(items, python)` / `required(python)` | `Status` 枚举与文案 | CLI，直接拿结论 |
| 报告面 | `report(items, Option<PyVersion>)` | 整段排版好的文本 | CLI，`main` 一行打印 |

`Status` 有四个取值，`label()` 给出中文，`hint()` 给推荐语用的补充（只有 `NeedEnvironment` 非空）：

```
Ready            符合
NeedEnvironment  符合，但当前命令行没有 VC 环境
MissingSdk       缺少 Windows SDK
TooOld           版本过低
```

这样判定逻辑住在组件里、可被单测覆盖；`main` 只负责排版。GUI 那边不碰 `Status`，直接读结构体。

## 4. 本机实测

```
MSVC 14.50.35717 (18 BuildTools, Hostx64/x64)  符合，但当前命令行没有 VC 环境
   C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Tools\MSVC\14.50.35717\bin\Hostx64\x64\cl.exe
```

VS 18 BuildTools，Python 3.12 → 要求 14.3，实际 14.50，SDK 已装，但当前 shell 没有 VC 环境变量 —— 所以报告里会明确要求「在 VS 开发人员命令提示符中运行」。

## 5. 限制

- 只扫描，不安装、不下载、不调用编译器验证可用性。
- 不检查 ARM64（`Hostarm64`）工具集，只认 x64/x86。
- `cl.exe` 能否真正编译取决于环境变量（`INCLUDE`/`LIB`），本组件不构造编译环境。
- 非 Windows 上没有任何 MSVC，`scan()` 返回空。
