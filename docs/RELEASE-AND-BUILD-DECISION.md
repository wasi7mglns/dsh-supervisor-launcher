# 发布与构建决策（壳仓视角）

> 本文是**壳仓**侧的发布规范。**跨仓时序与契约**的权威定义在内核仓
> `release/README.md` §0（两仓构建决策）与 §0.2（跨仓发布时序）—— 本文与之保持一致，冲突时以内核仓为准。
> 最近更新：2026-09-13。

## 1. 为什么是两个仓（**必须分开，不是历史包袱**）

| | 内核仓 `advgyxqamf/dsh-supervisor-core`（公开） | 壳仓 `wasi7mglns/dsh-supervisor-launcher`（公开，本仓） |
|---|---|---|
| 职责 | 产品逻辑 + 守护：API/路由/relay/实例/插件/端口/更新编排 | **仅**桌面体验：引导页、托盘、安装程序、原生能力 |
| 技术栈 | JS（CommonJS），运行时依赖 **0**、原生扩展 **0** | Rust（Tauri 2）+ TS/React |
| 产物 | npm 平台子包 `@dsh-sup/dsh-core-*`（4 平台） | 安装程序 `deb/rpm`、`dmg/app`、`msi/nsis` + `@dsh-sup/shell-*` |
| 分发 | npm registry | GitHub Release + npm（自更新产物） |
| 节奏 | 高频、可单独 hotfix | 低频（安装程序） |
| 构建负担 | 轻（纯 JS，一次构建派生四平台） | 重（Rust + 各平台系统库） |

**四个具体好处**：① 更新节奏解耦（内核热修不必重发安装程序）；
② 用户更新成本（内核走 npm 增量/可静默，壳走安装程序/需重启）；
③ 构建负担隔离（Rust/Tauri 依赖不污染内核的「零依赖纯 JS」）；
④ **为后期决策留空间**（两仓可独立决定开源策略、节奏、商业形态）。

**代价与对策**：不共享代码 → 契约只能靠**文件**传递（`registry.json` / `identity.json` /
`update-guard.json` / `update-journal.json`）。字段**新增**须向后兼容；
字段**移除或改语义**须**内核先行**，并允许两侧版本错配运行一个发布周期。

## 2. 壳的构建与发布 SOP

### 2.1 版本（三处互锁，单源校验）

```bash
bash scripts/bump-shell.sh <ver>        # 同号写入 Cargo.toml / tauri.conf.json / Cargo.lock
node scripts/verify-shell-versions.js   # 自洽校验（不一致即失败）
```

### 2.2 本地自证（发布前）

```bash
cd src-tauri && cargo test            # 全部门禁（含跨平台导入门禁、CI 覆盖性门禁）
cargo check --all-targets             # 0 警告
```

### 2.3 发布

```bash
git add -A && git commit -m "release: v<ver>" && git tag v<ver>
git push origin main && git push origin v<ver>
```

→ tag 触发 `launcher-build.yml`：**四平台完整构建**
（`ubuntu-22.04` / `windows-latest` / `macos-latest` / `macos-15-intel`）
→ Tauri bundle + 壳 npm 包 + `shell-manifest.json` + 验签
→ `publish` job **仅 tag 触发**（挂 GitHub Release + `npm publish`）。

## 3. CI 触发策略（2026-09-13 定稿）

| 事件 | 行为 |
|---|---|
| **push `main`** | 跑**完整构建矩阵**（四平台 bundle + 全部门禁测试）。不打包发布、不发布。 |
| **push tag `v*`** | 同上 + `publish`（Release + npm）。 |
| `workflow_dispatch` | 同 push main（可手动触发验证）。 |

> **为什么 push 也跑完整构建**：只做编译校验（`cargo check`）发现不了
> **打包 / 签名 / 产物装配**阶段的问题，而本仓恰在那些阶段踩过坑；轻量检查会给出
> 「绿了」的假象、反而掩盖问题。公开仓 Actions 免费，故一律跑完整构建。
> 实证：该策略第一次运行（run 34737146315）就抓到 `macos.rs` 的语法错误
> （嵌套未转义双引号 → macOS 无法出包），该错误在 Linux 上因 `#[cfg(target_os)]`
> 完全不可见。

> ⚠ **门禁必须自动枚举**：CI 的门禁步骤用 `ls tests/*.rs` 自动列出全部 test target
> （仅排除需打包产物的 `updater_artifacts`）。曾经的硬编码 `--test` 名单导致
> **新增门禁被静默排除在 CI 之外**（实测漏 4 个）—— `tests/ci_gate_coverage_test.rs`
> 现在锁死「不得硬编码 + 必须有 mac/win 构建」。

## 4. 跨仓发布时序（规范）

```text
1) 内核先发（契约变更方）
2) 壳后发（消费方），至少在「内核那一版已发布」之后
3) 交叉验证：
     · 壳=最新 / 内核=上一版  → 验证降级路径
     · 壳=上一版 / 内核=最新  → 验证向后兼容
```

## 5. 平台矩阵要点

| 平台 | runner | 产物 |
|---|---|---|
| linux-x64 | `ubuntu-22.04`（**基座固定**，glibc 2.35）| `deb,rpm` |
| darwin-arm64 | `macos-latest` | `app,dmg` |
| darwin-x64 | `macos-15-intel`（**macos-14 已弃用**）| `app,dmg` |
| win-x64 | `windows-latest` | `nsis,msi` |

> Linux **必须**用 ubuntu-22.04 基座：在 24.04（glibc 2.39）构建的产物**无法**在
> 22.04 / Debian 12 运行（Rust std 对 `pidfd_spawnp`/`pidfd_getpid` 的弱引用
> 在 2.39 主机会被解析成硬性 `verneed`）。
