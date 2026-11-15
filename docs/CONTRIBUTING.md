# 贡献指南

**Read in English**:

[English](CONTRIBUTING.Eng.md)

这个项目欢迎 PR，但它不是「提了就会合」的仓库。改动合不合，只看两件事：能不能过质量门，以及符不符合下面的约定。

## 上手



1. Fork，从 `master` 切一个分支。**不要直接往 `master` 提 PR** —— `master` 是保护分支，必须过检查才能合。

2. 按下面的质量门跑一遍。

3. 提 PR，把模板里的 Checklist 逐条打勾。

分支名按改动类型起，别用 `patch-1` 这种：



| 类型      | 分支名            |
| ------- | -------------- |
| 新功能     | `feature/xxx`  |
| 修 Bug   | `fix/xxx`      |
| 只改文档    | `docs/xxx`     |
| 重构、行为不变 | `refactor/xxx` |

## 质量门

和 README 的「构建」一节同源，提交前全部要过：



| 命令                               | 在哪跑          | 要求                                               |
| -------------------------------- | ------------ | ------------------------------------------------ |
| `cargo build`                    | `nuitka-rs/` | 无 error，不新增 warning                              |
| `cargo test --lib`               | `nuitka-rs/` | 全过（当前 115 个）                                     |
| `cargo clippy --lib`             | `nuitka-rs/` | **不新增告警**。仓库里现存 4 处旧 lint 还在清，不用你顺手全改，除非你就是来清它们的 |
| `node --check src/ui/web/app.js` | `nuitka-rs/` | 动了前端就必须跑                                         |

要真编出 exe 的测试默认跳过，本机环境齐全时再跑：



```
cargo test -- --ignored --nocapture
```

## 提交规范

Conventional Commits，`type(scope): 做了什么`。scope 用模块名，方便一眼看出改了哪块。



| type       | 用途                                    |
| ---------- | ------------------------------------- |
| `feat`     | 新功能                                   |
| `fix`      | 修 Bug                                 |
| `refactor` | 重构，行为不变                               |
| `docs`     | 文档                                    |
| `style`    | 界面 / 样式，不动逻辑(一般可以用feat替代，除非只修改了css文件) |
| `test`     | 测试                                    |
| `chore`    | 杂活：依赖、构建、配置                           |



```
fix(bootstrap): 3.13 自由线程版没有二进制 wheel，换成标准版
feat(settings): Python 与编译器改成绝对路径输入框 + 浏览按钮
```

scope 常用值：`bootstrap`、`toolchain`、`scanmsvc`、`scangcc`、`scannuitka`、`python_discovery`、`settings`、`build`、`ui`。

一条提交只做一件事。顺手把十个文件的格式全改了、又夹带一个功能，请拆开。

## 代码约定



* **注释只解释「为什么」**，不复述「做了什么」。`// 先清空列表` 这种删掉；`// 3.13 起只认 MSVC，所以这里要拦` 这种留着。

* **改动要接进真实链路**。改界面不改 `toolchain::resolve()`，等于没改 —— 真正生效的工具链来自那里，不是界面上那个字符串。

* **路径一律绝对路径**，并且走 `settings`，不要在代码里硬编码盘符或用户名。

* **界面不出现「请先安装 Visual Studio」这类话**。这是产品红线（见规划第 7 行）：缺什么就在界面里给补装入口，不要把用户推去装。

* **编译器只有 MSVC 或 Nuitka 自带的 winlibs**，不引入第三条路线（规划第 8 行）。

* **中文为主，英文同名加 `.Eng.md`**（`README.Eng.md`、`CONTRIBUTING.Eng.md`）。两个文件顶部要互相链接，改一个必须改另一个。

* 界面改动请同时改 `src/ui/web` 那一套（`index.html`、`style.css`、`app.js`），并附前后截图到 `docs/screenshots/`。

## 不要提交的文件

`target/`、`nuitka-rs.log`、`settings.json`、`last-form.json`、`.tmp-verify/`、`nuitka-crash-report.xml`、`debug.log`。这些是本机运行产物。

## 提交什么材料



| 类型     | 附带                                                     |
| ------ | ------------------------------------------------------ |
| 修 Bug  | 复现步骤 + 程序目录 `nuitka-rs.log` 里对应的那几十行                   |
| 界面改动   | 改之前 / 改之后的截图                                           |
| 环境相关问题 | Python 版本与路径、Nuitka 版本、编译器（MSVC 工具集号或 winlibs 版本）、系统版本 |

## 什么时候会被挡回来



* 只做了格式化、重命名，没有行为或可读性上的收益

* 引入新的第三方依赖却没说为什么、换不掉什么

* 一半功能、一半测试，靠「以后再补」

* 把不相关的旧 lint、旧命名顺手全改，混在一个 PR 里

* 实际上没跑过质量门（CI 会直接告诉你）

## 关于「先副本后合并」

作者本人做较大改动时，会先复制一份副本目录验证，跑通了才并回 `nuitka-rs`。你不需要照做，但 PR 里请说明你在哪验证过、验证了什么。

## 联系方式

开 Issue 就行。安全问题不要发公开 Issue，走维护者主页上的邮箱。