# scangcc 组件

`src/nuitka/` 是组件集，本目录是其中一个组件：**扫描本机 gcc 并判断 Nuitka 认不认它**。核心实现只有 `mod.rs` 一个文件。

## 1. 判定依据从哪来

Nuitka 对 gcc 的态度不是「版本够新就行」，而是**只认自己下载的那一份**。实测 `python -m nuitka --version` 的输出就写着：

```
Nuitka-Scons: Non downloaded winlibs-gcc 'D:\C\...\mingw64\bin\gcc.exe' is being ignored,
              Nuitka is very dependent on the precise one.
FATAL: Only this specific gcc is supported with Nuitka.
```

所以规则很简单：

| 来源 | 判定 |
| --- | --- |
| `%LOCALAPPDATA%\Nuitka\winlibs\mingw64\bin\gcc.exe` | **符合**（这就是 Nuitka 自己下的那一份） |
| PATH 上 / 常见目录里的其它 gcc | 不符合，Nuitka 会忽略 |

这条比「gcc >= 某个版本」准确得多 —— 本机 gcc 16.1.0 比 winlibs 的 13.2.0 新，但 Nuitka 照样拒绝。

### 1.1 还有一层：PATH 上的 gcc 会**抢先**并直接打断

实测对比（同一台机器，只改 PATH）：

| PATH 里有没有 mingw | `python -m nuitka --version` 的结果 |
| --- | --- |
| 有 `D:\C\...\mingw64\bin` | `FATAL: Only this specific gcc is supported with Nuitka.`，不会回退 |
| 去掉以后 | `Nuitka will use gcc from MinGW64 of winlibs to compile on Windows.` |

也就是说：**缓存里那份 winlibs 是「能用」，但只要 PATH 上还有别的 gcc，Nuitka 会先选中那个、然后直接失败**。所以「符合」还带一个前提——PATH 上没有外来 gcc。这个前提由调用方在报告里体现（本组件只负责标注来源）。

## 2. 怎么扫

纯文件系统 + 一次 `gcc --version`：

1. 候选来源三处：Nuitka 缓存、`PATH` 的每一项、常见目录（`%SystemDrive%\mingw64`、`msys64\mingw64`、`msys64\ucrt64`、`%ProgramFiles%\mingw64`、`mingw32`）。
2. 过滤掉不存在的路径，按 `canonicalize` 去重（PATH 里常有重复项）。
3. 对每个候选执行 `gcc.exe --version`（5 秒超时，复用 `python_discovery::run_capture`）。
4. 从首行取版本：从右往左找第一个「只由数字和点组成且含点」的 token。
   - `gcc.exe (MinGW-W64 x86_64-ucrt-posix-seh, built by Brecht Sanders, r8) 13.2.0` → `13.2.0`
   - `gcc.exe (x86_64-win32-seh-rev1, Built by MinGW-Builds project) 16.1.0` → `16.1.0`
5. 排序：Nuitka 缓存优先，其次版本降序。

## 3. 数据结构

```rust
pub enum Source { NuitkaCache, Path, KnownDir }

pub struct Gcc {
    pub version: PyVersion,
    pub path: PathBuf,
    pub source: Source,
    pub shadowed: bool,   // PATH 上是否存在别的 gcc
}

impl Gcc {
    pub fn is_pinned(&self) -> bool;  // 是不是 Nuitka 自己那份
    pub fn status(&self) -> Status;
}
```

`shadowed` 由 `scan()` 统一计算：只要扫描结果里出现任何 `Source::Path` 的 gcc，所有条目都会置位。为什么把它放进结构体而不是让调用方自己判断——因为「PATH 上有没有别的 gcc」是扫描过程才知道的事实，交给调用方就等于让 `main` 又背一份判定逻辑。

### 3.1 两个面：数据面与判定面

| 面 | 入口 | 返回 | 谁用 |
| --- | --- | --- | --- |
| 数据面 | `scan()` | `Vec<Gcc>` 原样结构体 | GUI / Tauri |
| 判定面 | `status()` / `best()` | `Status` 枚举与文案 | CLI |
| 报告面 | `report(items)` | 整段排版好的文本 | CLI，`main` 一行打印 |

```
Ready      符合
Shadowed   符合，但会被 PATH 上的 gcc 抢先
Conflicts  会导致 Nuitka 直接报错
Ignored    非 Nuitka 下载，会被忽略
```

`is_usable()` 只对 `Ready` 为真 —— 被抢先和被忽略都不算可用，哪怕文件确实在那儿。

## 4. 本机实测

```
gcc 13.2.0 (Nuitka 缓存)  符合，但会被 PATH 上的 gcc 抢先
   C:\Users\Administrator\AppData\Local\Nuitka\winlibs\mingw64\bin\gcc.exe
gcc 16.1.0 (PATH)  会导致 Nuitka 直接报错
   D:\C\x86_64-16.1.0-release-win32-seh-msvcrt-rt_v14-rev1\mingw64\bin\gcc.exe
```

正好演示了「更新不等于可用」，以及「PATH 上多一个 gcc 反而会让 Nuitka 瘫痪」。

## 5. 限制

- 只扫描，不下载 winlibs。缓存不存在时只报系统 gcc（并标注不符合）。
- 不判断 `ucrt` / `msvcrt` 运行时差异 —— Nuitka 缓存那份是 ucrt，系统那份是 msvrt，但判定不依赖它。
- 只找 `gcc.exe`，不认 clang。
- 每次扫描会对每个候选起一次进程（通常 1-3 个），有 5 秒超时保护。
