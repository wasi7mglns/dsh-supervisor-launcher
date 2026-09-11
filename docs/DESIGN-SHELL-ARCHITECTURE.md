# 桌面壳工程架构规范

> 2026-09-11 · 目标：把壳做成**标准的、跨平台的、工业级**工程
> 本文件是**规范**（normative）。每条不变量都对应一条**会失败的测试**。

---

## 一、现状诊断（实测，非感觉）

| 指标 | 实测值 | 判断 |
|---|---|---|
| **平台分支分布** | **43 处 / 8 个文件**（另 7 处 `#[cfg(test)]` 不计）| ❌ **零平台抽象层** |
| 内核对照（同口径）| **67 处，60 处在 `platform/os/`（90%）**，仅 7 处泄漏到域层 | ✅ 内核有平台层，壳没有 |
| `main.rs` 规模 | **1487 行** = 20 个 IPC 命令 + 4 个 CLI + 平台服务控制 + 业务逻辑 | ❌ 单体，无分层 |
| 平台分支明细 | `node.rs` 11 / `main.rs` 10 / `service.rs` 9 / `env.rs` 6 / `bounded.rs` 2 / `nodeprobe.rs` 2 / `update.rs` 2 / `core.rs` 1 | ❌ 加一个平台要翻 8 个文件 |
| 分层违规 | `main.rs:542-605` 有 4 份 `#[cfg]` 的 `start/stop_guard_service`，与 `service.rs` 的 `ensure_defined` **同属一个概念却分居两层** | ❌ 加平台要改两处不同层 |
| 错误类型 | `Result<_, String>` **42 处**，自定义 Error 枚举 **0 个** | ❌ 只能字符串匹配，无法程序化处理 |
| 前端隔离 | `bootstrap.html` **760 行单个 `<script>` 块** | ❌ 一处语法错 → 全页不执行（已真实发生一次）|
| 架构文档 | 10 份 docs，全文 `Contract` 0 次、`不变量` 0 次、`平台矩阵` 0 次 | ❌ 没有能回答「加一个平台要改哪些地方」的文档 |
| 测试 | `cargo test` 68 项；`bootstrap_flow.rs` 1159 行（B1–B55）| ✅ 测试基础健康 |
| 有界执行 | `bounded.rs` 191 行，全部阻塞调用经它 | ✅ **良好基础设施**，应推广 |

**结论：壳不是「一塌糊涂」，是「缺一层抽象 + 缺一份规范 + 缺把规范变成门禁」。**

---

## 二、目标架构

### 2.1 分层与依赖方向（单向，可门禁）

```
src-tauri/src/
├── main.rs                 # 仅：组装 + 入口（目标 < 150 行）
│
├── commands/               # ① IPC 边界层
│   ├── mod.rs              #    参数校验（serde）+ 调用 domain + 序列化结果
│   ├── env.rs  node.rs  core.rs  mirror.rs  shell.rs  window.rs
│   └── （**禁**：业务逻辑、平台判断、直接 Command 调用）
│
├── domain/                 # ② 业务层（平台无关）
│   ├── probe/              #    环境探测（nodeprobe 拆分）
│   ├── provision/          #    供给：Node 安装 + 内核安装
│   ├── mirror/             #    镜像目录 + 选择 + 契约投放
│   ├── update/             #    壳自更新 + 护栏账本
│   └── contract/           #    与内核的契约（schema 校验 + 读写）
│
├── platform/               # ③ 平台适配层 —— **全仓唯一 #[cfg(target_os)] 所在地**
│   ├── mod.rs              #    trait Platform / trait ServiceControl
│   ├── linux.rs  macos.rs  windows.rs
│   └── unsupported.rs      #    显式不支持（复用 service.js 的 CapabilityError 语义）
│
├── infra/                  # ④ 原语（与业务无关）
│   ├── bounded.rs          #    有界执行（既有，强化）
│   ├── fs.rs  net.rs  proc.rs
│
└── error.rs                # 结构化错误 ShellError
```

```
依赖方向（硬约束）：
  commands ──▶ domain ──▶ platform ──▶ infra
                │                        ▲
                └────────────────────────┘
  禁止反向（infra 不得依赖 domain；platform 不得依赖 commands）
```

### 2.2 平台适配层（把 43 处收拢成 1 个契约）

```rust
// platform/mod.rs
/// 平台能力契约。**每个能力要么实现，要么显式声明不支持**（不得静默成功）。
pub trait Platform: Send + Sync {
    fn name(&self) -> &'static str;

    // ── Node 制品与安装 ──
    /// 制品形态（Linux tar.xz / macOS pkg / Windows msi）与文件名。
    fn node_artifact(&self, version: &str) -> Option<NodeArtifact>;
    /// 安装（含提权）。**提权为本平台专有实现**。
    fn install_node(&self, artifact: &Path) -> Result<InstallReport, ShellError>;
    /// npm 子包平台标签（linux-x64 / darwin-arm64 / win-x64 …）。
    fn core_platform_tag(&self) -> &'static str;

    // ── 探测 ──
    fn path_dirs(&self) -> Vec<PathBuf>;
    fn known_node_locations(&self) -> Vec<PathBuf>;
    /// 该目录是否在「本地固定盘」（Windows 需排除网络盘/可移动盘）。
    fn is_local_fixed_dir(&self, dir: &Path) -> bool;

    // ── 服务（定义 + 启停 **同一对象** —— 修掉历史上的分层违规）──
    fn service(&self) -> &dyn ServiceControl;

    // ── 能力声明（供契约与面板）──
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

**关键决定：把「定义」与「启停」合并到同一个 `ServiceControl`。**
现状是 `service.rs` 管定义、`main.rs` 管启停 —— 同一个概念的 per-OS 知识分居两层，
加一个平台要改两处不同层。这是最典型的分层违规。

### 2.3 错误模型（结构化，不再靠字符串）

```rust
// error.rs
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ShellError {
    /// 探测失败：**必须带阶段与耗时** —— 「卡住时看得见」是硬要求。
    Probe   { stage: String, cause: String, elapsed_ms: u128 },
    Network { url: String, cause: String },
    Install { platform: String, cause: String },
    Service { action: String, cause: String },
    /// 契约读写（与内核的边界）
    Contract{ file: String, cause: String },
    /// IPC 边界自身
    Ipc     { cause: String },
    /// **显式不支持**（替代静默成功）—— 与内核 service.js 的 CapabilityError 同规
    Unsupported { capability: String, platform: String },
}
```

前端据此按 `kind` 给**不同的可操作建议**，而不是显示一句可能判错的话。

---

## 三、跨平台规范（可执行，不是文档）

### 3.1 平台矩阵：每条能力 × 每个平台 = 实现 或 **显式不支持**

| 能力 | Linux | macOS | Windows | 实现位 |
|---|---|---|---|---|
| 环境探针（候选枚举/版本/PATH）| ✅ | ✅ | ✅ | `domain/probe` |
| Node 制品解析 | ✅ `tar.xz` | ✅ `pkg` | ✅ `msi` | `platform/*::node_artifact` |
| Node 安装（提权）| ✅ `pkexec` | ✅ `osascript` | ✅ `msiexec` | `platform/*::install_node` |
| 镜像测速与选择 | ✅ | ✅ | ✅ | `domain/mirror` |
| 内核安装/升级 | ✅ | ✅ | ✅ | `domain/provision` |
| 服务定义（守卫）| ✅ systemd | ✅ LaunchAgent | ✅ schtasks | `platform/*::ServiceControl` |
| 服务启停 | ✅ `systemctl --user` | ✅ `launchctl` | ✅ `schtasks` | 同上 |
| 提权通道探测 | ✅ | ✅（恒有）| ✅ | `platform/*::has_privilege_channel` |
| 壳自更新 | ✅ deb/rpm | ✅ app | ✅ exe/msi | `domain/update` |

**不变量 P1**：矩阵的每一格必须是「实现」或「显式不支持」。
「显式不支持」在代码里表现为返回 `ShellError::Unsupported`，**绝不静默成功**。

### 3.2 契约层（与内核的唯一耦合面）

```
domain/contract/
  ├── schema.rs     契约版本常量 + 校验
  ├── mirror.rs     写 ~/.dsh/supervisor/registry.json（壳是唯一写入方）
  └── identity.rs   写/读 ~/.dsh/shell/identity.json
```

| 不变量 | 内容 |
|---|---|
| **C1** | 每个契约文件**只有一个写入方**（壳写 `registry.json`；内核写 `update-journal.json`）|
| **C2** | 契约带 `schema` 版本；不匹配时**明确拒绝**并记录，不静默降级 |
| **C3** | 写入必须**原子**（`tmp + rename`），读取必须容忍缺失 |
| **C4** | 壳启动时**必须导出**契约（修掉「只在手动改镜像时才导出」的缺口）|

### 3.3 前端隔离（消除「静默死亡」）

现状：`bootstrap.html` 760 行单块 JS —— 一处语法错 → **全页不执行**（已真实发生）。

目标：

```
bootstrap/
  ├── bootstrap.html          # 骨架 + 各模块 <script> 引入
  ├── shell.html              # 壳框架
  └── js/
      ├── 00-runtime.js       #   IPC 获取 + withTimeout + 全局 onerror
      ├── 10-ui.js            #   状态/步骤/失败页
      ├── 20-env.js           #   环境检测步骤
      ├── 30-node.js          #   运行环境步骤
      ├── 40-mirror.js        #   镜像设置
      ├── 50-shell-update.js  #   桌面版本
      ├── 60-kernel.js        #   内核版本
      └── 70-guard.js         #   守卫就绪 + 面板
```

| 不变量 | 内容 |
|---|---|
| **F1** | 每个 JS 文件**独立语法检查**（每文件一个断言，非「整页一个」）|
| **F2** | 全局 `window.onerror` + `unhandledrejection` → 上报后端落 `shell.log` |
| **F3** | 启动自检：IPC 不可用时**明确报错**，不得静默停在静态文案 |
| **F4** | 需要 IPC 的页面必须是**主帧**（既有门禁 B54）|

---

## 四、门禁（把规范变成会失败的测试）

**没有门禁的规范等于没有规范。** 每条不变量对应一条断言：

| # | 门禁 | 违反即失败 |
|---|---|---|
| **G1** | 平台分支只允许出现在 `platform/` 内 | 把 43 处收拢成 1 处可维护点 |
| **G2** | `commands/` 内不得出现 `std::process::Command`、`#[cfg]` | 命令层只做校验与委托 |
| **G3** | `main.rs` ≤ 200 行；每个命令体 ≤ 40 行 | 防单体回潮 |
| **G4** | 每个能力 × 平台：实现 或 `Unsupported` | 防「假装支持」（macOS 壳自启的历史缺陷）|
| **G5** | 每个前端 JS 文件独立语法正确 | 防单点语法错全页死 |
| **G6** | 契约文件带 `schema`；壳启动必导出 | 防两仓静默错位 |
| **G7** | 所有可能阻塞的调用经 `infra::bounded` | 已有 B32/B40，扩展到新层 |
| **G8** | 注释引用的**仓内路径必须存在** | 防「仓库对自己说谎」（已发现 30+ 悬空引用）|

### 门禁 G1 的实现示意

```rust
#[test]
fn g1_platform_branches_only_in_platform_layer() {
    // 扫描 src/**/*.rs，若 platform/ 之外出现平台分支即失败。
    // 现状：43 处散落 8 文件 → 目标全部收进 platform/（内核已是此形态：67 处中 60 处在 platform/os/）
    // 把「加一个平台要翻 8 个文件」变成「只改 platform/」
}
```

---

## 五、迁移路径（分阶段，每阶段可独立发布与回滚）

| 阶段 | 内容 | 风险 | 可回滚 |
|---|---|---|---|
| **P0** | 建 `platform/` 层，把 **43 处平台分支**搬位置不改逻辑；合并「服务定义 + 启停」；加门禁 **G1/G2** | 低（纯搬移）| ✅ 单提交回滚 |
| **P1** | `main.rs` 拆成 `commands/` + `domain/`；加门禁 **G3** | 中（拆文件）| ✅ |
| **P2** | 引入 `ShellError`，IPC 边界改为结构化错误；前端按 `kind` 出建议 | 中（前后端同时改）| ⚠️ 需前后端一起回滚 |
| **P3** | 前端拆分 + 全局 onerror；门禁 **G5** | 低 | ✅ |
| **P4** | 契约层（`domain/contract/`）+ 壳启动导出 + `schema`；门禁 **G6** | 低（新增能力）| ✅ |
| **P5** | 平台矩阵自检 CLI（`--platform-matrix`）+ 三平台 CI 跑真实调用；门禁 **G4** | 低 | ✅ |

**顺序不可颠倒**：P0 是地基（没有平台层，后面的分层都无处安放）。

---

## 六、本规范与既有成果的关系

| 已有 | 状态 |
|---|---|
| `bounded.rs`（有界执行）| ✅ **保留并强化** —— 它是本仓最好的基础设施，应成为 `infra::bounded` |
| `bootstrap_flow.rs` B1–B55 | ✅ **全部保留** —— 门禁体系的基础 |
| `service.rs` 的 per-OS 实现 | ✅ **作为 `platform/` 的模板** —— 它本来就是正确模式 |
| `nodeprobe.rs` 的「规则一/规则二」（每步上报 + 硬死线）| ✅ **提升为全仓不变量** |
| 主帧 IPC 不变量（B54）| ✅ **保留** —— 已由真实事故确立 |

---

## 七、不变量总表（本规范的执行清单）

| 类别 | # | 不变量 |
|---|---|---|
| 分层 | L1 | 依赖单向：`commands → domain → platform → infra` |
| 分层 | L2 | `#[cfg(target_os)]` 只在 `platform/` |
| 分层 | L3 | `commands/` 只做校验与委托 |
| 平台 | P1 | 每能力 × 每平台 = 实现 或 **显式 Unsupported** |
| 平台 | P2 | 「定义」与「启停」同属一个 `ServiceControl` |
| 平台 | P3 | 平台差异**不得**泄漏到 domain |
| 错误 | E1 | 对外错误一律 `ShellError`（结构化）|
| 错误 | E2 | 探测类错误**必须带阶段与耗时** |
| 契约 | C1 | 每个契约文件只有一个写入方 |
| 契约 | C2 | 契约带 `schema`，不匹配明确拒绝 |
| 契约 | C3 | 原子写 + 容忍缺失 |
| 契约 | C4 | 壳启动必导出契约 |
| 前端 | F1 | 每个 JS 文件独立语法检查 |
| 前端 | F2 | 全局错误上报落 `shell.log` |
| 前端 | F3 | IPC 不可用必须明确报错 |
| 前端 | F4 | 需要 IPC 的页面必须是主帧 |
| 阻塞 | B1 | 可能阻塞的调用经 `bounded` |
| 阻塞 | B2 | 阻塞前必须 `stage()` 上报（「不上报不如不调用」）|
| 阻塞 | B3 | 必须有硬死线（「probing」不得永久为真）|
| 文档 | D1 | 注释引用的仓内路径必须存在 |
| 文档 | D2 | 能力声明必须由可执行断言支撑（文字不构成证据）|

---

## 八、验收标准（何时算「标准壳工程」）

```
1. cargo test 全绿，且包含 G1–G8 全部门禁
2. 三平台 CI 各跑一次 --platform-matrix 自检，输出与本文档 §3.1 一致
3. main.rs ≤ 200 行；commands/ 内零 #[cfg]、零 Command
4. 前端 8 个 JS 模块，各自语法门禁；kill 任一模块不影响其余模块的加载与报错
5. 契约 schema 校验 + 壳启动导出，内核实测能读到并按其 probe 规格复测
6. 本文档 §七 的每一条不变量，都能指出对应测试名
```
