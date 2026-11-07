# Nuitka++ 开发规划

> 不算新项目  团队已静默开发1个月左右

## 先说几条硬的

- 目标是「开箱即用」。任何一句「请先安装 Visual Studio」出现在界面上，都算这个产品失败。
- 编译器只有两条路：MSVC，或者 Nuitka 自己下载的 winlibs。没有第三条，也别想绕。
- 构建必须离开中文路径。项目自己就住在 `D:\Hex备份\项目\` 底下，所以这条不是提醒，是必须实现的功能。
- 现在这个阶段只扫描，不下载、不解压、不注入。那些都还没写。
- 环境探测部分已经能跑了，别推倒重来。

## 这是什么

一个用 Tauri 写的 Nuitka 图形编译工具，把 Python 脚本打成 exe。

它要挡住两件让普通用户翻车的事：一是编译器环境，Nuitka 背后得有 MSVC 或 MinGW64；二是命令行参数，`--standalone --onefile --windows-disable-console --include-data-dir=...` 这么一长串，靠记不可能，靠抄容易漏。

## 技术栈

| 层 | 选型 |
| --- | --- |
| 前端 | HTML / CSS / JavaScript |
| 后端 | Rust，Tauri 2.0 |
| 引擎 | Nuitka |

## 三个模块

### 环境向导，Rust 主导

启动时探测本机有没有 Python、MSVC、GCC，以及 Nuitka 装没装、哪一版。缺什么补什么：从国内镜像拉便携版 Python 和 MinGW64，解压到 AppData 私有目录，不碰系统环境。真正编译时只在子进程里注入需要的变量，退出即还原。

边界很清楚：它只负责让编译能跑起来，不管打包参数。

现状：探测部分完成，下载、解压、注入未开始。

### 可视化编排，前端主导

左边指令库，按基础、GUI 插件、Pygame、资源、高级分类；中间画布；右边实时命令行预览。每个拖拽块由一份 config.json 定义，写清楚 ID、类型、对应的 Nuitka 参数。再配几个一键模板。冲突组合要当场标红，比如同时拖入 `--standalone` 和 `--onefile`。

现状：未开始。

### 实时构建台，前后端协作

Rust 用管道接住 Nuitka 的输出，逐行推给前端，而不是等编译完再一次性返回。用户最怕点了没反应，流式日志就是治这个的。匹配到成功关键字就弹提示，带一个「打开输出目录」。

现状：未开始。

## 已经做出来的东西

四个组件，各自独立可调用，互相之间没有依赖，共用 `python_discovery` 里的版本类型和进程捕获。

| 组件 | 负责 | 入口 | 状态 |
| --- | --- | --- | --- |
| `python_discovery` | 从注册表、PATH、常见目录找 Python，探针确认版本、实现、位数、是否 venv | `scan(&Policy)` | 完成 |
| `scannuitka` | 找 Nuitka，读出它支持哪些 Python 版本 | `scan(&[Python])` | 完成 |
| `scanmsvc` | 找 MSVC 工具集，按 Python 版本判够不够，检查 SDK 与命令行环境 | `scan()` / `status(python)` / `best()` | 完成 |
| `scangcc` | 找 gcc，区分「Nuitka 自带」和「外来」，检测 PATH 抢占 | `scan()` / `status()` / `best()` | 完成 |

每个组件对外有三个面：`scan()` 给 GUI 拿原始数据，`status()` / `best()` 给 CLI 拿结论，`report()` 直接出排版好的文本。

现在 `cargo run` 的真实输出：

```
找到Python：版本 3.12.0，来源 Registry，路径：D:\Python3.12\python.exe

Nuitka 4.1.3（要求 >= 2.0.0：满足）
  解释器: D:\Python3.12\python.exe / Python 3.12.0（完全支持）
  支持 Python: 2.6 2.7 3.4 3.5 3.6 3.7 3.8 3.9 3.10 3.11 3.12 3.13
  部分支持: 3.14
  尚未支持: 无
  推荐版本: 3.13

编译器：
  判定基准：Python 3.12.0 → MSVC 需要 >= 14.3.0
  MSVC 14.50.35717 (18 BuildTools, Hostx64/x64)  符合，但当前命令行没有 VC 环境
  gcc 13.2.0 (Nuitka 缓存)  符合，但会被 PATH 上的 gcc 抢先
  gcc 16.1.0 (PATH)  会导致 Nuitka 直接报错
  推荐：MSVC 14.50.35717（需在 VS 开发人员命令提示符中运行）
```

质量门：`cargo test` 58 个全过，`cargo clippy -D warnings` 干净

## 实测踩到的坑

这几条和「通用做法」不一样，是拿本机试出来的，写下来免得以后重踩。

MSVC 不是有 `cl.exe` 就行。把 cl 的目录塞进 PATH 没用，`INCLUDE`、`LIB`、`VCINSTALLDIR` 还是空的，编译照样失败。要么先跑 `VsDevCmd.bat -arch=x64` 拿它生成的一整套变量，要么别指望普通命令行能用。

GCC 不能随便注入。Nuitka 只认自己下载的那份 winlibs。PATH 上只要存在别的 gcc，它会先选中那个，然后直接 `FATAL: Only this specific gcc is supported`，不会回退。所以要么把 PATH 指向 Nuitka 自带的 winlibs，要么把外来 gcc 从子进程环境里摘掉。

版本号别用 semver 硬套。MSVC 工具集是 `14.50.35717`，三段都可能是大数，塞进 `u8` 会被截断。Python 版本又可能是 `3.13.0rc1` 这种。各自建类型，别图省事共用一个。

别用 `Command::new("python")`。PATH 上的 python 可能是 Microsoft Store 的存根，跑起来只会弹商店。用探测阶段解析出来的绝对路径。

中文路径。Nuitka、Python、C 编译器在中文路径下都容易出问题。构建前把工作目录切到全英文的临时目录。

杀毒软件。编译会频繁调用 gcc.exe，容易被 Defender 或 360 拦。界面上要提示用户把输出目录加白名单。

下载要能续传。环境包一百多 MB，断一次网从头再来，用户会骂人。

便携版 Python 没有 pip。解压完要自动跑一次 `get-pip.py`。

## 里程碑

| 阶段 | 内容 | 状态 |
| --- | --- | --- |
| M0 | 环境探测：Python、Nuitka、MSVC、GCC，含平台判定与版本矩阵 | 已完成 |
| M1 | Tauri 空壳，一个按钮跑通 IPC；Rust 下载、解压、注入环境，成功执行 `nuitka --version` | M1 的探测部分已完成，其余未开始 |
| M2 | 最小闭环：选 .py、勾单文件、点打包、成功弹窗。先不做拖拽 | 未开始 |
| M3 | 可视化：指令库、画布、实时命令预览，加 Pygame 和 PyQt 模板 | 未开始 |
| M4 | 打磨：实时日志、进度条、错误提示、一键打开输出目录 | 未开始 |

## 还没动的机械活

从现在的代码走到 Tauri，有三件事是确定的，跑不掉：

加 lib target。现在是 bin crate，`src-tauri` 引用不了，得有个 `lib.rs` 把组件暴露出去。

加 serde。数据结构要过 IPC，现在一个 derive 都没有。

把私有目录加进扫描范围。将来沙盒里的便携版 Python 装在 AppData，现在四个扫描器只认系统里装的那套，向导自己下载的自己扫不到。

本文档由 蓝色大肥鱼（DeepSeek） 整理生成。
