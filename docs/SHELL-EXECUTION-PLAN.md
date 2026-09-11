# 桌面壳稳定与热更新 · 完整执行方案（SHELL-EXECUTION-PLAN）

> 📌 **配套权威文档**：`RELEASE-AND-UPDATE-MECHANISM.md`（发布与更新机制总纲：两条发布链、产物矩阵、端到端时序、边界约束）。
> 本方案聚焦「怎么实施」，总纲聚焦「机制是什么」。

> 依据：D1 已定案（**保留 Tauri 原生壳**，最终形态 = 桌面级产品）+ 采纳我的全部建议（D2–D7）。
> 判据：**稳定与可靠优先**，承认有舍有得，工业级标准。
> 本文件是执行方案，不是代码变更。

---

## 0. 定案摘要（本方案采纳的决定）

| # | 决策 | 结论 |
|---|---|---|
| D1 | 是否保留原生壳 | **保留**（桌面级产品形态） |
| **D4** | **Linux 分发形态** | **废弃 AppImage，采用标准 Linux 包（deb，可选 rpm）** —— 体积/耗时均优 20 倍，且与生产现状一致 |
| D2 | Linux 主通道 | ~~两者分工~~ → **D4 覆盖：废弃 AppImage，采用标准 deb 包**（详见 D4 行） |
| D3 | 更新执行方 | **壳直连公网自更新（冷启动即生效）**；内核只做安全网（预取/备份/观察/回退/审计） |
| D4 | 「强制」边界 | **强力尝试 + 有界失败放行**（阈值 2 次） |
| D5 | Windows 安装模式 | **per-user**（Tauri NSIS 默认，免提权） |
| D6 | 签名与公证 | **现在就做**（桌面级硬性前提） |
| D7 | 可靠性工程次序 | **R1→R4 先行**（日志 / 版本可见 / panic 收敛 / 配置健壮化） |

---

## 1. 目标架构（一句话）

**壳直连公网发布通道自更新（冷启动即可用）；内核不做更新源，只做安全网（预取/备份/观察/有界回退/审计）。**

> **发布通道已定（实测验证，详见 `SHELL-UPDATE-CHANNEL-VERIFICATION.md`）：npm CDN（unpkg / jsdelivr 直链包内文件）。**
> 原因：**GitHub Release 直连实测仅 15–28 KB/s**（`objects.githubusercontent.com` 15s 超时），
> 77MB AppImage 需约 45 分钟，且 **Tauri 的传输超时会先触发 → 更新永远失败**；
> 而 **npm CDN 实测 1.71 MB/s**（大文件），与内核在用的 npmmirror（1.44 MB/s）同级，且**零新增基础设施**
> （我们已在用 npm 发布内核子包）。

> 另经源码验证：**Tauri 的 `url` 字段接受任意 HTTPS URL**（`pub url: Url`，无域名白名单），
> **签名校验独立于托管位置** —— 故换 CDN 不影响安全性。

> ⚠ 本节已按用户质疑修正（详见 `SHELL-UPDATE-TRIGGER-CORRECTION.md`）。
> 原设计把内核当「更新权威 + 产物提供方」，却同时要求壳**在内核之前**更新自己——**自相矛盾**：
> 冷启动/内核未安装/内核损坏时内核不在，壳拿不到更新，只能「下次启动生效」，
> 恰好在最需要它的时刻失效。修正方式：拆开「更新源」与「安全网」两个角色。

```
       ① 直连公网（HTTPS，Tauri 强制验签）
┌──────────────────────────────────────────────┐
│ npm CDN（unpkg / jsdelivr 直链包内文件）       │  ← 唯一「更新源」
│   shell-manifest.json: version + url + sig    │    不依赖本机任何组件
│   实测 1.71 MB/s（大文件）                     │
└──────────────────────────────────────────────┘
        ▲
        │
┌─ 壳（Tauri 原生，桌面形态保留）──────────────────────────────────────────┐
│  [探针] 只读：平台/arch/网络可达/installKind（失败不阻断）                │
│  门 0 壳自更新：调用 tauri-plugin-updater                                 │
│        · 端点 = 公网发布通道（3s 超时；失败即放行）                       │
│        · **先备份当前产物**（原地安装无旧版本，不备份就无法回滚）         │
│        · 校验签名（Tauri 强制，不可关闭）                                 │
│        · 安装（平台替换机制由 Tauri 负责）→ app.restart() 进新版本        │
│        · 无更新 / 离线 / 不可自更新(deb) / 已达阈值 → **放行**            │
│  门 1 Node  →  门 2 内核（按兼容区间选版）  →  门 3 守卫 → 面板          │
│  达 finish_boot → ② 上报健康（= 更新确认信号）                            │
│  自身**不含**自定义下载/校验/版本比较逻辑（只调用官方插件）                │
└──────────────────────────────────────────────────────────────────────────┘
        │ ③ 读 identity.json / 观察健康            ▲ ④ 回退时重新应用缓存
        ▼                                          │
┌─ 内核 / 守卫（受 systemd Restart=always 监督；**安全网**，非更新源）────┐
│  1. 预取（优化）：提前下载 + 验签 + 缓存 → 使壳门 0 几乎瞬时              │
│  2. 备份：保留当前 + 上一个壳产物（回滚素材）                            │
│  3. 观察：读 identity.json（version / phase / attempt 自增）              │
│  4. 有界回退：更新未确认且 attempts > 2 → 判坏 + 加黑名单 + 用缓存回装    │
│  5. 审计：update-journal + 事件日志（available/started/confirmed/rolled_back）│
│  6. 版本配对：校验「壳 ↔ 内核」兼容区间                                  │
└──────────────────────────────────────────────────────────────────────────┘
```

### 为什么这样分工（四条硬理由）

1. **冷启动自洽**：壳更新不依赖内核是否在跑 → 满足「启动即更新壳」。
2. **与壳现有行为一致**：壳**本来就在直连公网**（`nodejs.org`、npm 镜像源），已验证具备 HTTPS 能力
   （`ureq + tls`）——连发布通道不是新增能力。
3. **受监督方做安全网**：壳崩溃时无人拉起（无 systemd 单元），而内核**可能仍然活着**——
   它是唯一有能力回退壳的角色，且已被监督、已有全套网络/镜像/日志/任务工程。
4. **平台替换机制不自己造**：macOS 的 `.app` 是**目录**，替换 + 签名有效性 + quarantine 极易做错；
   Windows 需静默安装器语义；AppImage 需替换自身。这些 **Tauri 官方 updater 已实现**。

### 修正带来的净收益

| 收益 | 说明 |
|---|---|
| 冷启动可用 | 满足用户核心诉求「启动即更新壳」 |
| **少一个安全妥协** | 端点改公网 HTTPS 后，**不再需要** `dangerousInsecureTransportProtocol` |
| **少一组协议耦合** | 壳与内核之间不再有「更新协议」需同步演进（原风险 K4 基本消失） |
| 内核职责更纯 | 只做它独有的能力——受监督的安全网，不重复公网分发的活 |

---

## 2. 接口与文件契约（先定契约，后写代码）

### 2.1 磁盘布局

```
~/.dsh/shell/
  ├── identity.json          # 壳上报的身份与阶段（内核只读）
  ├── update-journal.json    # 内核维护的更新账本（唯一权威）
  ├── shell.log              # 壳落盘日志（滚动 5 x 1MB）
  └── cache/<version>/       # 内核预置的更新产物缓存（含 .sig）
```

### 2.2 identity.json（壳写，内核读）

```json
{
  "version": "0.2.0",
  "platform": "linux", "arch": "x64",
  "pid": 12345,
  "startedAt": "2026-09-11T02:00:00Z",
  "phase": "boot|node|kernel|guard|panel|ready",
  "attempt": 1,
  "installKind": "deb|rpm|nsis|msi|app",
  "selfUpdateCapable": true
}
```

`attempt` 是关键：**每次壳启动自增**。它让内核能判断「更新后壳是否反复起不来」，
而**不依赖壳能成功走到日志**（因为壳在启动最早阶段就写这个文件）。

### 2.3 update-journal.json（内核写，壳只读）

```json
{
  "from": "0.1.0", "to": "0.2.0",
  "attempts": 2, "maxAttempts": 2,
  "confirmed": false, "rolledBack": false,
  "startedAt": "...", "lastAttemptAt": "...",
  "pinnedVersions": ["0.2.0"]
}
```

`pinnedVersions` = 已判定为坏的版本黑名单；内核不再向壳提供它们（**防循环**）。

### 2.4 内核本地端点（仅回环；**不含更新源职责**）

| 端点 | 用途 |
|---|---|
| `POST /shell/health` | 壳上报 `{phase, version, attempt}`；`phase=ready` 即**更新确认信号** |
| `GET /shell/status` | 面板展示：壳版本 / 更新状态 / 回退记录 / 缓存版本 |
| `POST /shell/prefetch` | 手动触发内核预取（面板按钮；可选） |
| `POST /shell/rollback` | 手动回退到上一可用版本（排障入口；可选） |

**已移除**（原设计）：`GET /shell/update/v1/check` 与 `/shell/update/v1/artifact/<file>`——
内核不再是壳的更新源（壳直连公网）。

> 端点带 `/v1/` 版本段：内核演进破坏兼容时才有机会并行，符合我们已有的 API 契约纪律。

### 2.5 Tauri 侧配置要点（含一个必须知道的坑）

- `bundle.createUpdaterArtifacts: true`；`plugins.updater.pubkey` = 公钥（**不能是文件路径**）。
- `plugins.updater.endpoints = [npm CDN 上的 shell-manifest.json]`（例如 `https://unpkg.com/@dsh-sup/shell-<os>-<arch>@latest/shell-manifest.json`）——**TLS 强制天然满足**，
  因此**不再需要** `dangerousInsecureTransportProtocol`（原设计为回环 HTTP 而不得不开的妥协，现已消除）。
- **坑 1（多端点回退语义）**：官方文档原文——「Tauri will **only continue to the next url if a
  non-2XX status code is returned**」。即**网络超时/连接失败不会触发回退**。
  → 「离线即放行」的控制权必须在**壳自己手里**（自加 3s 超时 + 短路），不能指望 updater 回退。
- **坑 2（回退需要放行降级）**：Tauri updater 默认只向前。内核驱动的回退需壳用 `version_comparator`
  允许「与当前不同」的版本；**循环风险由内核的 attempts + pinnedVersions 兜住**。
- Windows 建议 `plugins.updater.windows.installMode: "passive"`（小进度条、无需交互）。
- **强制前置**：更新前**必须备份当前产物**（Tauri 原地安装，不保留旧版本 → 不备份则无法回滚）。
- **通道已定**：npm CDN（unpkg 主 / jsdelivr 备）。清单格式用 Tauri **静态 JSON** 语义：
  `{ version, platforms: { "<os>-<arch>": { url, signature } } }`。详见 `SHELL-UPDATE-CHANNEL-VERIFICATION.md`。

---

## 2.6 引导顺序（按用户诉求修正并精化）

用户要求：**先检测环境 → 检测壳 → 检测内核 → 进入主系统**。方向正确，我做一处精化并说明理由。

```
壳进程启动
 │
 ├─[探针] 只读环境探测（~200ms；失败不阻断）
 │        平台 / arch / 网络可达性 / 安装形态(deb|rpm|nsis|app|msi)
 │        目的：判断「能否自更新」，避免无谓等待
 │
 ├─[门 0] 壳自更新（直连公网）   ← 用户要求的「先更新壳」
 │        · 无更新 / 离线 / 不可自更新(deb) → 放行（落盘记录原因）
 │        · 有更新 → 备份 → 下载 → 验签 → 安装 → 重启进新版
 │        · **失败 → 显示选择页【重试】【继续】**（用户已确认，见 §2.7）
 ├─[门 1] 环境就绪：Node 运行时（低于最低标准才安装）
 ├─[门 2] 内核：检测 → 按兼容区间选版 → 安装
 ├─[门 3] 守卫就绪 → 面板
 └─[确认] finish_boot → 上报健康（= 更新确认信号）
```

**为什么壳更新要排在 Node 之前**（而非仅在内核之前）：

| 理由 | 说明 |
|---|---|
| 壳是原生二进制，**不需要 Node 就能更新自己** | 更新壳无前置依赖 |
| **新壳可能带有不同的 Node 要求** | 例如新的最低版本、新的镜像/校验逻辑（`node.rs` 在壳里） |
| 用旧壳装 Node 有装错的风险 | 装完再被新壳判定不合规 = 白装一次（浪费带宽与时间） |

**要点**：廉价的**只读探针**放最前（符合「先检测环境」），
但**会改动系统的壳更新**必须排在 **Node 安装**之前——每个写动作都应由**最新版本的壳**执行。
用户原话在**检测**层面完全成立；精化只在「**安装/更新**」类动作的排序（壳 → Node → 内核）。

---

## 2.7 壳更新失败时的选择页（用户已确认）

**定案**：达阈值后**不静默放行**，而是显示一个选择页：**【重试】【继续】**。

```
┌────────────────────────────────────────────────┐
│  桌面壳更新未完成                              │
│                                                │
│  当前版本 v0.1.0 · 目标版本 v0.2.0             │
│  失败原因：下载超时 / 验签失败 / 安装失败       │
│  已尝试 2 次                                   │
│                                                │
│   [ 重试 ]              [ 继续使用当前版本 ]    │
└────────────────────────────────────────────────┘
```

### 设计要点

| 要点 | 说明 |
|---|---|
| **【继续】必须始终可用** | 保证极端情况下用户仍能进入产品（这是「有界失败放行」的用户可见形式） |
| **【重试】重置尝试计数** | 手动重试是明确的用户意图，应重新给足预算（但要落盘记录，避免无限循环） |
| **失败原因要如实呈现** | 网络 / 验签 / 安装 / 未知，各自不同，不说笼统的「更新失败」 |
| **区分「不适用」与「失败」** | 离线、`installKind=deb`（不可自更新）→ **不进选择页**，直接放行 + 面板提示 |
| **选择页本身不得成为新的故障点** | 它是引导页（HTML/JS）的一部分，失败也要能渲染；渲染不出来时**退化为静默放行** |

### 与既有机制的关系

- 选择页属于**门 0（壳自身）**，**不涉及内核**——内核的更新与回滚不受影响（见 P6 硬约束）。
- 该页复用现有 `bootstrap.html` 的失败态框架（已有「重试」「诊断」按钮），是**增量**而非新建流程。

---

## 3. 执行阶段（P0 → P7）

阶段划分原则：**每一阶段可独立交付、独立验收、独立回滚**；P0 必须最先做（没有它，后续问题无法诊断）。

```
P0 地基（壳可诊断/可自愈）  ← 无架构变更，先做
      │
P1 内核侧安全网（预取/备份/观察/回退）─┐
      │                                  │ 可并行
P2 壳接入 Tauri updater（直连公网）   ─┘
      │
P3 健康确认 + 有界回退（闭环）
      │
P4 签名 + 发布链路
      │
P5 分发形态落地（per-user 主通道）
      │
P6 版本配对（壳↔内核兼容区间）
      │
P7 纪律：逻辑下沉（长期）
```

---

### P0 — 地基：让壳可诊断、可自愈（零架构变更）

**目标**：把壳从「不可诊断 / 不可自愈」拉升到「可诊断 / 可自愈」。**这是所有后续工作的前提。**

| 任务 | 内容 | 交付物 |
|---|---|---|
| P0.1 | **落盘日志**：`~/.dsh/shell/shell.log`，滚动 5 x 1MB；记录启动各阶段、每次 IPC 失败、panic 信息 | 壳 `log.rs` |
| P0.2 | **身份与心跳**：写 `identity.json`（含 `attempt` 自增、`installKind`、`selfUpdateCapable`） | 壳 `identity.rs` |
| P0.3 | **panic 收敛**：`main.rs:603 default_window_icon().expect(...)` 改降级；9 处 `lock().unwrap()` 改毒化容忍 helper | 壳 `main.rs` |
| P0.4 | **配置读取健壮化**：统一兜底常量（消除 3100 vs 36360 不一致）；读不到时**明确报错**（落盘 + 上报 + 引导页显示），不静默猜 | 壳 `env.rs` |
| P0.5 | **自愈**：内核侧检测「壳长时间未运行/未达 ready」→ 桌面通知 + 事件；可选提供用户级重启入口 | 内核 `domains/shell/` |
| P0.6 | **本地构建能力**（**前置阻塞项**）：本机**无 cargo/rustc**，壳无法本地构建验证 | 安装 Rust + Tauri Linux 依赖 |

**验收门禁**：
- 人为触发 panic（删图标 / 破坏 config.json）→ **能从 `shell.log` 明确定位**，且引导页显示可读错误（不再静默/白屏）。
- `--node-plan` / `--core-plan` 无头冒烟仍通过（现有 CI 已在用）。
- `identity.json` 在壳启动 1 秒内出现且 `attempt` 递增。

**回滚**：纯新增（日志/身份为新增文件），配置解析改为「失败报错」是行为变更 → 需保留开关 `DSH_SHELL_STRICT_CONFIG=0` 回到旧兜底。

**平台**：三平台一致；`identity.json` 路径用统一 `~/.dsh/shell/`（Windows 用 `%USERPROFILE%`）。

---

### P1 — 内核侧：安全网（预取/备份/观察/回退/审计；不动壳）

**目标（修正后）**：内核具备「**预取 → 备份 → 观察 → 有界回退 → 审计**」的安全网能力。
**注意**：内核**不再**是壳的更新源（壳直连公网），故**不提供** `/shell/update/*` 端点。

| 任务 | 内容 | 交付物 |
|---|---|---|
| P1.1 | 新域 `src/domains/shell/`：版本查询（**只读**复用 `dist.fetchLatestVersion` 的镜像/重试体系）、下载、**验签**、缓存、GC（保留当前 + 上一个）——**产物供内核自己回退壳用**；**不得**调用 `runNpmInstall`、**不得**写任何内核版本状态 | `domains/shell/index.js` |
| P1.2 | 管理端点 `POST /shell/health`、`GET /shell/status`（+ 可选 `prefetch` / `rollback`） | `src/api/shell.js`（新域路由，纳入 `surface.js` 契约 + 双向一致性测试） |
| P1.3 | `update-journal.json` 读写与状态机（`pending -> confirmed | rolled_back`） | `domains/shell/journal.js` |
| P1.4 | 面板展示：壳版本 / 更新状态 / 回退记录 / 「立即重启以应用」 | `ui/src/features/supervisor/...` |
| P1.5 | 事件：`shell_update_available/started/confirmed/failed/rolled_back` | 事件日志 |

**验收门禁**：
- 内核能独立列出壳最新版本、下载并缓存（用假清单 + 假产物测试）。
- `POST /shell/health` 能正确驱动 journal 状态机（pending → confirmed / rolled_back）。
- 新增路由纳入 `src/api/surface.js` 且 `test/api-surface-test.js` 双向一致性通过。
- 面板可见壳版本（此时壳尚未上报 → 显示「未知」，P2 后填实）。

**回滚**：纯新增域与端点，去除路由即回退。

---

### P2 — 壳侧：接入 Tauri updater（应用端）

**目标（修正后）**：壳具备「**直连公网**检查 → 备份 → 下载 → 验签 → 安装 → 重启」，并在失败时**放行**。

| 任务 | 内容 |
|---|---|
| P2.1 | 加依赖 `tauri-plugin-updater` + `tauri-plugin-process`（`app.restart()`） |
| P2.2 | `tauri.conf.json`：`pubkey`、`endpoints=[npm CDN 上的 shell-manifest.json]`（HTTPS，**无需** insecure 开关）、`createUpdaterArtifacts: true`、Windows `installMode: passive` |
| P2.3 | **门 0 逻辑**：只读探针 → 检查（**自加 3s 超时**）→ **先备份当前产物** → 有更新则下载安装 → 重启；失败/离线/已达阈值 → **显示选择页【重试】【继续】**（§2.7；离线与 deb 直接放行不进选择页） |
| P2.4 | **健康上报**：`phase=ready`（finish_boot）时 POST `/shell/health`；各阶段也上报 progress |
| P2.5 | **可自更新判定**（已按源码验证修正）：`AppImage`/`App`/`NSIS`/`MSI` → 直接可自更新；**`deb`/`rpm` → 亦可自更新**（Tauri `install_deb` 走 `pkexec` 图形提权，需用户确认）；仅当「安装目录不可写 **且** 无提权通道（无 pkexec/sudo）」→ `selfUpdateCapable=false`，跳过门 0 并提示手动更新 |

**关键纪律**：壳内**不写**自定义下载/校验/版本比较——只用插件 API。

**验收门禁**：
- 本地伪造更高版本（本地起一个 HTTPS 清单 + 带正确签名的产物）→ 壳能更新并重启，重启后版本为新版本。
- **签名错误必须被拒**（这是安全底线，需专门测试）。
- **断网 / 端点超时 → 壳 3s 内放行**进入面板（不阻断、不卡住）。
- **更新前备份产物已生成**（回滚素材存在性断言）。
- `installKind=deb` → 走 pkexec 提权路径（**能自更新**）；无提权通道时才提示手动更新（**不进选择页**）。
- **失败选择页可用**：【重试】能重新尝试且重置计数；【继续】能进入面板（**始终可用**）；失败原因分类正确显示。

**回滚**：移除插件与门 0 代码即回退（无数据迁移）。

---

### P3 — 健康确认 + 有界回退（闭环，核心价值）

**目标**：把「更新」从「装上了」升级为「**确认真的能用**」；失败自动回到已知可用版本。

| 任务 | 内容 |
|---|---|
| P3.1 | 内核维护 `update-journal`：更新前记 `from/to`，壳上报 `ready` 且版本匹配 → `confirmed=true` |
| P3.2 | **有界策略**：壳每次启动自增 `attempt`；内核发现「journal 未确认且 `attempts > 2`」→ 判坏版本 |
| P3.3 | **回退**：把坏版本加入 `pinnedVersions`（壳门 0 读取并跳过该版本）；内核用缓存**重新安装 `previous`** → 事件 + 桌面通知 |
| P3.4 | **防循环**：`pinnedVersions` 持久化；同一版本不再被提供 |
| P3.5 | 面板/托盘：「立即重启以应用更新」入口；显示回退记录 |

**验收门禁（最关键）**：
- **注入一个「必崩」的壳版本**（启动即 panic）→ 壳启动 2 次后，内核自动回退，第 3 次启动是**已知可用版本**。
- 回退后 `pinnedVersions` 生效，**不再反复尝试**该坏版本。
- 全链路事件可回放（available → started → failed → rolled_back）。

**回滚**：策略参数可关（`shellUpdateAutoRollback=false`）→ 退化为纯手动。

**隔离保证（对应非目标 3c / 风险 K12）**：

| 维度 | 壳 | 内核 | 冲突 |
|---|---|---|---|
| 状态目录 | `~/.dsh/shell/` | `~/.dsh/supervisor/` | **无**（物理隔离） |
| 版本账本 | `update-journal.json`（壳） | `_selfUpdateExpectedVersion`（进程内） | **无**（不同命名空间） |
| 黑名单 | `pinnedVersions`（**只针对壳版本**） | 内核无同名机制、不受影响 | **无** |
| 安装执行器 | Tauri 官方 updater | `dist.runNpmInstall`（装 DSH） | **无**（不同工具链） |
| 触发来源 | npm CDN 上的壳清单（`shell-manifest.json`） | npm registry（内核子包）/ GitHub Release（源码） | **无**（不同清单、不同包名） |

---

### P4 — 签名与发布链路（桌面级硬性前提）

| 任务 | 内容 |
|---|---|
| P4.1 | 生成 **minisign 密钥对**（`tauri signer generate`）；**公钥**入 `tauri.conf.json`；**私钥**入壳仓 CI secret + **异地备份**（丢失 = 该批用户永久失去更新能力） |
| P4.2 | 壳仓 CI：`createUpdaterArtifacts: true` → 产出 **`.deb` + `.sig`**（**主**，D4）、**`.app.tar.gz` + `.sig`**、**NSIS `-setup.exe` + `.sig`**／`.msi` + `.sig`；**不再产出 AppImage** |
| P4.3 | **CI 配额优化（重要）**：现 `on.push.branches:[main]` 会**每次推 main 都构建三平台** → 改为 `tags: [v*]` + `workflow_dispatch` |
| **P4.4** | **产物发布到 npm**（新增，通道定案）：壳仓 CI 把更新产物 + `.sig` + `shell-manifest.json` 组装为 `@dsh-sup/shell-<os>-<arch>@<ver>` 并 `npm publish`（复用内核已有的发布凭据与流程）；清单 URL 指向 unpkg 直链。GitHub Release 仅保留作人工下载/备用 |
| P4.5 | 内核读取清单（npm CDN 上的 `shell-manifest.json`），取得 `version + url + signature` 供 P1 预取使用 |
| P4.6 | macOS **签名 + 公证**；Windows **代码签名**（D6 定案） |

**验收门禁**：
- CI 产出全部更新产物与 `.sig`；壳能用公钥验签通过。
- **产物已发布为 npm 包，且 unpkg 直链可下载**（`curl` 实测 200 + 验签通过）。
- **清单 URL 与签名在 CDN 上自洽**（清单里的 `signature` 能验过对应的产物文件）。
- 推 main **不再**触发三平台构建（配额验证）。
- macOS 未签名/未公证的产物**不得**进入发布流程（加门禁检查）。

**风险**：私钥丢失不可逆 → 备份策略须先落地再首次发布。

---

### P5 — 跨平台分发与构建矩阵（**D4：废弃 AppImage，标准 Linux 包；公开产品跨平台视角**）

> 📌 **完整矩阵见 `CROSS-PLATFORM-BUILD-AND-UPDATE.md`**（含实测的 glibc/webkit ABI 约束、GitHub runner 精确标签、三平台签名要求）。
> 本节只列执行项。

> **决策 D4（用户）**：Linux **废弃 AppImage**，按**标准 Linux 安装包机制**（deb，可选 rpm）分发。
> 详见 `RELEASE-AND-UPDATE-MECHANISM.md`（发布与更新机制总纲）。

> ⛔ **必修缺陷（实测确认，F1）**：当前 Linux 产物在**本机 Ubuntu 24.04 基座**构建，
> 二进制要求 **`GLIBC_2.39`** → **只能装 Ubuntu 24.04+**，把最主流的 **Ubuntu 22.04 LTS（2.35）**
> 与 **Debian 12（2.36）** 用户**全部排除**。
> **修复**：CI 基座改 **`ubuntu-22.04`**（已实测该基座提供 `libwebkit2gtk-4.1-0`），
> 并加 **glibc 上限门禁**（`ci/check-glibc.sh` + `test/glibc-gate-test.js`，已落地）。
>
> ✅ **已本地实证（2026-09-11）**：Rust 1.98.1 就绪，`cargo build --release` **成功（3m52s）**；
> 用 glibc 2.35 的 `ld.so` 实测加载 → **确认无法运行**；根因定位为 **Rust 预编译 std 的 `process` 模块
> 对 `pidfd_spawnp`/`pidfd_getpid` 的弱引用 + 构建基座 glibc 版本**（两行代码即可复现）。
> 详见 `CROSS-PLATFORM-BUILD-AND-UPDATE.md` §2.4–2.8。

| 平台 / 架构 | Runner | 目标形态 | 体积 | 更新能力 | 提权 |
|---|---|---|---|---|---|
| **Linux x64** | **`ubuntu-22.04`** | **`.deb`**（主）；rpm 可选（N2b） | **3.8MB** | 应用内自更新 ✓（`pkexec dpkg -i`） | **更新需一次密码** |
| Linux arm64 | `ubuntu-22.04-arm` | `.deb` | ~3.8MB | 同上 | 同上 |
| macOS arm64 | `macos-latest` | `.app.tar.gz` + `.dmg` | ~3.1MB | 应用内自更新 ✓ | 否 |
| **macOS x64** | **`macos-15-intel`** | `.app.tar.gz` + `.dmg` | ~3.5MB | 应用内自更新 ✓ | 否 |
| Windows x64 | `windows-latest` | NSIS `-setup.exe`（主，per-user）+ `.msi` | ~3.7MB | 应用内自更新 ✓（`passive`） | 否 |

> **修正**：此前假设「GitHub 无 Intel macOS runner」——实测 `macos-15-intel` **存在**，
> 故 darwin-x64 可**原生构建**（避开交叉编译的签名风险）。注意 `macos-14` 已弃用，勿用。

**D4 的收益（实测支撑）**：

| 指标 | AppImage（已废弃） | **deb（采用）** | 倍数 |
|---|---|---|---|
| 体积 | 77 MB | **3.8 MB** | **20×** |
| 更新耗时（unpkg@1.71MB/s） | 约 45 秒 | **约 2 秒** | **20×** |
| 自更新 | 免提权 | 需一次 pkexec 密码 | — |

> **契合现状**：已取证 `dpkg -S /usr/bin/dsh-supervisor-gui` → `dsh-supervisor: /usr/bin/dsh-supervisor-gui`，
> 即**当前生产本就是 deb 安装**。D4 让分发与现状一致，无需用户迁移形态。

**必须同步修正的不一致（已取证）**：
- 内核 `desktop/` 模板指向 `@HOME@/.local/bin/dsh-supervisor-gui`（**用户级**）
- 但 deb 实际安装到 `/usr/bin/dsh-supervisor-gui`（**系统级，root 所有**）→ **模板与实况不符**
- 结论：**以 deb 为标准**，修正内核 `desktop/` 模板与 autostart 的路径解析（支持系统级安装），
  并保留用户级回退（源码/开发形态）。

**任务**：

| # | 任务 |
|---|---|
| **P5.0** | **Linux 基座改 `ubuntu-22.04`**（F1 必修）：覆盖从「仅 24.04+」扩到「22.04+ / Debian 12+」；CI 加 `ci/check-glibc.sh` 断言 |
| P5.1 | 壳仓 `tauri.conf.json`：`targets` 由 `["deb","appimage","dmg","msi"]` → **`["deb","dmg","msi"]`**（可选加 `"rpm"`）；`createUpdaterArtifacts: true` |
| P5.2 | 壳仓 CI：**移除 AppImage**；构建矩阵改为 **6 个 job**（见 `CROSS-PLATFORM-BUILD-AND-UPDATE.md` §4.1）；收集 `.sig`；Windows NSIS per-user `passive` |
| **P5.3** | **macOS 签名 + 公证**（Developer ID + notarytool + stapler，**硬性**）；**Windows 代码签名** |
| **P5.3b** | **Windows WebView2** 策略确认（`downloadBootstrapper` 默认 / 离线 `embedBootstrapper`） |
| P5.4 | **内核 `desktop/` 模板 + autostart 路径解析修正**：以 deb 的 `/usr/bin` 为准，保留用户级回退 |
| P5.5 | 桌面集成：deb 内由 Tauri 自带 `.desktop` + 图标（**标准包应自带**，不依赖内核模板） |
| P5.6 | 文档/面板：Linux 更新需一次密码的说明；`pkexec` 不可用时的降级提示（`apt install ./…deb`） |

**验收**：
- **deb 为标准形态**：`dpkg -i` 安装后 `dpkg -S` 可查；桌面入口（.desktop/图标）由包自带。
- **自更新端到端可用**（Linux 走 pkexec 提权路径）。
- **无 pkexec/sudo 的环境** → 明确提示手动更新（`apt install ./x.deb`），**不静默失败**。
- **不含任何 AppImage 产物**（CI 与发布清单均无）。
- **glibc 上限门禁通过**（`ci/check-glibc.sh` 断言最高符号 ≤ 2.35）→ 产物可在 Ubuntu 22.04+ / Debian 12+ 运行。
- **macOS 产物已签名且已公证**（`spctl -a -vv` 通过）；Windows 产物已代码签名。
- 内核 `desktop/` 模板 / autostart 指向与实际安装路径**一致**（消除 `/usr/bin` vs `~/.local/bin` 冲突）。

---

### P6 — 版本配对（壳 ↔ 内核兼容区间）

**问题（现状 F14）**：壳装内核走 `core_plan` → **总是 latest**，不校验兼容区间 →
旧壳可能装出它不认识的新内核（一旦内核协议演进，如本次端口架构重构，旧壳可能无法正确拉起）。

#### ⛔ 硬约束（用户明确要求）：**不得破坏内核既有更新机制**

内核侧现有**三条**更新路径，全部保持原语义、**不因本方案改变**（已取证）：

| 路径 | 入口 | 现有语义 | 本方案是否改动 |
|---|---|---|---|
| ① 守卫自更新 | 面板「检查更新」→ `guardSelfUpdateStatus/Apply` | **全更新强制**：`latest > 当前` 即装，**无跳过、无降级**；安装后校验磁盘版本（`verified`），重启后按 `_selfUpdateExpectedVersion` 复核 | **不改** |
| ② 原生 DSH 更新 | `NativeManager.install/upgrade/uninstall` → 唯一 `_runInstall` → `dist.runNpmInstall` | 单通道；失败**自动回滚**；`installedVersion` 版本校验闭环 | **不改** |
| ③ 沙箱实例更新 | `InstanceManager` → `dist.runNpmInstall`（带 `--prefix`） | 每实例独立安装；升级/回滚闭环 | **不改** |
| ④ manifest 通道 | `dist/self-update.js`（`apply/fetchManifest`） | `selfUpdateManifestUrl` 默认 **null** → 当前**未启用**（D1 定案：仅作底层执行器保留） | **不改** |

**本方案与内核更新的接触面仅有一处**：`P1` 复用 `dist` 的**只读**能力（`fetchLatestVersion` / 镜像回退）
去查询**壳**的发布版本。**不复用** `runNpmInstall`（那是装 DSH 的），也不写任何内核版本状态。

#### 修正后的任务（避免破坏内核更新）

| 任务 | 内容 | 与原设计的差别 |
|---|---|---|
| P6.1 | 壳发布元数据**声明** `kernelMin` / `kernelRecommended`；内核**声明** `shellMin`（纯新增元数据） | 不变 |
| P6.2 | ~~`core.rs` 按壳声明选择版本~~ → **改为：壳只对「它自己安装内核的动作」做兼容性检查，并在不兼容时【先升级壳】，绝不允许把内核降级** | **重要修正**：原设计有缺陷 |
| P6.3 | 不兼容时**唯一允许的动作**是「把壳升到兼容版本」；**禁止**为兼容旧壳而把内核降级或 pinned（那会破坏路径①②③的强制更新语义） | 保持方向，补上禁止项 |
| P6.4 | 壳回退时**只回退壳自身**，**不连带回退内核**（内核有它自己的更新/回滚闭环，必须独立） | **重要修正**：原「整对回退」会越界改动内核状态 |

**为什么原 P6.2 / P6.4 是错的**：

1. **P6.2（按壳声明选版）** 会绕过内核的「全更新强制语义」——可能出现「内核已是最新，但旧壳把自己的区间套上去，反而装上旧版」，
   等于**用壳的策略推翻内核的更新策略**，正是用户担心的「破坏内核更新机制」。
2. **P6.4（整对回退）** 会让一个**壳的更新失败**去改动**内核的版本**——把两个独立、各自已验证的更新闭环耦合起来，
   使壳的故障可以污染内核状态，风险面被放大。

**正确的边界**：
- 壳**管好自己**的更新与回退（只影响壳的产物）。
- 内核**管好自己**的更新与回滚（现有三条路径原样保留）。
- 两者的**兼容性**通过「元数据声明 + 不兼容时优先升壳」来协商，**绝不通过互相降级**来解决。

---

### P7 — 纪律：逻辑下沉（长期）

**规则**：凡能放进面板 / 引导页 JS / 内核的逻辑，**绝不写进 Rust 二进制**。
每条壳 PR 必须回答：「这个改动是否真的必须在原生层？」

**目的**：降低壳更新频次 = 降低发布风险面。这是长期稳定性的第一来源。

---

## 4. 风险登记册

| # | 风险 | 影响 | 缓解 |
|---|---|---|---|
| K1 | minisign 私钥丢失 | **已发布用户永久无法更新** | 异地多份备份 + 双人托管 + 首次发布前演练恢复 |
| K2 | macOS `.app` 位于 root 目录（如 `/Applications`） | 自更新失败 | 引导安装到用户可写位置；不可写时 `selfUpdateCapable=false` 并提示 |
| K3 | ~~Tauri release 强制 HTTPS vs 回环 HTTP~~ | **已消除** | 修正后端点改公网 HTTPS，不再需要 insecure 开关 |
| K4 | ~~内核与壳的更新协议演进不同步~~ | **显著降低** | 壳不再解析内核的更新协议；仅保留 `health`/`status` 两个简单端点 |
| **K11** | **Tauri 原地安装不保留旧版本** | 新版本坏掉时**磁盘上无退路** | **更新前强制备份当前产物**（P2.3）+ 内核缓存（P1.1） |
| **K12** | **壳侧逻辑越界扰动内核更新** | 破坏内核「全更新强制」语义 / 壳故障污染内核版本 | **P6 硬约束**：内核四条更新路径原样保留；壳只回退壳自身；**禁止互相降级**；P1 只复用 `dist` **只读**能力 |
| **K13** | **npm CDN 可用性**（unpkg / jsdelivr 均为第三方） | 更新通道不可用 | ① 清单配置**多 CDN 回退**（unpkg 主 / jsdelivr 备）；② **内核本地缓存**兜底（已装用户可用缓存更新）；③ 失败进选择页（可【继续】）
| **K14** | **deb 自更新需提权**（pkexec 弹窗） | 用户拒绝 → 更新失败 | 视为正常失败路径 → 选择页【重试】【继续】；文档说明「需一次密码」 |
| **K15b** | **glibc 基座过新**（实测已发生：需 2.39） | 主流发行版装不上 | **CI 基座固定 `ubuntu-22.04`** + `ci/check-glibc.sh` 门禁 |
| **K16** | **webkit2gtk ABI 不匹配** | 运行失败 | 固定构建 4.1；`Depends` 显式声明；文档列最低发行版 |
| **K17** | **macOS 未签名/未公证** | Gatekeeper 拦截，**用户装不上** | Developer ID + notarytool + stapler（硬性，D6） |
| **K18** | **Windows 缺 WebView2** | 启动失败 | `downloadBootstrapper` 自动安装；提供离线包选项 |
| K5 | 回退循环（坏版本反复被提供） | 无限重启/反复降级 | `pinnedVersions` 黑名单 + `attempts` 上限 |
| K6 | ~~deb 用户无法自更新~~ | **已消除** | D4 后 Linux 标准形态即 deb，且 Tauri `install_deb` 支持自更新（pkexec） |
| K7 | 壳仓 CI 推 main 即全平台构建 | **配额消耗** | P4.3：改为 tag + 手动触发 |
| K8 | 本机无 Rust 工具链 | 本地无法验证壳改动（反馈环慢） | P0.6 安装工具链 |
| K9 | Windows SmartScreen / 杀软对 per-user 安装的告警 | 用户流失 | 代码签名（D6） |
| K10 | 壳更新期间断电/断网 | 半成品 | 由 Tauri 的原子安装语义 + 内核 journal 回退兜底 |

---

## 5. 非目标（明确不做）

1. **不做** `.deb` 上的应用内自替换（与 dpkg 冲突，行业共识：交给包管理器）。
2. **不做**无条件强制更新（一次坏发布即全体不可用）。
3. **不在壳内**实现自定义下载/校验/版本比较（全部交给官方插件 + 发布通道清单）。
3b. **不让内核当壳的更新源**（冷启动时内核可能不在 → 必须由壳直连公网）。
3c. **绝不改动内核既有更新机制**（用户明确要求）：守卫自更新（全更新强制语义）、原生 DSH 更新（`_runInstall` + 自动回滚）、
    沙箱实例更新（带 `--prefix`）、manifest 通道（当前未启用）——**四条路径全部原样保留**。
    壳与内核的兼容性只通过「元数据声明 + 不兼容时先升壳」协商，**禁止互相降级**（详见 P6）。
4. **不改**守卫所有权模型（systemd 唯一所有者；壳绝不直接 spawn 守卫）——维持既有架构契约。
5. **不做**去壳（D1 已定案保留原生壳）。
6. **不做**「只在本机发行版可用」的产物（F1 教训）：每个平台产物必须声明并验证其**最低支持版本**。
7. **不假设**单一 Linux 形态：发行版、架构、glibc、webkit ABI 都要覆盖（见 `CROSS-PLATFORM-BUILD-AND-UPDATE.md`）。

---

## 6. 前置条件（必须先解决，否则方案无法验证）

| # | 前置 | 状态 |
|---|---|---|
| A1 | **本机 Rust + Tauri 构建环境**（当前无 cargo/rustc） | 缺 → P0.6 |
| A2 | **minisign 密钥对 + 备份策略** | ✅ **已完成**（2026-09-11）：正式密钥已生成（`~/.tauri/`），本机备份 + 恢复演练通过 → 见 `release/runbooks/updater-signing-key.md` |
| A3 | macOS 开发者证书 + 公证 / Windows 代码签名 | ⏸ **暂缓**（用户定案 2026-09-11：暂无证书）。不阻塞构建与手动安装；**自动更新的完整性由 minisign 保障**，与本项无关 |
| A4 | 壳仓 CI secrets 配置 | ⏳ **待你在 GitHub 网页配置**（本机无 PAT，无法代做）：`TAURI_SIGNING_PRIVATE_KEY`、`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`（空值也必须设）、`NPM_TOKEN` |

---

## 7. 建议的执行顺序与首批范围

**第一批（立即可做，零架构风险，收益最高）**：

1. **P0.6** 本地 Rust/Tauri 工具链（否则后面全是盲改）
2. **P0.1** 壳落盘日志 —— 所有诊断的前提
3. **P0.2** 壳身份与 `attempt` 计数 —— 后续回退策略依赖它
4. **P0.3 / P0.4** panic 收敛 + 配置健壮化
5. **P4.3** 壳仓 CI 配额优化（一行改动，立即省额度）

**第二批**：P1（内核权威）+ P2（壳接入）并行 → P3 闭环。

**第三批**：P4 签名 → P5 分发形态 → P6 版本配对 → P7 持续纪律。

---

## 8. 待你确认的开放项

| # | 开放项 | 我的建议 |
|---|---|---|
| O1 | macOS 安装位置策略（`~/Applications` vs `/Applications`） | **`~/Applications`**（免提权，自更新可行） |
| O2 | 壳更新是否需要用户可见的进度 UI | **需要**（静默更新会让用户对「重启」感到突兀） |
| ~~O3~~ | ~~minisign 私钥托管方式~~ | **已定案（用户 2026-09-11）：本机备份即可，不做异地**；已完成本机备份 + 恢复演练 |
| O4 | 壳仓 Linux 构建是否也改本地（配额） | **暂不改**：壳构建低频，且产物需挂 GitHub Release；仅做 P4.3 的触发优化 |
| O5 | 是否立刻开始第一批 | 建议**是**（P0 是全套方案的地基） |
| ~~O6~~ | ~~更新失败时是否静默放行~~ | **已定案（用户确认）：显示选择页【重试】【继续】**（见 §2.7） |
| ~~O7~~ | ~~是否允许壳侧影响内核更新~~ | **已定案（用户明确要求）：绝不**（见非目标 3c、P6 硬约束、风险 K12） |
| **N1** | **更新通道**（实测驱动） | A GitHub 直连 / B 第三方代理 / **C npm CDN** / D 自建 | **C**：实测 1.71MB/s（GitHub 直连仅 15–28KB/s）、零新增基础设施、无供应链风险 |
| ~~N2~~ | ~~Linux 更新主形态~~ | **已定案（D4）：deb 标准包，废弃 AppImage** | 生效；仅余「是否加 rpm」为开放项（见 N2b） |
| **N3** | 内核本地缓存加速 | 需要 / 不需要 | **需要**（热路径 45 秒 → 秒级；且是离线降级路径） |
| **N2b** | Linux 是否**同时**发 rpm | 只发 deb / deb+rpm | **先只发 deb**（覆盖主流、减少 CI 与测试面）；视用户需求再加 |
| **N4** | 是否发布 **apt/yum 仓库**（VS Code 模式） | 现在做 / 后续 | **后续**：可加分（系统包管理器自动更新 + 依赖解析），但需仓库托管 + GPG 密钥管理 |

