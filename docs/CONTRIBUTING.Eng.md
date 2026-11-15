# Contributing

**查看中文文档**：
[中文](CONTRIBUTING.md)

PRs are welcome here, but this isn't a repo where "opened" means "merged". Whether a change lands comes down to two things: it passes the quality gates, and it follows the conventions below.

## Getting started

1. Fork, then branch off `master`. **Don't open a PR straight against `master`** — `master` is protected and the PR has to pass checks before it merges.
2. Run the quality gates below.
3. Open the PR and tick off every box in the template.

Name branches by the kind of change. `patch-1` tells nobody anything:

| Kind | Branch |
| --- | --- |
| New feature | `feature/xxx` |
| Bug fix | `fix/xxx` |
| Docs only | `docs/xxx` |
| Refactor, same behaviour | `refactor/xxx` |

## Quality gates

Same source as the "Build" section in the README. All of these must pass before you submit:

| Command | Where | Requirement |
| --- | --- | --- |
| `cargo build` | `nuitka-rs/` | No errors, no new warnings |
| `cargo test --lib` | `nuitka-rs/` | All green (115 as of now) |
| `cargo clippy --lib` | `nuitka-rs/` | **No new warnings.** Four older lints are still being cleaned up — don't feel obliged to sweep them unless that's exactly what you came for |
| `node --check src/ui/web/app.js` | `nuitka-rs/` | Required if you touched the frontend |

Tests that actually compile an exe are skipped by default. Run them once your local environment is complete:

```bash
cargo test -- --ignored --nocapture
```

## Commit conventions

Conventional Commits — `type(scope): what it does`. Use the module name as scope so it's obvious what moved.

| type | Use |
| --- | --- |
| `feat` | New feature |
| `fix` | Bug fix |
| `refactor` | Refactor, behaviour unchanged |
| `docs` | Documentation |
| `style` | UI / styling, no logic change (usually `feat` does the job — keep `style` for PRs that touch nothing but CSS) |
| `test` | Tests |
| `chore` | Housekeeping: dependencies, build, config |

```
fix(bootstrap): 3.13 自由线程版没有二进制 wheel，换成标准版
feat(settings): Python 与编译器改成绝对路径输入框 + 浏览按钮
```

Common scopes: `bootstrap`, `toolchain`, `scanmsvc`, `scangcc`, `scannuitka`, `python_discovery`, `settings`, `build`, `ui`.

One commit, one thing. Reformatting ten files while smuggling in a feature means ten commits, not one.

## Code conventions

- **Comments explain "why", never "what".** Delete `// clear the list`; keep `// 3.13 起只认 MSVC，所以这里要拦`.
- **Wire the change into the real path.** Editing the UI without touching `toolchain::resolve()` changes nothing — that's what actually decides the toolchain, not the string on screen.
- **Paths are absolute and go through `settings`.** No hardcoded drive letters or usernames.
- **Never put "please install Visual Studio" in the UI.** That's a product red line (planning doc, line 7): when something's missing, offer the installer in-app instead of sending people off to install things.
- **Two compilers only: MSVC or Nuitka's bundled winlibs.** No third route (planning doc, line 8).
- **Chinese first, English as `.Eng.md` siblings** (`README.Eng.md`, `CONTRIBUTING.Eng.md`). The two files link to each other at the top; changing one means changing the other.
- **UI changes cover the whole `src/ui/web` set** (`index.html`, `style.css`, `app.js`), with before/after screenshots dropped into `docs/screenshots/`.

## Don't commit these

`target/`, `nuitka-rs.log`, `settings.json`, `last-form.json`, `.tmp-verify/`, `nuitka-crash-report.xml`, `debug.log`. Those are local run artifacts.

## What to include

| Change | Attach |
| --- | --- |
| Bug fix | Reproduction steps + the matching stretch of `nuitka-rs.log` from the program directory |
| UI change | Before / after screenshots |
| Environment issue | Python version and path, Nuitka version, compiler (MSVC toolset number or winlibs version), OS build |

## What gets sent back

- Formatting or renames with no gain in behaviour or readability
- A new third-party dependency with no explanation of what it buys and what it replaces
- Half a feature, half its tests, "will finish later"
- Unrelated old lints and old names swept up in the same PR
- A PR that never actually ran the quality gates (CI will say so itself)

## On "copy first, merge later"

For larger changes the author keeps a duplicate copy of the project, verifies there, and only then merges back into `nuitka-rs`. You don't have to work that way — but say where you verified and what you verified in the PR.

## Contact

Just open an issue. For security matters, don't use a public issue — email the address on the maintainer's profile.
