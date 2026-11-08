# scannuitka 组件

`src/nuitka/` 是组件集，本目录是其中一个组件：**发现 Nuitka、判断是否达标、列出它适配的 Python 版本**。代码不写注释，本文件承担全部说明。

## 1. 定位

与 `python_discovery` 平级，依赖它的 `Python` 类型和进程捕获工具，但不重复扫盘：

```
main
  ├─ nuitka::select_python()   → python_discovery::scan      （找一个可用的 Python）
  └─ nuitka::select_nuitka()   → python_discovery::scan
                                  + scannuitka::scan(解释器列表)
```

`select_nuitka()` 内部先扫一遍解释器，再逐个问「你这里装了 Nuitka 吗」。这样每个 Nuitka 都天然绑定到它的解释器，不需要猜。

## 2. 为什么不用 `nuitka --version`

实测 `python -m nuitka --version` 会做两件致命的事：

1. 检查编译器，打印 `Nuitka-Scons: Non downloaded winlibs-gcc ... is being ignored` 并 `FATAL`；
2. 询问是否下载 gcc —— `Proceed and download? [Yes]/No :`，没有 tty 时按默认拒绝。

也就是说它会**联网、弹交互、写字节级副作用**。对一个要接 GUI 的扫描器，这不可接受。

所以探针只做纯导入：

```
<exe> -X utf8 -c "<单行脚本>"
```

脚本里只碰 `importlib.metadata` 和 `nuitka.PythonVersions`，导入耗时实测 22ms，无网络无提示。

### 为什么这里不加 `-I`

`python_discovery` 的探针用 `-I`（隔离），因为它只依赖标准库。但 Nuitka 是**第三方包**，而 `-I` 隐含 `-s`，会屏蔽用户级 site-packages —— `pip install --user nuitka` 的安装会凭空消失，产生假阴性。

所以本组件只用 `-X utf8`，保证「用户装在哪就能在哪看到」。

## 3. 探针协议

六个字段，`U+001F` 分隔：

| 位置 | 字段 | 来源 |
| --- | --- | --- |
| 0 | Nuitka 版本 | `importlib.metadata.version('Nuitka')`，失败退回 `nuitka.Version.getNuitkaVersion()` |
| 1 | 完全支持的 Python | `PythonVersions.getSupportedPythonVersions()` |
| 2 | 部分支持的 Python | `PythonVersions.getPartiallySupportedPythonVersions()` |
| 3 | 尚未支持的 Python | `PythonVersions.getNotYetSupportedPythonVersions()` |
| 4 | 推荐版本 | `PythonVersions.getRecommendedSupportedVersion()` |
| 5 | 当前解释器版本 | `sys.version_info` |

`getattr` + 存在性判断包住了四个取数函数，老版本 Nuitka 缺某个函数时字段退化为空，不会整条失败。

导入失败（没装 Nuitka、或解释器跑不起来）→ 探针返回 `None`，该解释器直接跳过。这就是「有没有装 Nuitka」的判定方式。

实测输出（本机 Nuitka 4.1.3）：

```
4.1.3 | 2.6,2.7,3.4,3.5,3.6,3.7,3.8,3.9,3.10,3.11,3.12,3.13 | 3.14 |  | 3.13 | 3.12.0
```

## 4. 数据结构

```rust
pub struct Nuitka {
    pub version: PyVersion,          // Nuitka 自身版本
    pub interpreter: PathBuf,        // 装在哪个解释器里
    pub python_version: PyVersion,   // 那个解释器的版本
    pub supported: Vec<PyVersion>,   // 完全支持
    pub partial: Vec<PyVersion>,     // 部分支持
    pub unsupported: Vec<PyVersion>, // 尚未支持
    pub recommended: Option<PyVersion>,
}
```

`PyVersion` 复用 `python_discovery` 的版本类型：它能解析 `3.12` 这种两段式（Nuitka 的支持矩阵就是 `'3.12'` 这种形式），并且能比较 `3.10 > 3.9`。

## 5. 判定逻辑

```rust
pub enum Support { Full, Partial, NotSupported }

impl Nuitka {
    pub fn meets(&self, requirement: &Requirement) -> bool;       // 版本 >= 要求
    pub fn support_for(&self, version: PyVersion) -> Support;     // 某版本属于哪一档
    pub fn interpreter_support(&self) -> Support;                 // 自己所在解释器属于哪一档
    pub fn report(&self, requirement: &Requirement) -> String;    // 多行可读报告
}
```

比较只到 **major.minor**：Nuitka 的支持矩阵给的是 `3.12`，而解释器报的是 `3.12.4`，按 `3.12` 归一后匹配。

`Requirement` 默认 `>= 2.0.0`，是一个可配置字段，由调用方决定要卡到哪一版。

`report()` 只返回字符串，不打印 —— 打印留在 `main`，GUI 直接读字段自己渲染。

## 6. 输出示例

```
Nuitka 4.1.3（要求 >= 2.0.0：满足）
  解释器: D:\Python3.12\python.exe / Python 3.12.0（完全支持）
  支持 Python: 2.6 2.7 3.4 3.5 3.6 3.7 3.8 3.9 3.10 3.11 3.12 3.13
  部分支持: 3.14
  尚未支持: 无
  推荐版本: 3.13
```

## 7. 与 tauri

- 组件无 `println!`、无全局状态，`scan()` 是纯函数式的（输入解释器列表，输出 Nuitka 列表）。
- 每个解释器一次探针，串行执行；解释器数量通常个位数。需要并行时把 `scan` 的循环换成与 `python_discovery::probe_many` 同款即可。
- 过 IPC 需要序列化，serde 仍由调用方引入：

```rust
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Nuitka { /* ... */ }
```

- 建议在 command 里 `spawn_blocking`，因为整体要做两次进程扫描（Python 一次、Nuitka 一次）。

## 8. 验证

| 场景 | 结果 |
| --- | --- |
| `cargo build` | 零警告 |
| `cargo clippy --all-targets -D warnings` | 通过 |
| `cargo test` | 39/39（本组件 9 个） |
| `cargo run` | 打印上面第 6 节那份报告 |
| 未安装 Nuitka 的解释器 | 探针失败 → 跳过该解释器 |
| 老版本 Nuitka 缺 `getNotYetSupportedPythonVersions` | 该字段为空，其余正常 |

## 9. 已知限制

- 只认 `nuitka.PythonVersions`；Nuitka 1.x 及更早没有这个模块，会被判为「没装」。
- 支持矩阵来自**已安装包自带的常量**，不联网查 PyPI，因此与官方文档可能滞后（例如新版本发布后矩阵才会更新）。
- 同一解释器存在多个 Nuitka（site-packages 与 user site 同时装）时，报的是实际被导入的那一个。
- 不做 `nuitka --version`，因此拿不到 `Flavor`、`Commercial`、编译器检测结果 —— 那些属于「能不能真编译」的范畴，不是「装了什么版本、支持哪些 Python」的范畴。
- Windows 之外的探测逻辑相同，但只在 Windows 上实测过。

## 10. 文件地图

| 文件 | 职责 |
| --- | --- |
| `mod.rs` | `Nuitka`、`Support`、`Requirement`、`scan`、`report`、排序 |
| `probe.rs` | 探针脚本、`Record` 解析、单解释器探测 |
| `probe.rs` 依赖 | `python_discovery::{PyVersion, run_capture}` |
