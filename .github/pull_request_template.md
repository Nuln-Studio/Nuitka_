## 这个 PR 做了什么

<!-- 一两句说清。改的是哪块：bootstrap / toolchain / scanmsvc / scangcc / settings / build / ui -->

## 相关 Issue

<!-- Closes #123。没有就写「无」 -->

## 验证方式

| 命令 | 跑了没 | 结果 |
| --- | --- | --- |
| `cargo build` | | |
| `cargo test --lib` | | |
| `cargo clippy --lib` | | |
| `node --check src/ui/web/app.js` | | |

> 动了前端才需要最后一行。clippy 的要求是**不新增告警**，现存那 4 处旧 lint 不用顺手全清。

## Checklist

- [ ] 我跑过测试了吗？（`cargo test --lib` 全过）
- [ ] 我更新文档了吗？（改了行为就要改 README，中英两份都得动）
- [ ] 改动接进真实链路了吗？（改界面不改 `toolchain::resolve()` 等于没改）
- [ ] 注释只解释「为什么」，没有复述「做了什么」的废话
- [ ] 没有提交 `target/`、`nuitka-rs.log`、`settings.json`、`.tmp-verify/`
- [ ] 界面改动附了前后截图（放在 `docs/screenshots/`）
- [ ] 没有把不相关的格式化和旧 lint 顺手混进来

## 界面改动截图

<!-- 改之前 / 改之后，各一张。没改界面就删掉这一节 -->

## 破坏性改动 / 需要重启 / 需要额外安装

<!-- 比如：需要重装 Python、需要重启程序才能扫到新装的 MSVC、改了 settings.json 字段名要迁移。
     没有就写「无」 -->
