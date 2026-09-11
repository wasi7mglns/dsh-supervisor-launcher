# 桌面壳引导逻辑深度检测报告

> ## ⚠ 本文件是**历史审计快照**（2026-09-11 之前），不是现状描述
>
> 其中的**结论与行号均已被后续修复推翻/移位**，例如：
>   · `core_install()` —— 该命令**已不存在**（现为 `core_apply` + `core_plan`，带版本仲裁）；
>   · `core_status()` 现返回**版本字段**（不再只有 `{installed, hint}`）；
>   · `locate_core()` 现**按版本仲裁**（不再只判 `is_file()`）；
>   · 引用的行号（`main.rs:180-187` 等）因后续分层重构**全部失效**。
>
> 保留它的价值：记录「当时有哪些缺陷、怎么发现的」。**要看现状请读代码**
> （`src/commands/` + `src/domain/`）与 `docs/DESIGN-COMPLETE.md`。

> 当时范围：`dsh-supervisor-launcher`（壳仓）全部引导代码 —— 907 行 Rust + 312 行 HTML。
> 逐行取证，附行号。结论先行：**壳的引导逻辑「能跑通首次安装」，但「重启后强制更新内核」完全缺失，且有 4 处可致假成功/死锁的结构性缺陷。**

---

## 一、直接回答你的问题

> 「完整退出管家后重启，是否检测最新内核并强制更新，而不是跳过？」

### 答案：**不是。壳完全没有「内核版本」这个概念，只判断「内核文件在不在」。**

| 环节 | 现状 | 结论 |
|---|---|---|
| 壳检测内核 | `core_status()` 只返回 `{installed: bool, hint}`（main.rs:180-187） | **无版本** |
| 壳定位内核 | `locate_core()` 只做 `is_file()` 存在性判断（main.rs:163-177） | **无版本** |
| 壳安装内核 | `core_install()` 仅在 `!installed` 时调用（bootstrap.html:161-165） | **只补缺，不升级** |
| 重启后行为 | 内核存在 → 直接 `stepGuardDone` → 进面板 | **跳过一切更新** |
| 内核自身更新 | `/self-update/*`（guard.js:139-149）需**用户在面板手动**点「检查更新」+「更新」 | **非自动、非强制** |

**即：你退出管家→再启动，只要旧内核还在磁盘上，壳就会直接用它——无论它落后多少个版本。**
"强制更新"这个语义在整个壳仓**不存在**。

---

## 二、引导链路全景（数据流）

    用户启动壳
      │
      ├─ Tauri 主窗口 → shell.html（唯一窗口栏 + 内容 iframe）
      │     └─ iframe → bootstrap.html（引导页 JS 状态机）
      │
      └─ 同时：Rust setup 线程（main.rs:472-501）
            probe_system_node → latest_lts → ensure_guard → emit env_ready

    bootstrap.html 四步状态机：
      stepEnv   → node_status → 缺 Node 则 start_node_install → stepNodeWait
      stepNodeDone → stepGuard
      stepGuard → core_status → 缺内核则 core_install → stepGuardWait
      stepGuardDone → stepPanel → finish_boot → go_panel(emit shell:goto-panel)

    shell.html 收到 shell:goto-panel → iframe.src = 守卫面板 URL

**关键结构问题：这里有两条互不知晓的并行流程**（Rust setup 线程 vs JS 引导页），它们没有汇合点。

---

## 三、问题清单（按严重度）

### 🔴 严重

#### K1. 内核版本检测 / 强制更新完全缺失（你的核心诉求）
- `core_status` 无版本字段；`locate_core` 无版本仲裁；`core_install` 只在缺失时触发。
- 内核有自更新能力（`/self-update/status|apply`），但**壳不知道、也不调用**。
- 后果：**旧内核会一直用下去**，除非用户手动进面板点更新。

#### K2. `stepGuardWait` 超时后**无条件假成功**
```js
// bootstrap.html:177
setTimeout(function () { clearInterval(t); coreOk = true; resolve(stepGuardDone()); }, 600000);
```
- 内核安装失败/卡住时，10 分钟后仍被当作成功 → 进面板 → 守卫不存在 → **面板连接失败页面**。
- `coreOk = true` 是硬编码的假成功，掩盖真实失败。

#### K3. Rust setup 线程与 JS 引导页**双流程并行 + 事件契约断裂**
- Rust 侧 `emit("env_ready")`（main.rs:489）—— **引导页从未监听**（只 listen `env_progress/env_done/env_error`，bootstrap.html:204-209）。
- 且 `env_done` / `env_error` 的处理器是**空函数**（bootstrap.html:208-209）。
- `ensure_guard` 的成败（`env_ready.core`）被完全丢弃；JS 只凭 `core_status.installed` 决定进面板。
- 后果：守卫启动失败时，引导页**毫不知情**，照样进面板。

#### K4. `core_install` fire-and-forget 且**吞掉全部错误**
```rust
// main.rs:208-213
let _ = Command::new(npm).args(["install","-g","--no-audit","--no-fund",&pkg])
    .stdout(Stdio::null()).stderr(Stdio::null()).spawn()   // 不等、不看输出、不看退出码
```
- npm 失败（无网/权限/包不存在）没有任何可观测信号。
- 引导页只能靠 `core_status` 轮询，失败时永远轮不到 → 走到 K2 的 10 分钟假成功。

### 🟠 中

#### K5. `locate_core` 多候选**无版本仲裁**
```rust
// main.rs:163-177：PATH → ~/.local/bin → ~/.npm-global/bin → resource_dir
if let Some(p) = env::find_in_path(name) { return Some(p); }  // 第一个命中即返回
```
- 若 `PATH` 里是**旧内核**、`~/.local/bin` 是新内核，**旧内核胜出**。
- 无「取最高版本」逻辑。

#### K6. **守卫就绪竞态**：面板可能早于守卫启动就跳转
- JS 节奏：`stepNodeDone`(450ms) → `stepGuardDone`(450ms) → `stepPanel`(900ms) → `finish_boot` ≈ **1.8s** 就导航。
- 守卫启动（systemd start + Node 引导）常需 1~3s，可能**慢于 1.8s**。
- 且 `go_panel` 三次重发同一 URL（400/1200/2500ms），但 shell.html:92 `if (u !== frame.src) frame.src = u;` —— **首次导航后 src 已相同，后续重发被忽略 → 加载失败无重试**。

#### K7. 引导页**无失败恢复入口**
- 注释定稿「全程自动，无任何用户操作按钮」（bootstrap.html:11）。
- `boot().catch(e => status("检测异常: "+e))`（bootstrap.html:199）只改文字，**无重试按钮**。
- 任一环节失败即**死锁在引导页**，用户无路可走。

#### K8. 壳 ↔ 内核**无版本兼容检查**
- 壳 `0.1.0`（tauri.conf.json），内核 `0.1.2-BETA.7`。
- 内核 API 演进后壳可能不兼容（如 `apiPort` 配置结构变化），无任何检测/告警。

### 🟡 低

#### K9. `skip_env_upgrade` 是**死命令**
- 注册于 `generate_handler!`（main.rs:458），但引导页/壳页**从未 invoke**（HTML 中无引用）。

#### K10. Node 版本策略：仅最低门槛，不强制最新 LTS（**设计如此**）
- `meets_minimum`（>=22.12）即放行（main.rs:483）；`latest` 仅展示。
- 与「内核强制更新」的诉求不一致——至少内核应更严。

#### K11. `node_status` 的后台 `latest_lts()` 拉取**无节流**
- 每次 `latest` 为空即起线程（main.rs:53-63）；网络失败时 `latest` 恒空。
- 引导页 600ms 轮询 → **每 600ms 一次网络请求**（`busy` 期间被挡，失败重试期不设防）。

---

## 四、代码结构健壮性评估

| 维度 | 评价 |
|---|---|
| 分层 | 尚可：`main.rs`（命令/窗口）/ `env.rs`（环境探测）/ `node.rs`（Node 引导）职责基本清晰 |
| 状态一致性 | **差**：Rust `RunState` 与 JS 状态机各持一份「进度真相」，无单一事实源 |
| 错误传播 | **差**：`core_install` 吞错、`stepGuardWait` 假成功、事件处理器为空 |
| 事件契约 | **断裂**：`env_ready`/`env_status` 无人监听；2 个监听器是空壳 |
| 就绪语义 | **缺失**：无「守卫就绪」握手，面板导航靠固定延时 |
| 版本意识 | **缺失**：壳对内核对版本零感知（无当前版本、无最新版本、无兼容） |
| 降级/恢复 | **缺失**：失败无重试、无跳过、无诊断出口 |
| 平台抽象 | 尚可：`start/stop_guard_service` 已按 Linux/macOS/Windows 分派（阶段 1 成果） |

**总评：壳的引导逻辑是「乐观单行道」——假设每一步都成功，失败无处可去；且完全没有版本治理能力。**

---

## 五、修复设计（建议，待你拍板）

### 5.1 补上「内核版本治理」（解决 K1/K5/K8）

壳（Rust）新增能力：
1. **读本地内核版本**：执行 `dsh-supervisor --version`（已可用，实测输出 `dsh-supervisor v0.1.2-BETA.7`），或读 npm 包 `package.json`。
2. **查最新内核版本**：`ureq` 直接 GET npm registry（`https://registry.npmjs.org/@dsh-sup%2Fdsh-core-<plat>-<arch>`）取 `dist-tags.latest`；官方不可达走镜像。
3. **版本比较 + 强制更新**：`latest > installed` → `npm i -g @dsh-sup/dsh-core-<plat>-<arch>@<latest>` → 重启守卫 → 进面板。
4. **多候选按版本仲裁**：`locate_core` 收集全部候选，取版本最高者（解决 K5）。

**策略选项（需你定）**：
- **A. 强制更新（你描述的）**：检测到更新即装，无跳过。风险：新内核不兼容时无法回退。
- **B. 强制更新 + 失败回退**：装失败/启动失败 → 回退旧版本可用（推荐，工业级）。
- **C. 提示更新**：引导页展示「发现新版本」，默认自动更新但保留「暂不」入口。

### 5.2 修复「假成功」（解决 K2/K4）
- `stepGuardWait` 超时**不得**置 `coreOk = true`；改为明确失败态。
- `core_install` 改为**可观测**：捕获 exit code + stderr，失败回传引导页。

### 5.3 统一「就绪握手」（解决 K3/K6）
- 删除 Rust setup 线程的并行 `ensure_guard`（或改为引导页显式驱动）。
- 新增 `guard_ready()` 探针（壳轮询 `is_alive(port)` + `/healthz`），**就绪后才** `finish_boot`。
- shell.html 的 `shell:goto-panel` 改为**带就绪确认**，并修正「同 URL 不重载」导致的失败无重试。

### 5.4 补「失败恢复 UI」（解决 K7）
- 引导页失败时显示：失败原因 + **重试** + **诊断日志导出**（K10 允许的最小用户操作）。

### 5.5 清理（解决 K9/K11）
- 删除 `skip_env_upgrade` 或接入引导页；`node_status` 的 `latest_lts` 加节流/单飞（in-flight 标记）。

---

## 六、结论

1. **你问的「重启后强制更新内核」——当前完全没做**，壳对内核版本零感知。
2. 引导逻辑**不够健壮**：4 处严重缺陷（无版本治理、超时假成功、双流程+事件断裂、安装吞错），2 处中危（就绪竞态、无恢复 UI）。
3. 代码结构**基本分层但缺横切关注点**：无单一状态源、无错误传播、无版本意识、无就绪握手。
4. 修复方向明确：**内核版本治理 + 就绪握手 + 错误传播 + 失败恢复**，共 5 组改动。

**建议按 5.1(B 强制更新+失败回退) → 5.2 → 5.3 → 5.4 → 5.5 实施。**
