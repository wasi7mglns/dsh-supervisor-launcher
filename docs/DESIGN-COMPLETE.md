# 桌面壳 ↔ 内核：完整架构理解与一次性执行方案

> 2026-09-11 · 基于**全量代码阅读**（内核 80 文件 / 19354 行 + 壳 Rust 9 文件 / 4385 行 + 前端 2 文件 + 测试与文档）。
> 本文件取代此前的分阶段提案，给出**一次性目标态**。
> 所有结论均带 `文件:行号` 证据；未核实的一律标「未确认」。

---

## ⚠ 执行状态（本方案已执行 —— 请先读本节）

本文件写于**执行之前**，其中的「缺陷清单」「重叠面」描述的是**改造前状态**。
这些缺陷**绝大部分已修复**；下文表格里的 `文件:行号` 是**审计时快照**，行号因重构已移位。
下表是唯一事实源；要看现状请直接读代码。

### 批 A–F

| 批 | 内容 | 状态 |
|---|---|---|
| **A** | 平台适配层（43 处平台分支 → 2 处白名单例外）| ✅ 完成 |
| **B** | 分层：`main.rs` 1420 → 469 行，拆出 `commands/` + `domain/` | ✅ 完成 |
| **C** | 结构化错误 `ShellError` | ✅ 完成（曾因 `mod error;` 丢失成孤儿文件，已修复并接线）|
| **D** | 契约层 M1–M5（镜像目录归壳 / 探测统一 / Node 门槛 / 版本向量）| ✅ 完成 |
| **E** | 前端拆分 + 全局错误上报 + IPC 自检 + 门禁 G5 | ✅ 完成（802 行单块 → **9 个模块**；`bootstrap.html` 963 → 221 行；B53/G5 已适配且**注入验证能失败**）|
| **F** | 内核缺陷 K1–K10 + M6 有界执行 | ✅ 完成（含 K3 状态源合并）|

### 壳侧缺陷 S1–S5

| # | 内容 | 状态 |
|---|---|---|
| **S1** | 分层违规（服务定义与启停分居两层）| ✅ 并入 `platform::ServiceControl` |
| **S2** | `main.rs` 单体 1487 行 | ✅ 469 行 + 门禁 G3 锁定 |
| **S3** | 42 处 `Result<_, String>` | ✅ IPC 边界统一 `ShellResult`；内部函数保留字符串（有意渐进）|
| **S4** | `bootstrap.html` 单块 JS | ✅ 已拆为 9 个模块（责任单一、逐文件语法检查）；全局 onerror + 启动自检保留在 `00-runtime` |
| **S5** | 43 处平台分支散落 8 文件 | ✅ 收拢至 `platform/`（门禁 G1）|

### 内核缺陷 K1–K10

| # | 内容 | 状态 |
|---|---|---|
| **K1** | daemon 脚本路径全断（生产致命）| ✅ 修（`platform/srcpath.js` + 门禁 G10）|
| **K2** | `platform/exec.js` 零引用 + 23 处无 timeout | ✅ 修（18 处绕过执行器的调用全部迁入 + 门禁 G9）|
| **K3** | 双生命周期状态源 | ✅ 修 —— `managed.js` 曾自建一份分叉副本（多了死词 `degraded`、少了 `backoff`/`failed`/`restarting`/`installing`）；现改为引用 canonical（同一数组引用）。回归 `test/phase-vocabulary-test.js`（8 项）|
| **K4** | `ManagedLifecycle.start` 忽略回调 `ok:false` | ✅ 修（`start`/`stop` 均尊重显式失败；保留「无 ok 字段=成功」兼容）；回归 `test/managed-lifecycle-failure-test.js`（13 项）|
| **K5** | 影子决策未建模 `_crashHalted` | ✅ 修（快照补 `crashHalted`/`sessionHalting`，与 `_shouldRun()` 同序）；回归 `test/shadow-decision-test.js`（9 项）|
| **K6** | `originAllowed` 只比端口不校验 host | ✅ 修（补 Host 闸 + 壳 origin；行为级断言）|
| **K7** | `ports.js` 用 `HOME` 兜底 `/tmp` | ✅ 修（改 `os.homedir()`）|
| **K8** | 平台命令泄漏到业务层 | ✅ 修（下沉 `platform/os/netinfo.js`）|
| **K9** | 版本解析正则 `[^s]` | ✅ 修（改 `[^\s]`）|
| **K10** | 卸载失败仍删 manifest | ✅ 修（成功才删）|

### 重叠面 O1–O7

| # | 内容 | 状态 |
|---|---|---|
| **O1** | npm 镜像目录 3 份副本 | ✅ 壳拥有，内核留 2 条最小兜底 |
| **O2** | 镜像探测方法分叉（选源不一致）| ✅ 探测规格随契约投放，两侧同法 |
| **O3** | npm 安装执行 | ✅ 规格共享（执行各自保留，见 D2）|
| **O4** | Node 最低门槛不一致 | ✅ 内核三态判定 + 读壳投放的 `minNode` |
| **O5** | 平台原语 | ✅ 内核侧统一执行器 |
| **O6** | 版本语义 3 处分歧 | ✅ 共享测试向量（两仓逐字节相同）|
| **O7** | 自启/服务定义所有权 | ✅ 已定案（所有权矩阵）|

### 门禁（均已**实测能失败**）

| 门禁 | 内容 | 仓库 |
|---|---|---|
| G1 / G1-2 | 平台分支只在 `platform/`；`all_rust_sources` 必须递归 | 壳 |
| G2 / G3 | 命令层只做委托；`main.rs` 上限且不含 IPC 命令 | 壳 |
| G5 | 前端每个内联 script 独立语法 + F2/F3 断言 | 壳 |
| G9 | 仅 `platform/exec.js` 可调用 `execFileSync` | 内核 |
| G10 | 受管 daemon 脚本路径必须可解析 | 内核 |

**已全部执行**：本文件描述的缺陷清单（K1–K10 / S1–S5 / O1–O7 / 批 A–F）均已处理。
---

## 审计后修复（2026-09-12，全仓架构与跨平台审计）

对两仓做了一次全量只读审计（3 路并行 + 逐条复核），发现并修复 6 个 P1：

| # | 位置 | 缺陷 | 修法 | 回归测试 |
|---|---|---|---|---|
| **P1-A** | 壳 `update.rs` | 自更新护栏**永久自锁**：`attempt` 只在成功时归零，而成功路径被 `attempt>=2` 掐断 → 用户永久收不到更新 | 改为**时间冷却**（6h）+ `reset_guard()` 显式恢复入口 + 前端可见提示 | `update_guard_test.rs`（5）|
| **P1-B** | 两仓 CI | 壳 59+ 门禁与内核 `npm test` **从未在 CI 执行**（README 却声称有保障）| 壳入三平台矩阵；内核补 ubuntu `test` job + 日常触发 | CI 命令本地复现 |
| **P1-C** | 内核 `dist`/`native` | Windows 上裸 npm 不做 PATHEXT 解析 → 升级/安装/卸载全失败 | `exec-path.npmBin()` 唯一解析入口（Windows → `npm.cmd`）| `npm-resolution-test.js`（10）|
| **P1-D** | 壳 `windows.rs` | 计划任务 `/TR` 未加引号 → 用户名含空格时**登录自启静默失效** | 值内嵌引号（与内核已有写法一致）| `bootstrap_flow::b56` |
| **P1-E** | 内核 `api/index.js` | 注释声称支持 RFC1918，实为只查回环 → 开局域网后**写操作全 403** | 复用 `identity.isPrivateIpv4`，双闸纳入私有网段（**不放宽公网拒绝**）| `lan-access-boundary-test.js`（20）|
| **P1-F** | 内核 `native/manager.js` | 卸载 npm **无超时** → 一次挂起即永久锁死安装/卸载 | 15min 看门狗 + `killTree` + `finally` 释放锁 + `timedOut` 上报 | 静态（12）+ **行为级**（8）|

### 附带修复

- `40-shell-update.js` 的 `showUpdChoice` 曾被**重复定义两次**（前次拆分遗留）—— 已去重；
- `api/index.js` 闸门上方注释与文件头的信任集合声明**自相矛盾** —— 已校准；
- 新增 `test/test-safety-gate-test.js`：禁止测试用「patch 模块导出」伪造依赖
  （`const { x } = require()` 是值绑定，patch 无效 → 会跑真实副作用；已发生一次真实事故，所幸 no-op）。

### 验证

```
内核  npm test            61 文件 / 1153 断言 / 0 失败
壳    cargo test          83 项 / 0 失败（12 + 60 + 5 + 6）
壳    cargo check         0 警告
壳    前端 9 模块语法      9/9 通过
```

每条修复都配了**注入 → 失败 → 还原 → 通过**的验证；门禁均实测能失败。
### 第二轮：P2/P3 批量修复（2026-09-12 续）

| # | 位置 | 缺陷 | 回归测试 |
|---|---|---|---|
| **G1 盲区** | 壳 `tests/bootstrap_flow.rs` | 门禁只拦 `#[cfg(` **属性**，看不见 `cfg!()` **宏** → 平台/ 之外实有 3 处生产平台分支（env/core/coreloc）| B59（含反向自检）|
| **hint 未序列化** | 壳 `error.rs` | `hint()` 只是 Rust 方法、从未进 JSON → 前端 `e.hint` 恒 undefined，可操作建议永远到不了用户 | 2 项内嵌单测 |
| **B45/B46 空转** | 壳 `tests/bootstrap_flow.rs` | 断言读的是**注释**（实现已下沉 platform/）→ 门禁永久为真，拦不住真实缺陷 | 注入验证：改坏实现即 FAIL |
| **服务定义只创建不更新** | 壳三平台 `ensure_defined` | 「已存在即返回」→ 模板演进后老用户永远跑旧定义（P3 引号修复因此到不了已装用户）| B60（静态）+ B61（行为级）|
| **GBK 丢诊断** | 壳 `bounded.rs`/`core.rs` | `read_to_string` 遇非 UTF-8 返回空 → 中文 Windows 上失败详情全丢 | B62 |
| **重复执行器** | 壳 `core.rs` | 复制了 bounded.rs 的完整实现且**行为已分叉**（漏 stdin(null)/曾用毫秒名/漏 prepare）| B63 |
| **内核死代码** | `supervisor.js`/`settings-view.js` | 死导入 child_process（G9 盲区）· 死方法 guardSelfUpdateDir | — |
| **systemd 引号** | 壳 `platform/linux.rs` | `ExecStart` 路径未加引号 → 家目录含空格时命令被拆断（systemd-analyze 实测确认）| B58 |

⚠ **两处审计误判已被拦下**（未采纳）：
  · 「`self-update.js` 整文件死代码」—— 实际被 `guard-update-test.js` require、API 面在线；若照删会破坏守卫自更新；
  · 我一度认定「内核 CI 不跑 npm test」—— 实际 `ci-core.sh [2/5]` 会跑（只是无 Linux runner）。

两处均通过**逐条复核**发现，印证「子代理报告不构成证据」。
### 第三轮：剩余 P2/P3（2026-09-12 续）

| # | 位置 | 缺陷 |
|---|---|---|
| **G9 盲区** | 内核 bin/dsh-supervisor | 7 处裸 execFileSync（**无 timeout**）—— G9 只扫 src/，看不见 bin/；dbus 挂起时 CLI 无限阻塞 |
| **死字段** | 内核 api/identity.js | trusted 每请求计算但零消费。⚠ **不能接线**：接到 access-key 门卫会让局域网**豁免**密钥 = 安全降级；故**删除** |
| **架构静默当 x64** | 内核 dist._platformTag / 壳 linux.rs | 非 arm64 即 x64 → ppc64le 等按 x64 取产物（轻则 404，重则架构不符）；改白名单 + 未支持即报 |
| **XML 转义** | 壳 macos.rs / 内核 autostart.js | plist 是 XML：真正会破坏它的是 & 与尖括号，而旧实现只转义双引号（XML 里本就合法）|
| **Exec 未加引号** | 内核 .desktop | Desktop Entry 规范也是空格分词；家目录含空格时 Exec 被拆断 |
| **clippy 16 条** | 壳 | 其中一条是**真问题**：B12 的 while let 循环体必定 panic 且 idx 从不更新 → 循环只跑 0/1 次（形态伪装成扫描全部）|

⚠ **又发现两条既有测试把缺陷当成了要求**：
autostart-ownership-test.js 的 P3-e/P3-f 断言的是「裸拼接 GUI_LABEL」与
「用 replace 双引号当 XML 转义」—— 即测试**锁定了错误实现**，故修复后变红。
已改为断言真正的 XML 转义。这与前两轮的「门禁空转」是同一族问题的另一面：
**门禁不仅可能空转，还可能把 bug 固化为规范。**

### 三轮累计验证

```
内核  npm test   64 文件 / 1187 断言 / 0 失败（起点 1098，+89）
壳    cargo test  92 项 / 0 失败（起点 77，+15）
壳    cargo check 0 警告      clippy 0 警告（排除 items-after-test-module 风格项）
```


---

# 第一部分　系统全貌

## 1. 三个进程，两种角色

```
┌─────────────────────────────────────────────────────────────────────┐
│  ① 桌面壳 dsh-supervisor-gui（Tauri + Rust，4385 行）              │
│     · 角色：**安装器 + 显示框**（供给层）                           │
│     · 引导：探测环境 → 装 Node → 装内核 → 定义守卫服务 → 启守卫     │
│     · 常驻：托盘；关窗=隐藏，「退出管家」=停全部服务链              │
│     · 壳进程崩了会怎样 → 由 ③ 的「壳看护」拉起（2026-09-11 新增）   │
└──────────────┬──────────────────────────────────────────────────────┘
               │ ① 装 ②（npm 全局包）  ② 启动/停止 ③（服务管理器）
               │ ③ 读 config.json     ④ 把 webview 指向 ② 的面板
               ▼
┌─────────────────────────────────────────────────────────────────────┐
│  ② 守卫 dsh-supervisor daemon（纯 JS，0 依赖，19354 行）            │
│     · 角色：**系统服务 + 面板后端**（运行期层）                     │
│     · 无头运行：systemd Restart=always / launchd KeepAlive /       │
│                schtasks；Linux 经 linger 在**用户注销后仍活**       │
│     · 托管面板：http://127.0.0.1:<apiPort>/（浏览器可直开）         │
│     · 保活 ③、装/升级 DSH 与插件、智能路由、局域网反代              │
│     · 壳看护：壳崩溃时把它拉起来（三平台一套机制）                  │
└──────────────┬──────────────────────────────────────────────────────┘
               │ 保活 / 监督 / 重启
               ▼
┌─────────────────────────────────────────────────────────────────────┐
│  ③ DeepSeek Harness（DSH，第三方，node dsh web）                    │
│     · 被监管目标（原生单例 + 多个 sandbox 实例）                    │
└─────────────────────────────────────────────────────────────────────┘
```

**关键推论（决定了后面全部归属判定）**：

| # | 事实 | 证据 | 推论 |
|---|---|---|---|
| **F1** | 用户装壳时，**内核还不存在** | 产品形态（空壳） | 装内核之前/期间需要的能力，**必须在壳** |
| **F2** | 内核是**系统服务**，壳关窗它照跑 | `service.rs` 三平台服务定义 + linger | 运行期需要的能力，**必须在核** |
| **F3** | 内核可在**无图形会话**时运行 | `service.rs:96` `loginctl enable-linger`；注释「未登录也保持用户服务」 | 内核**不得**依赖壳/GUI 在场 |
| **F4** | 面板由**内核** HTTP 托管，壳只是 webview 框 | `api/index.js:40-56` `resolveUiDir()`；README「浏览器可直接打开」 | 面板功能不得依赖壳在场 |
| **F5** | 壳与内核**语言不同**（Rust / JS） | 工程事实 | 「抽到壳里」只能：整体搬迁 / 壳定义+内核消费产物 / 统一规格+测试向量 |

## 2. 生命周期所有权矩阵（2026-09-11 定案）

| 对象 | 生命周期所有者 | 启动 | 停止 | 重启权威 |
|---|---|---|---|---|
| **桌面壳** | 桌面会话（登录 autostart） | XDG `.desktop` / LaunchAgent / schtasks | 用户关窗（隐藏）/ 托盘退出 | **守卫看护**（崩溃时）|
| **守卫** | **服务管理器**（唯一） | 壳请求 `systemctl --user start` / `launchctl` / `schtasks /Run` | 壳请求（退出握手后） | 服务管理器（`Restart=always` 等）|
| 原生 DSH | **守卫** | 守卫 spawn（进程组） | `stopProcess` | 守卫（desired+guardian）|
| 沙箱实例 | **守卫** | `systemd-run`（Linux 专有） | `systemctl stop` | 守卫监督 |
| router/lan daemon | **守卫** | `DaemonLifecycle` spawn | `.stop()` | 守卫监督 |

**铁律**：每个进程**有且只有一个**生命周期所有者；非所有者只能「请求所有者」。

## 3. 引导时序（壳的六步 + 内核的介入点）

```
阶段                 执行者   调用                              产物
─────────────────────────────────────────────────────────────────────────────
1 检测环境           壳       nodeprobe::status                候选枚举 + 版本
2 运行环境           壳       node::install（pkexec/msiexec）  官方 Node LTS
                                                        → 壳写 runtime.json
3 桌面版本           壳       Tauri updater + minisign         壳自更新
4 内核版本           壳       core::latest_version → install   npm 装内核
                                                        ← 内核被装上
5 守卫就绪           壳       service::ensure_defined + start  服务定义 + 启动
                      内核     （被服务管理器拉起）              API 就绪
                      壳       guard_ready（轮询 /healthz）      握手
6 控制面板           壳       window → shell.html → iframe      指向内核面板
                      内核     托管 supervisor.html + assets
```

**第 4 步前的全部能力属于壳（F1）；第 5 步后的全部能力属于内核（F2）。**

---

# 第二部分　内核架构（实测）

## 4. 分层与真实依赖

```
bin/dsh-supervisor（651 行，宿主+CLI）
        ↓
src/supervisor.js（1188 行，组合根/唯一状态宿主）
   ├─ api/index.js（328 行，HTTP 网关：信任门卫→域分派→静态）
   │     └─ api/<9 域>（仅 owns()+handle(ctx)，**零跨域依赖**，干净的依赖倒置）
   ├─ guard/（5042 行，守卫自身：生命周期/监督/端口/原生）
   ├─ domains/（8453 行，业务：instance/router/relay/plugin/dist/shell）
   └─ platform/（3141 行，基础 + os 平台抽象层）
```

**实测的平台分支分布（同口径：`process.platform` + `isLinux/isMac/isWindows`）**：

| 位置 | 处数 |
|---|---|
| `platform/os/` 内 | **60**（90%）|
| `platform/os/` 外 | **7** —— `domains/relay/frpmgr.js` 4 / `guard/supervisor/settings-view.js` 2 / `supervisor.js` 1 |

→ **内核有平台抽象层，且集中度好。** 这是壳缺失而内核已有的东西。

## 5. 监督循环（三层 + 事件旁路）

```
单一定时器 setInterval(probeIntervalMs=5s)     supervisor.js:457-463（_heartbeatBusy 防重入）
        ↓
ManagedRegistry.heartbeat(5000)                objects.js:289-324
        ↓  遍历受管对象，按 tickEvery 节流
  ├─ dsh            tickEvery=1  → _dshSuperviseOnce → _dshConverge（唯一状态机）
  ├─ sandbox-instance tickEvery=1 → InstanceManager.supervise
  ├─ router-daemon  tickEvery=6（≈30s）→ DaemonLifecycle.classify/ensure
  └─ lan-daemon     tickEvery=6（≈30s）→ 同上

旁路：setDesired/requestRestart → 立即 tick()（_ticking 防并发）
    退避不是独立定时器，而是 converge 内的 backoffDue/restartDue 时间闸
独立定时器：自更新检查（20s 首查 + 1h 周期）、壳看护（20s）
```

**注意（结构债）**：存在**两套并行生命周期状态源**，靠三个手工镜像函数对账：

| 状态源 | 文件 | phase 词表 |
|---|---|---|
| `LifecycleManager` | `guard/lifecycle/managed.js:19` | stopped/starting/running/draining/degraded |
| `ManagedRegistry` | `guard/lifecycle/objects.js:23` | + installing/backoff/failed/restarting |

对账函数：`supervise-view.js:52` `_syncDshLifecycleView` / `control-view.js:583` `_syncRouterLifecycleView` / `control-view.js:606` `_syncInstancesLifecycleView`。

## 6. 跨仓契约面（全清单）

| 契约 | 类型 | 方向 | 格式/语义 |
|---|---|---|---|
| `~/.dsh/supervisor/config.json` | 文件 | 内核写 / **壳读** | `apiPort`、`closeAction`(hide\|exit)、`apiAccessKey`、`shellWatchdog`…（**壳读内核配置的唯一入口**：`env.rs:229 config_json()`，一律真 JSON 解析，禁字符串扫描）|
| `~/.dsh/supervisor/runtime.json` | 文件 | **壳写** / 内核读 | `{nodeVersion,nodePath,installedAt,source}`（`node.rs:378`；`settings-view.js:40` 读）|
| `~/.dsh/supervisor/registry.json` | 文件 | **壳写** / 内核读 | 镜像源 `{mode,origins,manualOrigin}`（`mirror.rs:191 export_to_kernel`；`supervisor.js:192` 读）|
| `~/.dsh/shell/identity.json` | 文件 | **壳写** / 内核读 | `{version,phase,pid,exe,attempt,pinned,…}`（`update.rs:187`；`domains/shell/index.js:45` 读）|
| `~/.dsh/shell/update-journal.json` | 文件 | **内核写** / 壳读 | 更新账本 pending/confirmed/pinned（`domains/shell/index.js:50`）|
| 守卫服务定义 | 文件 | **壳写** / 服务管理器读 | systemd unit / LaunchAgent plist / schtasks（`service.rs:72-203`）|
| 面板 HTTP API | HTTP | 内核提供 / 壳与浏览器消费 | **71 精确 + 14 前缀**（`api/surface.js`）；信任三层（socket 身份 → Origin 端口 → 可选 access key）|
| 壳健康上报 | HTTP | 壳 → 内核 | `POST /shell/health`（`phase=ready` 即更新确认信号）|
| 守卫握手 | HTTP | 壳 → 内核 | `GET /healthz`、`GET /session/status`、`POST /session/stop` |

**逐项核实结果**：

| 契约 | 单写入方？ | 原子写？ | schema？ |
|---|---|---|---|
| `config.json` | ✅ 内核 | ✅（`supervisor.js:735-750`）| ❌ |
| `runtime.json` | ✅ 壳 | ❌ 直接 `fs::write`（`node.rs:387`）| ❌ |
| `registry.json` | ⚠️ **壳写，2 个调用点**（`node.rs:178` 于 `latest_lts()` 内 / `main.rs:1079` 于手动改镜像）—— 但**只写 `origins`**，不写 `catalog`/`probe` | ✅（`mirror.rs:213` tmp+rename）| ❌ |
| `identity.json` | ✅ 壳 | ✅（`update.rs:write_json` tmp+rename）| ❌ |
| `update-journal.json` | ✅ 内核 | ✅（`domains/shell/index.js:34-40`）| ❌ |

→ **契约面的三个结构缺口**：① `registry.json` **内容不完整**（只有 `origins`，无 `catalog` 与 `probe` 规格 → 两侧仍会选到不同的源；且 `latest_lts()` 失败时不导出）；② 全部无 schema 版本；③ `runtime.json` 非原子写。

---

# 第三部分　桌面壳架构（实测）

## 7. 逻辑架构（现状）

```
壳 = 引导状态机  +  窗口/托盘  +  与内核的 HTTP/文件对话

引导状态机（bootstrap.html 单块 760 行 JS）：
  stepEnv()          → 轮询 node_status（900ms 预算，前端 15s withTimeout）
  afterEnv()         → Node 缺/过低 → probeMirrorThen → start_node_install
  stepShellUpdate()  → shell_update_check / apply（Tauri updater + minisign）
  stepCorePlan()     → core_plan → core_apply
  stepGuardReady()   → guard_start → guard_ready 轮询
  stepPanel()        → finish_boot → window.location.replace(shell.html)

窗口/托盘（main.rs + shell.html）：
  decorations:false → 自绘标题栏（min/max/hide + 显式拖动）
  CloseRequested → closeAction==exit ? shutdown_all()+exit(0) : hide()+prevent_close()
  托盘左键 → show_main；右键 → 菜单（显示/启动/停止/重启/退出）
```

**IPC 形态（全部经 `main.rs` 的 20 个 `#[tauri::command]`）**：

| 命令 | 行 | 体量 | 职责 |
|---|---|---|---|
| `node_status` | 73 | 47 | 环境探测（异步，900ms 预算）|
| `mirror_warmup` / `mirror_cached` | 125/132 | 5/17 | 镜像预热（后台）+ 读缓存（无 I/O）|
| `node_latest` | 151 | 17 | 官方最新 LTS（网络）|
| `start_node_install` | 186 | 39 | 装 Node |
| `core_status`/`core_plan`/`core_apply` | 369/394/407 | 22/8/38 | 内核版本治理 |
| `guard_start`/`guard_ready` | 447/484 | 30/14 | 守卫启停与握手 |
| `win_ctl` | 502 | 26 | 窗口动作 |
| `shell_identity`/`shell_set_phase`/`shell_panel_url` | 960/971/820 | 9/4/8 | 壳身份与阶段 |
| `mirror_status`/`mirror_set` | 1020/1054 | 31/30 | 镜像设置 |
| `shell_update_check`/`shell_update_apply` | 1088/1145 | 54/77 | 壳自更新 |
| `shell_restart`/`finish_boot` | 1224/172 | 6/5 | 重启/完成 |

**无头自检入口（4 个，任何平台可跑）**：`--env-plan` / `--mirror-plan` / `--node-plan` / `--core-plan` / `--service-plan`。

## 8. 工程现状：脆弱性量化

| 指标 | 实测 | 对照（内核）| 判断 |
|---|---|---|---|
| 平台分支 | **43 处 / 8 文件** | 67 处 / 60 在 `platform/os/`（90%）| ❌ **壳零平台层** |
| 分支明细 | `node.rs` 11 / `main.rs` 10 / `service.rs` 9 / `env.rs` 6 / `bounded` 2 / `nodeprobe` 2 / `update` 2 / `core` 1 | — | ❌ 加平台翻 8 文件 |
| `main.rs` | **1487 行**（20 命令 487 行 + 4 CLI + 平台服务控制 + 业务）| `supervisor.js` 1188 行但已拆 6 mixin | ❌ 单体，无分层 |
| 错误类型 | `Result<_, String>` **42 处** / Error 枚举 **0** | 内核 `{ok:false,error,code?}` 对象 | ❌ 前端只能字符串匹配 |
| 前端 | `bootstrap.html` **760 行单块 JS** | 内核 `ui-react/` 有构建与模块化 | ❌ 一处语法错全页死（已真实发生）|
| **有界执行** | `bounded.rs` + **B32 门禁**（禁裸 `.output()`）| `platform/exec.js` **零引用** + 23 处无 timeout | ✅ **壳优于内核**（罕见）|
| 文档 | 10 份，`Contract`/`不变量`/`平台矩阵` 出现 **0** 次 | `ARCHITECTURE-CONTRACT-phase0.md` 等 | ❌ 无规范 |
| 测试 | `cargo test` **68 项**（B1–B55 + V1–V6 + 单元）| 47 文件 / 991 断言 | ✅ 基础健康 |

**结论：壳不是「一塌糊涂」，是「缺一层抽象（platform/）＋ 缺一层分层（commands/domain）＋ 缺规范文档＋ 缺把规范变门禁」。**

## 9. 壳的真实缺陷（本次审计确证）

| # | 缺陷 | 证据 | 后果 |
|---|---|---|---|
| **S1** | 分层违规：`service.rs` 管**定义**、`main.rs:543-614` 管**启停** | 同一概念分居两层，各 4 份 `#[cfg]` | 加平台要改两处不同层 |
| **S2** | `main.rs` 混装 IPC + 平台 + 业务 | 1487 行 / 20 命令 | 无法单测、无法替换 |
| **S3** | 错误全靠字符串 | 42 处 `Result<_, String>` | 前端按文案猜、无法程序化分支 |
| **S4** | 前端单块 760 行 | `bootstrap.html` 实测 1 个内联 script | 一处语法错全页不执行（2026-09-11 已发生一次）|
| **S5** | 平台分支无归属 | 43 处散落 8 文件 | 加平台/查平台 bug 成本高 |

---

# 第四部分　抽取审计（内核 → 壳）

## 10. 判据（由 F1–F5 推出，非偏好）

```
能力 X 的归属判定：
  X 在「装内核之前/期间」需要？          → 壳（F1）
  X 在「无 GUI/无壳」时仍需工作？         → 核（F2/F3）
  两侧都需要，但只是「同一份事实/规格」？ → 壳拥有定义，内核消费产物（F5-②）
  两侧都需要，且是「同一套行为语义」？    → 统一规格 + 测试向量（F5-③）
  只有一侧需要？                         → 留在那侧，不做无意义搬迁
```

## 11. 逐项判定

### 11.1 内核**运行期专有** → 留内核（约 18900 / 19354 行，97.6%）

| 分域 | 行数 | 归属依据 |
|---|---|---|
| `guard/`（21 文件）| 5042 | F2/F3：守卫在无壳时运行 |
| `domains/router`（14 文件）| 4774 | F2：纯运行期（路由/配额/供应商）|
| `domains/relay`（4 文件）| 1444 | F2：反代/公网暴露 |
| `domains/plugin`（2 文件）| 1281 | F2+F4：面板与运行期都要 |
| `domains/instance`（1 文件）| 987 | F2+F4：实例生命周期 |
| `domains/shell`（2 文件）| 507 | **F3**：壳崩溃时壳不存在，只有抗重启的守卫能救它 |
| `domains/dist`（2 文件）| 548 | F2：内核自升级/装插件要走 npm |
| `api/`（13 文件）| 1495 | F4：面板由内核托管 |
| `platform/` 基础（非 os）| 约 1400 | F2：日志/事件/token/tasks/config |
| `supervisor.js` + `bin/` | 1839 | F2：编排与宿主 |

### 11.2 真正的重叠 → 需统一

| # | 重叠 | 壳侧 | 内核侧 | 判定 |
|---|---|---|---|---|
| **O1** | **npm 镜像目录** | `mirror.rs` NPM_PRESETS **6** | `dist/index.js` REGISTRY_PRESETS **6** + `config.js` registries **6** | 🔴 **逐字节相同的 3 份** → **壳拥有，内核消费产物** |
| **O2** | **镜像探测方法** | 真实包元数据 `@dsh-sup/dsh-core-<plat>` | `/-/ping` | 🔴 **方法不同 → 结论不同**（实测 ustclug 2613ms vs 389ms；内核选 huaweicloud、壳选 npmmirror）|
| **O3** | **npm 安装执行** | `core.rs`（含三平台提权）| `dist/index.js runNpmInstall`（不提权）| 🟡 **执行各留**（提权需人在场，内核做不到）；**规格共享** |
| **O4** | **环境探测** | `nodeprobe.rs` 639 + `env.rs` 272（含**最低门槛 v22.12**）| `env-catalog.js` 83（只 `which --version`）| 🟡 **规格必须统一**：内核现会谎报「环境就绪」 |
| **O5** | **平台原语** | `bounded.rs` 191（有界执行）| `platform/exec.js` 48（**零引用**）| 🟡 **规格 + 测试向量**；且内核有 23 处无 timeout 的 `execFileSync` |
| **O6** | **版本校验/比较** | `core.rs is_valid_version` / `semver_cmp` | `dist VERSION_RE` / `semverCompare` | 🟡 **测试向量**（实测 3 处分歧：`1.0.0+`、`1.0.0+!!!`、`1.0.0+あ`）|
| **O7** | **自启/服务定义** | `service.rs` 258（三平台定义）| `autostart.js` 354（开关 + GUI 自启）| ✅ **已定案**（所有权矩阵）|

### 11.3 **不做**的事（诚实说明）

| 不做 | 理由 |
|---|---|
| 把 DSH/插件安装移到壳 | F2：内核在壳关闭时必须能自升级/装插件 |
| 把 `platform/os/service.js` 移到壳 | 它管 **DSH 实例**的 systemd transient 单元，与守卫服务是**不同对象** |
| 合并日志 | 内核 `EventHub` 是有**不变量**的子系统（单写者、seq 全局单调、跨重启续号）；合并会破坏它。应做**统一格式 + 统一读取视图** |
| 把壳看护移到壳 | 壳不能自监督（F3）；已由守卫承载 |
| 追求「代码量减少」| 待统一的仅**内核侧约 420 行副本**（占 2%）。真实收益是**「同一问题只有一个答案」** |

## 12. 内核的真实缺陷（本次审计确证，与壳重叠无关）

| # | 缺陷 | 证据 | 后果 |
|---|---|---|---|
| **K1** | **daemon 脚本路径全断**（§7.6 拆分后未更新）| `control-view.js:216-220` `path.join(__dirname,'..')` + `'src/domains/.../daemon.js'` → 实际解析为 `src/guard/src/domains/...`（**node 实测 MISS**）；`registry-view.js:185/195` 同一错误 | `_daemonLifecycle` 恒 `null` → **守卫永远无法自起 router/lan daemon**；`lanDaemon=true` 直接返回「脚本缺失」。**发行态更甚**：`build-launcher.sh` 只发 `bin/core.cjs/ui-react`，连修对路径也无 `src/`。**测试绕过**（`daemon-lifecycle-test.js:24` 直接 new、`lan-daemon-test.js:93` 自行 spawn）|
| **K2** | **`platform/exec.js` 是死代码** | 全仓 **0 处 require**；而 23 处 `execFileSync` **无 timeout**（`autostart.js` 11 / `native/manager.js` 4 / `settings-view.js` 4 / `service.js` 2 / `fs-utils.js` 1 / `token.js` 1）| 该文件自称「同步 exec 的**唯一入口**」，实际无人使用 → systemctl/schtasks/npm 无响应时**无界挂起**。与 macOS 假注释**同一失效模式** |
| **K3** | 双生命周期状态源 | `lifecycle/managed.js:19` 与 `lifecycle/objects.js:23` 各存 desired/phase，靠 3 个手工镜像函数对账 | 易漂移；phase 词表实际有 4 套 |
| **K4** | `ManagedLifecycle.start` 忽略回调 `ok:false` | `managed.js:113-118` 不看 `r.ok` | `/lifecycle/status` 谎报成功 |
| **K5** | 影子决策未建模 `_crashHalted` | `converge-view.js:29-35/67-72` | G3 切换门槛永久不可达，日志持续刷 diff |
| **K6** | `originAllowed` 只比端口不校验 host | `api/index.js:118-128`；`identity.js:7-8` 声称有 Host 校验但**实现不存在** | DNS-rebinding CSRF 可驱动全部写接口 |
| **K7** | `ports.js:46` 用 `process.env.HOME \|\| '/tmp'` | 同上 | Windows 无 HOME → 端口注册表写 `/tmp`（错误位置）|
| **K8** | 平台命令泄漏到业务层 | `settings-view.js:100`（systemd 专属）、`:323/327`（`ip` 命令）、`native/manager.js:165-170/646-651`（裸 `node`/`npm`）| macOS/Windows 上局域网面板 URL 为空、npm 调用失败 |
| **K9** | 版本解析正则错误 | `settings-view.js:228` `/dsh-supervisor v([^s]+)/` —— `[^s]` 应为 `[^\s]` | 自更新 verified 判定被污染 |
| **K10** | 卸载失败仍删 manifest | `native/manager.js:657-665` | 残留无法清理 |

---

# 第五部分　目标架构（一次性目标态）

## 13. 壳的逻辑架构

```
┌─ 引导状态机（显式，可测）────────────────────────────────────────────┐
│  Step 枚举：Env → Node → ShellUpdate → Kernel → Guard → Panel        │
│  每步契约：{ id, budgetMs, run(ctx) → StepResult, onFail(action) }   │
│  StepResult = { ok } | { ok:false, reason, retryable, hint }         │
│  引擎负责：超时、重试、进度上报、错误归类 —— 步骤只关心自己           │
└──────────────────────────────────────────────────────────────────────┘
                                    │
┌─ 窗口/托盘 ───────────────────────┼──────────────────────────────────┐
│  自绘标题栏 · 关闭语义（hide/exit）· 托盘菜单 · 面板导航             │
└──────────────────────────────────────────────────────────────────────┘
                                    │
┌─ 与内核对话 ──────────────────────┼──────────────────────────────────┐
│  HTTP：/healthz /session/* /shell/* /env/* /dist/*（经 contract/）   │
│  文件：写 registry.json / identity.json / runtime.json（经 contract/）│
└──────────────────────────────────────────────────────────────────────┘
```

## 14. 壳的工程架构（目标目录结构）

```
src-tauri/src/
├── main.rs                    # 仅组装 + 入口（目标 ≤ 150 行）
│
├── commands/                  # ① IPC 边界层（20 个 #[tauri::command]）
│   ├── mod.rs  env.rs  node.rs  core.rs  mirror.rs  shell.rs  window.rs
│   └── 【禁】业务逻辑 · 平台判断 · 直接 Command 调用
│
├── domain/                    # ② 业务层（平台无关）
│   ├── boot/                  #    Step 枚举 + 引导引擎（取代 760 行前端状态机）
│   ├── probe/                 #    nodeprobe 拆分：候选枚举 / 版本 / PATH
│   ├── provision/             #    Node 安装 + 内核安装（供给）
│   ├── mirror/                #    镜像目录 + 选择 + **契约投放**
│   ├── update/                #    壳自更新 + 护栏账本
│   └── contract/              #    与内核的契约（schema 校验 + 原子读写）
│
├── platform/                  # ③ 平台适配层 —— **全仓唯一平台分支所在地**
│   ├── mod.rs                 #    trait Platform / trait ServiceControl
│   ├── linux.rs  macos.rs  windows.rs  unsupported.rs
│   └── 【目标】43 处分支从 8 文件收拢到此
│
├── infra/                     # ④ 原语
│   ├── bounded.rs             #    有界执行（既有，保留 B32 门禁）
│   ├── fs.rs  net.rs  proc.rs
│
└── error.rs                   # 结构化 ShellError
```

**依赖方向（硬约束，单向）**：

```
commands ──▶ domain ──▶ platform ──▶ infra
                │                    ▲
                └────────────────────┘
（infra 不得依赖 domain；platform 不得依赖 commands）
```

## 15. 平台适配层（把 43 处收拢成 1 个契约）

```rust
// platform/mod.rs
/// 平台能力契约。**每个能力要么实现，要么显式 Unsupported**（不得静默成功）。
pub trait Platform: Send + Sync {
    fn name(&self) -> &'static str;

    // ── Node 制品与安装（F1：这是壳的独有职责）──
    fn node_artifact(&self, version: &str) -> Option<NodeArtifact>;  // tar.xz / pkg / msi
    fn install_node(&self, a: &Path) -> Result<InstallReport, ShellError>;  // 含提权
    fn core_platform_tag(&self) -> &'static str;   // linux-x64 / darwin-arm64 / win-x64

    // ── 探测 ──
    fn path_dirs(&self) -> Vec<PathBuf>;
    fn known_node_locations(&self) -> Vec<PathBuf>;
    fn is_local_fixed_dir(&self, d: &Path) -> bool;   // Windows 排除网络盘/可移动盘

    // ── 服务（**定义 + 启停同一对象** —— 修掉 S1 分层违规）──
    fn service(&self) -> &dyn ServiceControl;

    // ── 能力声明（供契约与诊断）──
    fn capabilities(&self) -> PlatformCapabilities;
}

pub trait ServiceControl: Send + Sync {
    fn definition_path(&self) -> PathBuf;
    fn ensure_defined(&self, guard: &Path) -> Result<String, ShellError>;
    fn start(&self) -> Result<(), ShellError>;
    fn stop(&self) -> Result<(), ShellError>;
    fn spawn_daemon(&self, guard: &Path) -> Result<u32, ShellError>;
}
```

## 16. 结构化错误（替代 42 处 `Result<_, String>`）

```rust
// error.rs
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ShellError {
    Probe   { stage: String, cause: String, elapsed_ms: u128 },  // 必须带阶段与耗时
    Network { url: String, cause: String },
    Install { platform: String, cause: String },
    Service { action: String, cause: String },
    Contract{ file: String, cause: String },   // 与内核的边界
    Ipc     { cause: String },
    Unsupported { capability: String, platform: String },  // 替代静默成功
}
```

前端按 `kind` 给**不同的可操作建议**，而不是显示一句可能判错的话。

## 17. 跨平台规范（矩阵即断言）

| 能力 | Linux | macOS | Windows | 实现位 |
|---|---|---|---|---|
| 环境探针（候选/版本/PATH）| ✅ | ✅ | ✅ | `domain/probe` |
| Node 制品解析 | ✅ `tar.xz` | ✅ `pkg` | ✅ `msi` | `platform/*::node_artifact` |
| Node 安装（提权）| ✅ `pkexec` | ✅ `osascript` | ✅ `msiexec` | `platform/*::install_node` |
| 镜像测速与选择 | ✅ | ✅ | ✅ | `domain/mirror` |
| 内核安装/升级 | ✅ | ✅ | ✅ | `domain/provision` |
| 服务定义（守卫）| ✅ systemd | ✅ LaunchAgent | ✅ schtasks | `platform/*::ServiceControl` |
| 服务启停 | ✅ `systemctl --user` | ✅ `launchctl` | ✅ `schtasks` | 同上 |
| 提权通道探测 | ✅ | ✅（恒有）| ✅ | `platform/*::has_privilege_channel` |
| 壳自更新 | ✅ deb/rpm | ✅ app | ✅ exe/msi | `domain/update` |
| 壳崩溃自愈 | ✅ 守卫看护 | ✅ 守卫看护 | ✅ 守卫看护 | 内核 `domains/shell/watchdog` |
| 壳开机自启 | ✅ XDG | ✅ LaunchAgent gui | ✅ schtasks GUI | `platform/os/autostart`（内核）|

**不变量 P1**：每格必须是「实现」或「显式 Unsupported」。

## 18. 契约层（与内核的唯一耦合面）

```
domain/contract/
  ├── schema.rs     契约版本常量 + 校验（每个契约带 schema）
  ├── mirror.rs     写 ~/.dsh/supervisor/registry.json（**壳是唯一写入方**）
  ├── identity.rs   写 ~/.dsh/shell/identity.json
  └── runtime.rs    写 ~/.dsh/supervisor/runtime.json（改为原子写）
```

### 18.1 `registry.json` 升级（消费者内核已就位）

```jsonc
{
  "schema": 2,                          // 新增：契约版本
  "writtenBy": "shell@1.0.9",           // 新增：谁写的
  "writtenAt": 1789147076,
  "mode": "auto",                       // 既有
  "manualOrigin": "https://registry.npmmirror.com",
  "catalog": [ "…" ],                   // 新增：全集（此前只有被选中的）
  "selected": { "origin": "…", "latencyMs": 57, "checkedAt": 1789147070 },
  "probe": {                            // 新增：探测规格 → 两侧同一答案（修 O2）
    "kind": "package-metadata",
    "pathTemplate": "@dsh-sup%2Fdsh-core-{platform}",
    "timeoutMs": 6000
  }
}
```

### 18.2 投放时机（补齐内容 + 补齐时机）

```
壳启动              → 导出（契约缺失或 schema 过期时）
用户改镜像设置       → 导出（既有）
壳版本升级后         → 导出（含 schema 迁移）
内核读取             → 优先契约；缺失/损坏/schema 不匹配 → 最小兜底 + 写事件
内核复测（过期）     → **按契约的 probe 规格**执行 → 与壳同法
```

### 18.3 契约不变量

| # | 不变量 |
|---|---|
| **C1** | 每个契约文件**只有一个写入方**（双写必然漂移 —— macOS plist 已发生过）|
| **C2** | 内核在契约缺失时**必须能降级运行** |
| **C3** | 契约带 `schema`，不匹配时**明确拒绝**并记录 |
| **C4** | 写入**原子**（tmp+rename），读取**容忍缺失** |
| **C5** | 壳**启动必导出** |
| **C6** | 两侧对同一问题的判定**必须给同一答案** |

## 19. 前端架构（消除单点死亡）

```
bootstrap/
  ├── bootstrap.html          # 骨架 + 各模块 <script>
  ├── shell.html              # 壳框架
  └── js/
      ├── 00-runtime.js       #   IPC 获取 + withTimeout + **全局 onerror**
      ├── 10-ui.js            #   状态/步骤/失败页/诊断串
      ├── 20-env.js  30-node.js  40-mirror.js
      └── 50-shell-update.js  60-kernel.js  70-guard.js
```

| # | 不变量 |
|---|---|
| **F1** | 每个 JS 文件**独立语法检查**（每文件一个断言）|
| **F2** | 全局 `window.onerror` + `unhandledrejection` → 上报落 `shell.log` |
| **F3** | 启动自检：IPC 不可用时**明确报错**，不得静默停住 |
| **F4** | 需要 IPC 的页面必须是**主帧**（既有 B54）|

---

# 第六部分　一次性执行方案

> **不分阶段交付**：以下 5 批工作在**一次改造**中完成，可在同一 PR/同一版本内交付。
> 批内有序（有依赖），批间无发布节点。

## 20. 执行批次

### 批 A　壳的平台层（修 S5 + S1）

| # | 动作 | 验收 |
|---|---|---|
| A1 | 建 `platform/{mod,linux,macos,windows,unsupported}.rs`，定义 `Platform` + `ServiceControl` | 编译通过 |
| A2 | 把 **43 处平台分支**搬入 `platform/`（**搬位置不改逻辑**）| 门禁 **G1** 绿 |
| A3 | 把 `service.rs` 的 `ensure_defined` 与 `main.rs` 的 `start/stop_guard_service` **合并进 `ServiceControl`** | 门禁 **G2** 绿 |
| A4 | 平台能力矩阵自检 `--platform-matrix` | 三平台输出与 §17 一致 |

### 批 B　壳的分层（修 S2）

| # | 动作 | 验收 |
|---|---|---|
| B1 | `main.rs` 拆出 `commands/`（20 个命令按域分文件）| `main.rs` ≤ 150 行 |
| B2 | 业务抽到 `domain/`（probe/provision/mirror/update）| 门禁 **G3**（命令体 ≤ 40 行）|
| B3 | `bounded.rs` → `infra/bounded.rs`；新增 `infra/{fs,net,proc}.rs` | B32 门禁保持绿 |

### 批 C　错误模型（修 S3）

| # | 动作 | 验收 |
|---|---|---|
| C1 | 引入 `error.rs::ShellError` | 42 处 `Result<_, String>` 归零 |
| C2 | IPC 边界改为结构化错误；前端按 `kind` 出建议 | 诊断页显示 kind 而非裸字符串 |

### 批 D　契约层（修 O1/O2/O4/O6 + C1–C6）

| # | 动作 | 验收 |
|---|---|---|
| D1 | 建 `domain/contract/`，`registry.json` 升 schema=2（含 catalog/selected/probe）| 内核实测能读 |
| D2 | **壳启动即导出，且携带 `catalog` + `probe`**（现仅在 `latest_lts()` 成功时导出，且内容不全）| 全新 HOME 首启后文件存在且含三字段 |
| D3 | 内核 `dist` 的 6 源副本 → **最小兜底**（2 源）；`config.js` 同理 | 内核侧副本 < 10 行 |
| D4 | 内核按契约 `probe` 规格复测（修 O2 选源不一致）| 两侧选源一致（同机同刻）|
| D5 | 内核 `env-catalog` 采用 **v22.12 门槛**（修 O4）| 面板不再谎报「环境就绪」|
| D6 | `shared/version-vectors.json` + 两侧测试（修 O6）| 3 处历史分歧被覆盖 |

### 批 E　前端 + 门禁（修 S4 + 建立规范强制）

| # | 动作 | 验收 |
|---|---|---|
| E1 | `bootstrap.html` 760 行 → 8 个 JS 模块 | 门禁 **G5**（每文件独立语法）|
| E2 | 全局 `onerror`/`unhandledrejection` → `shell.log` | 人为抛错能在日志看到 |
| E3 | 落 **G1–G8** 全部门禁 | `cargo test` 含 8 组新断言 |

### 批 F　内核侧缺陷（修 K1–K10）

| # | 动作 | 验收 |
|---|---|---|
| F1 | **K1**：修 daemon 脚本路径（`control-view.js:216` / `registry-view.js:185/195`）| 门禁：路径必须 `existsSync` 为真 |
| F2 | **K1 发行态**：`build-launcher.sh` 打包 `src/` 或让 daemon 内联/改用同进程 | 发行包内路径可达 |
| F3 | **K2**：`platform/exec.js` 接入全部 `execFileSync`（或删除并统一到 `bounded` 等价物）+ 加「禁裸 exec」门禁 | 无 timeout 的 `execFileSync` 归零 |
| F4 | K6：`originAllowed` 增加 host 校验（补上 `identity.js` 声称的 Host 闸）| 新增 rebinding 测试 |
| F5 | K7/K8/K9/K10：`ports.js` HOME 兜底 / 平台命令下沉 / 正则修正 / manifest 保留 | 逐条断言 |
| F6 | K3/K4/K5：生命周期双源收敛、`start` 尊重 `ok:false`、影子建模 `crashHalted` | ✅ 全部完成 |

## 21. 门禁清单（规范的可执行化）

| # | 门禁 | 类型 |
|---|---|---|
| **G1** | 平台分支只在 `platform/` 内 | ✅ 会失败 |
| **G2** | `commands/` 内零 `Command`、零 `#[cfg]` | ✅ |
| **G3** | `main.rs` ≤ 150 行；命令体 ≤ 40 行 | ✅ |
| **G4** | 每能力 × 每平台 = 实现 或 `Unsupported` | ✅ |
| **G5** | 每个前端 JS 独立语法正确 | ✅ |
| **G6** | 契约带 `schema`；壳启动必导出 | ✅ |
| **G7** | 阻塞调用经 `infra::bounded` | ✅（B32 扩展）|
| **G8** | 注释引用的**仓内路径必须存在** | ✅（新增；已发现 30+ 悬空）|
| **G9** | 内核：无 timeout 的 `execFileSync` 归零 | ✅（新增）|
| **G10** | 内核：脚本路径引用必须 `existsSync` | ✅（新增；直接防 K1 复发）|

## 22. 验收标准（何时算「标准壳工程」）

```
1. cargo test 全绿，且含 G1–G8；内核 npm test 全绿，且含 G9–G10
2. 三平台 CI 各跑 --platform-matrix，输出与 §17 一致
3. main.rs ≤ 150 行；commands/ 内零 #[cfg]、零 Command
4. 前端 8 个 JS 模块各自语法门禁；删任一模块不影响其余模块加载与报错
5. 契约 schema=2 + 壳启动导出；内核实测读到并按其 probe 规格复测，两侧选源一致
6. §17 矩阵每格可指出对应测试名
7. §18.3 六条契约不变量、§19 四条前端不变量，各有断言
```

## 23. 风险与回滚

| 风险 | 缓解 |
|---|---|
| 43 处平台分支搬移引入回归 | 纯搬位置不改逻辑；既有 68 项测试 + 新增 G1/G2 双保险 |
| `main.rs` 拆分引入运行期 `ReferenceError` | 命令按域分文件、每文件独立编译单元；`cargo check` 即暴露 |
| 前端拆 8 模块引入加载顺序问题 | 模块间只经 `00-runtime.js` 暴露的全局 API；E2E（Xvfb 真跑）验证 |
| 契约 schema=2 与旧内核不兼容 | 内核侧**先就位**消费逻辑（向后兼容读旧格式），再切壳的写入 |
| 内核 F1/F2 路径修复影响 daemon 启动 | 修复后 `--service-plan` 与真实 daemon 启动双验证 |

**回滚粒度**：批 A–F 各自独立提交，单批可 revert；契约批（D）因两仓同时改，需两仓同版本回滚。

---

## 24. 本方案与前序文档的关系

| 文档 | 关系 |
|---|---|
| `DESIGN-BOUNDARY.md` | 本文件的**抽取审计**部分（判据与逐项判定的展开版）|
| `DESIGN-SHELL-ARCHITECTURE.md` | 本文件的**壳目标架构**部分（分层/平台层/错误/契约/门禁的展开版）|
| 内核 `PLATFORM-CAPABILITY-MATRIX.md` | §17 矩阵的内核侧权威版（14 项 × 3 平台）|
| 内核 `AUDIT-CROSS-PLATFORM.md` | 跨平台审计历史（§五.a 含 2026-09-11 复核更正）|
| 本文件 `DESIGN-COMPLETE.md` | **总纲**：系统全貌 + 双侧审计 + 抽取决策 + 目标架构 + 一次性执行方案 |

## 25. 诚实说明

1. **内核约 97.6% 不需要动**。真实重叠仅约 420 行副本 —— 本方案的价值在**消除不一致**与**建立规范**，不在减少行数。
2. **壳在「有界执行」这一维度优于内核**（有 B32 门禁；内核的 `exec.js` 是死代码）。方案中 F3 是**内核向壳学**，不是反向。
3. **K1（daemon 路径全断）是本次审计最严重的发现**，且它**不在**任何既有测试覆盖内 —— 说明「测试绿」不等于「功能通」。
4. 本次审计中，**子代理报告与我亲自复核的结论一致**（K1 已用 node 实测确认路径 MISS）。
5. **未确认项**：壳在 macOS/Windows 上的真实运行行为（本机为 Linux，`--platform-matrix` 需三平台 CI 才能验证）；发行态 `build-launcher.sh` 之外的打包路径（`release-core.sh`）是否另有处理。

---

# 第七部分　迁移清单（逐项：从内核到壳）

> 这是**可执行清单**：每项给出「内核侧位置与当前形态 → 壳侧目标位置与形态 → 内核侧改成什么 → 验收断言」。
> 已逐行核对行号（2026-09-11）。

## 26. 总览：真正要搬的东西只有 6 项

| # | 项 | 方向 | 内核侧行数 | 壳侧目标 | 内核侧最终形态 |
|---|---|---|---|---|---|
| **M1** | npm 镜像目录（6 URL × **3 副本**）| 壳拥有 → 内核消费 | **约 20 行**（3 处）| `mirror.rs` 已有，补「导出全集」| 删除副本，留 **2 条最小兜底** |
| **M2** | 镜像**探测方法**（选源不一致的根因）| 壳定义 → 内核照做 | `_probeRegistry` **7 行** | `mirror.rs:237-247` 已有 | 按契约 spec 探测 |
| **M3** | 镜像**选择结果**（避免两侧各测一遍）| 壳投放 → 内核优先用 | `selectRegistry` 27 行（保留）| `warmup_async` 已有 | 优先读契约，过期才复测 |
| **M4** | Node **最低门槛**（内核谎报就绪的根因）| 壳定义 → 内核采用 | `env-catalog.js` 判据 | `node.rs MIN_NODE` 已是权威 | 增加 requiredVersion 判定 |
| **M5** | 版本**校验/比较语义**（实测 3 处分歧）| 规格共享（测试向量）| `VERSION_RE` + `semverCompare` | `core.rs` 已有 | 两侧跑同一向量 |
| **M6** | **有界执行纪律**（反方向：内核向壳学）| 内核侧修 | `platform/exec.js` **零引用** | `bounded.rs` + **B32 门禁**（已达标）| 接入或删除 + 加门禁 |

**其余全部留在内核**（约 18900 行）—— 判定依据见 §11.1。

---

## 27. M1　npm 镜像目录：删除内核的 3 份副本

### 27.1 现状（三份逐字节相同）

| 副本 | 位置 | 形态 | 行数 |
|---|---|---|---|
| ① 壳（**所有者**）| `mirror.rs` `NPM_PRESETS` | Rust 数组 | 已有 |
| ② 内核 | `dist/index.js:68-75` `REGISTRY_PRESETS` | `[{label,origin}]` × 6 | 8 |
| ③ 内核 | `platform/config.js:66-73` `registries` | `[string]` × 6 | 8 |

### 27.2 迁移动作

**壳侧（M1-a）**：`mirror.rs` 已是所有者，补「导出**全集**」（当前 `export_to_kernel(npm)` 只导出被选中的）：

```rust
// mirror.rs —— 扩展导出契约（当前只写 origins，现改为写全集 + 选择结果 + 探测规格）
pub fn export_to_kernel(m: &Mirrors, selected: Option<&Probe>) -> Result<(), String> {
    let v = serde_json::json!({
        "schema": 2,
        "writtenBy": format!("shell@{}", crate::env::shell_version()),
        "writtenAt": now_secs(),
        "mode": "auto",
        "manualOrigin": m.npm.first().cloned().unwrap_or_default(),
        "catalog": m.npm,                       // ← 新增：全集（内核读它，不再自带）
        "selected": selected.map(|p| serde_json::json!({
            "origin": p.source, "latencyMs": p.latency_ms, "checkedAt": now_secs(),
        })),
        "probe": {                              // ← 新增：探测规格（让内核给出同一答案）
            "kind": "package-metadata",
            "pathTemplate": npm_probe_path(),   // "@dsh-sup%2Fdsh-core-{platform}"
            "timeoutMs": 6000,
        },
    });
    /* 原子写（已有 tmp+rename 逻辑） */
}
```

**内核侧（M1-b）**：`dist/index.js:68-75` 的 `REGISTRY_PRESETS` **删除**，`platform/config.js:66-73` 的 `registries` **删除**，改为：

```js
// platform/config.js —— 最小兜底（契约缺失/损坏时才用；不再是「一等来源」）
//  为什么是 2 条而不是 6 条：目录归壳（M1），内核只需保证「契约缺失时也能跑」。
//  官方源 + 国内最普及源，覆盖「能上网」与「中国网络」两种基本情形。
registries: [
  "https://registry.npmjs.org",
  "https://registry.npmmirror.com",
],
```

```js
// dist/index.js —— 契约优先，兜底在后
_registryOrigins() {
  const fromContract = this._catalogFromContract();   // 读 ~/.dsh/supervisor/registry.json 的 catalog
  if (fromContract.length) return fromContract;
  const o = (this.registryConfig && this.registryConfig.origins) || [];
  const list = o.filter((x) => typeof x === "string" && x.trim());
  return list.length ? list : [...this.defaultRegistries];
}
```

### 27.3 验收断言

```js
// 内核 test/platform-capability-audit-test.js 新增
// A9-a：内核侧不得再持有完整 6 条镜像目录
check("A9-a 内核无 6 条镜像硬编码", !/repo\.huaweicloud\.com\/repository\/npm/.test(configSrc) || fallbackOnly(configSrc));
// A9-b：契约缺失时仍能运行（最小兜底存在）
check("A9-b 最小兜底存在", /registry\.npmjs\.org/.test(configSrc));
// A9-c：镜像源集合两侧一致（跨仓读壳的 mirror.rs）
check("A9-c 两侧集合一致", sameSet(kernelCatalog(), shellNpmPresets()));
```

---

## 28. M2　镜像探测方法：修「两侧选源不同」

### 28.1 现状（实测差异）

| 侧 | 方法 | 代码 |
|---|---|---|
| 内核 | `GET <origin>/-/ping` | `dist/index.js:127-133`（7 行）|
| 壳 | `GET <origin>/<真实包名>` | `mirror.rs:237-247` `npm_probe_path` |

**实测后果**（同机同刻、同一组 6 源）：

| 镜像 | 内核 `/-/ping` | 壳 真实包 | 差 |
|---|---|---|---|
| `npmreg.proxy.ustclug.org` | 2613 ms | 389 ms | **6.7×** |
| **选中** | `repo.huaweicloud.com` | `registry.npmmirror.com` | **不同** |

→ 面板显示一个源、壳实际用另一个、内核下载又用一个。**这是「镜像设置不可信」的直接原因。**

### 28.2 迁移动作

**壳侧（M2-a）**：把探测规格写进契约（见 §27.2 的 `probe` 字段）。

**内核侧（M2-b）**：`_probeRegistry` 改为**按契约 spec 探测**，无契约时退回 `/-/ping`：

```js
// dist/index.js —— 按契约的 probe 规格探测（与壳同法 → 同一答案）
async _probeRegistry(origin) {
  const spec = this._probeSpec();          // 来自契约；无契约时 { kind: "ping" }
  const base = origin.replace(/\/+$/, "");
  const timeout = spec.timeoutMs || 4000;
  let url;
  if (spec.kind === "package-metadata" && spec.pathTemplate) {
    // 与壳完全一致的探测目标（含 {platform} 展开）
    url = base + "/" + spec.pathTemplate.replace("{platform}", this._corePlatformTag());
  } else {
    url = base + "/-/ping";                // 兜底：契约缺失时的旧方法
  }
  const start = Date.now();
  try {
    const res = await fetch(url, { signal: AbortSignal.timeout(timeout) });
    return { ok: res.ok, latencyMs: Date.now() - start, probe: spec.kind || "ping" };
  } catch { return { ok: false, latencyMs: Date.now() - start, probe: spec.kind || "ping" }; }
}
```

### 28.3 验收断言

```js
// A10：契约存在时，两侧探测 URL 必须一致
const spec = contract.probe;
check("A10 内核按契约 spec 探测", kernelProbeUrl(spec) === shellProbeUrl(spec));
```

---

## 29. M3　镜像选择结果：避免两侧各测一遍

### 29.1 迁移动作

**壳侧（M3-a）**：`warmup_async` 已产出选择结果 → 写入契约 `selected`（见 §27.2）。

**内核侧（M3-b）**：`selectRegistry`（`dist/index.js:143-169`，27 行）**保留**（F2：内核无壳时也要选源），但**优先用契约的 selected**：

```js
async selectRegistry(force) {
  const rc = this.registryConfig || {};
  if (rc.mode === "manual" && rc.manualOrigin) { /* 既有：手动固定优先 */ }
  // ★ 新增：优先使用壳投放的选择结果（未过期 = TTL 内）
  const fromShell = this._selectedFromContract();
  if (!force && fromShell && (Date.now() - fromShell.checkedAt) < TTL) {
    this.selectedRegistry = { origin: fromShell.origin, latencyMs: fromShell.latencyMs,
                              checkedAt: fromShell.checkedAt, manual: false, source: "shell" };
    return fromShell.origin;
  }
  /* 既有：自己复测（现在用契约的 probe spec —— 见 M2）*/
}
```

**收益**：正常运行时不重复测速；契约过期时复测，且**方法一致**（M2）→ 结论一致。

---

## 30. M4　Node 最低门槛：修「面板谎报环境就绪」

### 30.1 现状

| 侧 | 判据 | 代码 |
|---|---|---|
| 壳 | `Node >= v22.12.0`（否则拒绝启动内核）| `node.rs MIN_NODE = "v22.12.0"` |
| 内核 | `which node` 成功即「ok」 | `env-catalog.js:36` `probe: () => cachedWhichVersion("node")` |

**后果**：装了 Node v18 时，面板显示「环境就绪 ✅」，而壳因门槛不满足**拒绝启动** → 用户看到「面板说没问题，但就是起不来」。

### 30.2 迁移动作

**壳侧（M4-a）**：把门槛写进 `runtime.json`（已由壳写、内核读的契约，见 §6）：

```rust
// node.rs record_runtime_meta —— 增加 minNode 字段
let meta = serde_json::json!({
    "nodeVersion": version,
    "nodePath": node_path,
    "installedAt": now_iso(),
    "source": "official-lts",
    "minNode": MIN_NODE,          // ← 新增："v22.12.0"
});
```

**内核侧（M4-b）**：`env-catalog.js` 的 node/npm 条目增加 `requiredVersion` 判定：

```js
// env-catalog.js —— 采用壳的门槛（从 runtime.json 读 minNode，兜底 v22.12.0）
const MIN_NODE_DEFAULT = "v22.12.0";
node: {
  label: "Node.js",
  required: true,
  probe: () => {
    const v = cachedWhichVersion("node");
    if (!v) return null;
    const min = runtimeMeta().minNode || MIN_NODE_DEFAULT;   // 壳投放的门槛
    return { version: v, meets: compareNodeVersion(v, min) >= 0, min };
  },
}
// state 判定改为：meets ? "ok" : "outdated"（不再是「能 which 到就 ok」）
```

### 30.3 验收断言

```js
// A11：Node 版本低于门槛时，内核必须报 outdated（不得报 ok）
check("A11 Node 门槛与壳一致", envCatalogWith("v18.0.0").items.node.state === "outdated");
check("A11 门槛值来自壳契约", envCatalogWith("v18.0.0").items.node.detail.min === runtimeMeta().minNode);
```

---

## 31. M5　版本语义：统一测试向量（跨语言唯一可行形式）

### 31.1 现状（实测 3 处分歧）

| 输入 | 壳 `is_valid_version` | 内核 `VERSION_RE` |
|---|---|---|
| `1.0.0+build5` | true | true |
| `1.0.0+` | **true** | false |
| `1.0.0+!!!` | **true** | false |
| `1.0.0+あ` | **true** | false |

根因：壳在验证前 `split("+")` **丢弃 build 段**（`core.rs:60`），内核**严格校验** build。

### 31.2 迁移动作

**不是移动代码**（Rust/JS 无法共享），而是建立**共享测试向量**：

```json
// 两份逐字节相同（壳仓 + 内核仓各一份，门禁校验一致）
// shell-release/version-vectors.json  ←→  kernel shared/version-vectors.json
{
  "schema": 1,
  "versionValidation": [
    { "input": "1.0.0",         "valid": true  },
    { "input": "1.02.3",        "valid": false },
    { "input": "1.0.0-rc.1",    "valid": true  },
    { "input": "1.0.0+build5",  "valid": true  },
    { "input": "1.0.0+",        "valid": false },   // ← 必须统一为 false（对齐 semver）
    { "input": "1.0.0+!!!",     "valid": false },
    { "input": "1.0.0+あ",      "valid": false },
    { "input": "1.0.0-",        "valid": false },
    { "input": "1.0.0-rc..1",   "valid": false }
  ],
  "compare": [
    { "a": "1.0.0",        "b": "1.0.0-rc.1", "expected": 1  },
    { "a": "1.0.0-rc.2",   "b": "1.0.0-rc.10","expected": -1 },
    { "a": "1.0.0+aaa",    "b": "1.0.0+bbb",  "expected": 0  },
    { "a": "2.0.0",        "b": "10.0.0",     "expected": -1 }
  ]
}
```

**壳侧**：`core.rs` 的 `is_valid_version` 需修正为**严格校验 build 段**（对齐 semver），否则无法通过向量。

### 31.3 验收断言

```rust
// 壳 tests/version-vectors.rs
// 加载 version-vectors.json，逐条断言 is_valid_version / semver_cmp
```
```js
// 内核 test/platform-capability-audit-test.js 新增 A12
// 加载同一文件，逐条断言 VERSION_RE / semverCompare
// 加门禁：两份向量文件必须逐字节相同
```

---

## 32. M6　有界执行纪律（**反方向**：内核向壳学）

### 32.1 现状对比

| | 壳 | 内核 |
|---|---|---|
| 设施 | `bounded.rs`（191 行，临时文件重定向 + try_wait 轮询 + 超时 kill + `CREATE_NO_WINDOW`）| `platform/exec.js`（48 行，`execFileSync + timeout`）|
| 使用 | `bounded::run` 21 处（main 9 / service 9 / node 3）| **0 处 require**（死代码）|
| 裸调用 | **0**（B32 门禁强制）| **23 处无 timeout** |
| 门禁 | ✅ B32 | ❌ 无 |

**壳的 `bounded.rs` 更完整**（`execFileSync` 的 timeout 在管道写满时可能失效，需临时文件避免）。故此项**不是迁移到壳**，而是**内核补齐**。

### 32.2 动作（全部在内核侧）

| # | 动作 |
|---|---|
| M6-a | 内核 `platform/exec.js` → 借鉴壳的实现（临时文件重定向 + try_wait 轮询 + 超时 kill）|
| M6-b | 把 23 处无 timeout 的 `execFileSync` 全部接入（`autostart.js` 11 / `native/manager.js` 4 / `settings-view.js` 4 / `service.js` 2 / `fs-utils.js` 1 / `token.js` 1）|
| M6-c | 新增门禁 **G9**：源码中不得存在无 timeout 的 `execFileSync` |

### 32.3 验收断言

```js
// 内核 test/platform-capability-audit-test.js 新增 A13 / 独立门禁
check("A13 无 timeout 的 execFileSync 归零", countBareExec() === 0);
check("A13 platform/exec.js 被实际引用", requireCount("platform/exec") > 0 || fileRemoved());
```

---

## 33. 明确**不迁移**的清单（附理由）

| 项 | 内核位置 | 不迁移的理由 |
|---|---|---|
| DSH 安装/升级/卸载 | `guard/native/manager.js`（678 行）| **F2**：内核在壳关闭时必须能自升级/装插件 |
| 沙箱实例管理 | `domains/instance/index.js`（987 行）| **F2 + R4**：面板可脱离壳操作 |
| 服务管理器（**实例** transient 单元）| `platform/os/service.js`（106 行）| 管的是 **DSH 实例**，与壳的守卫服务是**不同对象** |
| 日志/事件 | `platform/log.js` / `logcore.js` / `loghub.js`（610 行）| `EventHub` 有**不变量**（单写者、seq 全局单调、跨重启续号）；**合并会破坏它**。应做**统一格式 + 统一读取视图** |
| 端口注册表 | `guard/lifecycle/ports.js`（453 行）| **F2**：运行期端口仲裁；被 domains 多处复用 |
| 受管对象目录 | `guard/lifecycle/objects.js`（353 行）| **F2**：应然持久是运行期事实源 |
| 智能路由 | `domains/router/`（4774 行）| **F2**：纯运行期 |
| 局域网反代 | `domains/relay/`（1444 行）| **F2**：纯运行期 |
| HTTP 网关 | `api/`（1495 行）| **F4**：面板由内核托管 |
| 守卫编排 | `supervisor.js`（1188 行）| **F2**：它就是守卫本体 |
| 壳看护 | `domains/shell/watchdog.js`（224 行）| **F3**：壳不能自监督 |
| 安装执行器（npm）| `dist/index.js` `runNpmInstall` | **F2 + R3**：提权需人在场，内核做不到；**执行各留，规格共享** |

---

## 34. 迁移后的形态对照（Before / After）

### 34.1 镜像源（M1+M2+M3）

```
BEFORE（三份副本 + 两种探测法 + 各自缓存）
  壳 mirror.rs  ──┐
                  ├─ 各自 6 条 URL（逐字节相同）
  内核 dist     ──┤     /-/ping        → 选 huaweicloud 75ms
  内核 config   ──┘     真实包          → 选 npmmirror   57ms   ← 不一致
  契约 registry.json 虽在 latest_lts()/手动改镜像时写，但**只含 origins**

AFTER（壳拥有 + 内核消费 + 同一方法）
  壳 mirror.rs（唯一所有者）
      └─ 启动/改动/升级 → 导出 registry.json{schema:2, catalog, selected, probe}
  内核 dist
      ├─ 契约有 → 用 catalog + selected（不重复测速）
      ├─ 契约过期 → 用 probe 规格复测（与壳同法 → 同答案）
      └─ 契约缺失 → 2 条最小兜底 + 写事件
```

### 34.2 环境判定（M4）

```
BEFORE                          AFTER
  壳：Node >= v22.12 才放行        壳：把 minNode 写进 runtime.json
  内核：which node 成功即 ok       内核：读 minNode，低于即报 outdated
  面板显示「环境就绪」            面板显示「Node v18 低于最低要求」
  壳却拒绝启动内核 ❌             两侧结论一致 ✅
```

### 34.3 有界执行（M6）

```
BEFORE                                    AFTER
  壳：bounded.rs + B32 门禁 ✅            壳：不变
  内核：exec.js 零引用 + 23 处裸调用 ❌    内核：接入 exec.js + G9 门禁 ✅
```

---

## 35. 本清单与执行批次的对应

| 批次（§20）| 覆盖本清单 |
|---|---|
| 批 A（壳平台层）| — |
| 批 B（壳分层）| — |
| 批 C（错误模型）| — |
| **批 D（契约层）** | **M1 / M2 / M3 / M4 / M5 全部** |
| 批 E（前端 + 门禁）| — |
| **批 F（内核缺陷）** | **M6** + K1–K10 |

**关键点**：本清单的 6 项中，**5 项集中在批 D（契约层）**，1 项在批 F。
即：**「把东西移到壳里」实际上就是「建立契约层 + 删除内核副本」这一件事**。

### 第四轮：转发链路 + 前端门禁（2026-09-12 续）

覆盖此前未审计的转发三文件（proxy 893 / forward-core 448 / relay 488 行）与
router 其余（switch/evidence/router-ops/index/quota-strategies）、guard/ports、relay/manager，
以及 `ui/src` 前端源码。修 7 P1 + 7 P2/P3。

| 级别 | 位置 | 缺陷 |
|---|---|---|
| **P1 安全** | 内核 relay | `config.js` 声称「LAN 受 RFC1918 白名单约束」，**全仓从未实现**；relay 监听 0.0.0.0 且把 Origin 改写成回环 → 未设 token 时同网段零认证触达 DSH 特权面 |
| **P1** | 内核 forward-core | 请求级熔断**完全失效**：`markUsed` 在请求**发出前**无条件清零失败计数 → 阈值 2 数学上不可达 |
| **P1** | 内核 forward-core | `prov.markNetFail` —— **方法全仓不存在**，guard 恒 false；注释声称的「2026-09 二次修正」从未生效 |
| **P1** | 内核 proxy.js | `_restartPending` 只写不读（延后=丢弃）且 2min 退避在延迟**之前**置位 |
| **P1** | 内核 proxy.js | 裸 `npx` 未走平台解析（Windows ENOENT），且门禁正则只覆盖 npm、对 npx 盲区 |
| **P1** | 内核 base.js | `applyDetection` 失败分支不设 nextResetAt → 探测闸门**恒真**，每 5min 起停实例（启停风暴）|
| **P1 门禁** | 内核 CI | 前端门禁**从未执行**：CI 注释指向的「release-core.sh 的 [3/7]」**不存在**（只有 [1/5]），而 build-ui 只构建不测试 |
| **P2** | relay / ports / router-ops | 注释谎称 HOLD_MS 上限 · `readUpstreamBody` 无时间上限 · `acc.instance` 与 instanceOf 双源（12 处）· `release` 忽略 owner · `setProviderKeys` 删反代账号不释放实例/端口 · 更新进度双状态源 · OAuth 跨轮误杀 |

⚠ 前端门禁修复前**实测**：`tsc --noEmit` 0 错、`vitest run` 3 文件 15 用例全绿、`eslint` 0 错 ——
即这些测试一直是对的，只是从未被调用。已补进 `ci-core.sh`（verify 在 build **之前**）。

### 四轮累计

```
内核  npm test   68 文件 / 1255 断言 / 0 失败（起点 1098，+157）
壳    cargo test  92 项 / 0 失败（起点 77，+15）
前端  verify     tsc 0 错 / eslint 0 错 / vitest 15 通过（此前从未运行）
```
### 第五轮：剩余 P2/P3 + frpmgr/objects 审计（2026-09-12 续）

| # | 位置 | 缺陷 |
|---|---|---|
| **P2-5** | 内核 router-ops + 前端 | `added++` **不 await 检测** → 计数虚高（N 个全被 discarded 也报「已添加 N 个」）；后端改为等齐结果并回报 discarded/discardedKeys，前端如实提示 |
| **P2-8** | 内核 relay/manager | `list()` 每次触发 reconcile，而 reconcile 内**逐实例串行 TCP 探测**（600ms/个）→ 与 2s 节拍叠加；加**单飞** |
| **P2 双写** | 内核 supervisor | daemon 模式下「守卫不得写 providers.json」的纪律**只在 3 条路径中的 1 条**执行 → 另两条会与 daemon 双写覆盖；收敛为 `_disableRouterPersist()` |
| **P2 配套** | 内核 objects.js | P2-2 修复后，无 owner 的 `release(port)` 回退变成**绕过归属校验**的路径 → 删除 |

本轮对 `relay/frpmgr.js`（384 行）与 `guard/lifecycle/objects.js`（353 行）做只读审计：**未发现新缺陷**。
两者质量突出：frpmgr 的定时器全部 unref、SIGKILL 兜底用 `exit` 而非 `killed`（注释记录了旧实现的错）、
tar 解包只写硬编码文件名（无遍历风险）、下载带 60s 超时；objects.js 的四条设计公理与实现一致。

### 五轮累计

```
内核  npm test   69 文件 / 1278 断言 / 0 失败（起点 1098，+180）
壳    cargo test  92 项 / 0 失败（起点 77，+15）
前端  verify     tsc 0 错 / eslint 0 错 / vitest 15 通过 / build 成功
```