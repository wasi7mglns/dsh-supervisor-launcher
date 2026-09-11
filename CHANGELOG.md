# Changelog（桌面壳）

本文件记录桌面壳（`dsh-supervisor-gui`，公开仓 `wasi7mglns/dsh-supervisor-launcher`）的重要变更。

## [未发布]

（下一版本待记）

## [1.0.5]（2026-09-11）

### 修复（架构级）：全壳审计 —— 「无界阻塞 + 被 await」同一模式另有 6 处

用户要求顺着环境探测的根因，深度排查整个桌面壳是否还有同类（逻辑/架构）缺陷。
把根因抽象为可检索的模式 —— **① 无界阻塞调用 ② 被命令 await ③ 失败被静默吞掉** ——
逐类扫描全部源码后，确认同一模式**另有 6 处**，其中 2 处在引导关键路径上。

#### 新增公共设施 `src/bounded.rs`

审计发现「无界执行」是**分散潜伏**的：service.rs / main.rs / node.rs 各写各的
`.output()`。故提取公共有界执行器，**所有**外部命令一律经它执行：

- 输出重定向到**临时文件**而非管道（管道不读取会在填满 64KB 缓冲后死锁）；
- 轮询 `try_wait` + 超时 kill（std 无跨平台 wait-with-timeout）；
- Windows 加 `CREATE_NO_WINDOW`（GUI 调控制台程序不弹黑框）；
- 自带单测：成功路径 / 超时必须被 kill / 不存在的二进制返回 Err 而非 panic。

#### 六处同源缺陷与修复

**① 服务管理器命令全部无界（致命 —— P0 已在关键路径上）**

`service.rs` 的 `systemctl --user daemon-reload/enable`、`loginctl enable-linger`、
`launchctl bootstrap`、`schtasks /Query /Create`，以及 `main.rs` 的
`start_guard_service` / `stop_guard_service` / `taskkill` —— **全部**用 `.output()`。

而它们在**建立服务定义 → 启动守卫**这条唯一通道上。systemd 在 dbus 会话异常、
systemd 无响应时会长时间挂起 → `ensure_guard` 永不返回 → 引导页永久停在
「正在启动守卫…」。**与环境探测卡死是同一根因，只是发生在下一步。**

**② `guard_start` 无外层超时（致命）**

```rust
// 旧：spawn_blocking(ensure_guard).await   ← 无超时
```

而前端调用它时是**裸 invoke**（无 `withTimeout`）—— 两侧都没有界。
现：Rust 侧加 `tokio::time::timeout`（180 秒）+ 前端加 `GUARD_START_BUDGET_MS`（200 秒）。

**③ `guard_start` 全程静默（UX 缺陷，会被误判为卡死）**

`ensure_guard` 最长可耗时约 2 分钟（服务管理器 30s + 兜底 spawn 后 60s），
而这段时间前端只有一句静态的「正在启动守卫…」。
**静默等待与卡死无法区分** —— 用户会误判并强杀进程，从而错失本可成功的启动。
现每个阶段经 `guard_progress` 事件上报，前端实时显示。

**④ `core_status` 是同步命令且在**主线程**执行二进制**

`fn core_status`（非 async）→ Tauri 在**主线程**执行 → 内部 `locate_core` 会
**逐个候选执行内核二进制**（每个 10 秒上限）取版本做仲裁，且随后又对选中项
再执行一次。候选一多（PATH + npm 目录 + 资源目录）即把主线程占住数十秒 ——
界面完全无响应。现改为 async + 阻塞线程池，并让 `locate_core` 一并返回版本，
消除重复执行。

**⑤ 托盘菜单在 UI 线程做网络 I/O**

`on_menu_event` 由 UI 线程派发，而分支里直接调 `post_local`（最长阻塞 60 秒）。
守卫挂起或端口无响应时，点击「启动/停止/重启」会**把整个界面冻结 60 秒** ——
用户看到的是「点了没反应」。同样地，「退出」在 UI 线程做完整退出握手（最坏约 70 秒），
会被感知为「程序关不掉」而强杀，从而**跳过退出握手、留下未停的 DSH**。
现两者均派发到独立线程。

**⑥ 内核候选定位未过滤可能阻塞的路径**

`locate_core_candidates` 的 `is_file()` / `canonicalize()` 会触网 —— 在断开的映射盘
或 UNC 上可能阻塞数十秒，而该函数在**内核定位的关键路径**上（引导页每步都用到）。
已在 `env.rs` 修过同类问题（PATH 探测），但此处漏修。现复用 `is_local_fixed_dir`。

#### 另两处隐患（非阻塞类）

**⑦ 互斥锁中毒后全部命令永久 panic**

6 处 `.lock().unwrap()`：任何线程在持锁期间 panic → 锁**永久中毒** →
此后**所有**命令在加锁处 panic。用户看到的是「重启也没用、功能永久失效」。
锁内是普通状态快照（不承载跨字段不变式），中毒后仍可用，故改为
`.unwrap_or_else(|e| e.into_inner())`。原则：**一次 panic 不应让功能不可恢复地失效。**

**⑧ `now_iso()` 为取时间执行外部进程，且 Windows 返回空串**

Unix 上 spawn `date`（无界、且系统可能没有该命令）；**Windows 分支直接返回空串**，
使 `installedAt` 在 Windows 丢失、跨平台行为不一致。
现改为纯 std 计算（Howard Hinnant civil-from-days 算法），三平台一致、无副作用。

#### 系统性门禁（本次审计最重要的产出）

逐个修完还不够 —— 缺陷之所以能**分散潜伏**，正是因为缺少系统性检查。
故新增 8 条架构门禁，其中 B32 是**全量源码扫描**：

| 断言 | 内容 |
|---|---|
| **B32** | **任何** Rust 源码都不得出现裸 `.output()` / `.status()`（豁免执行器自身） |
| B33 | `guard_start` 必须两侧都有超时（Rust tokio + 前端 withTimeout） |
| B34 | 必须上报 `guard_progress` 且前端监听（静默等待 ≠ 卡死） |
| B35 | `core_status` 必须 async 且复用版本（阻塞工作不得留在主线程） |
| B36 | 托盘分支必须走 `spawn_local_post`（不得在 UI 线程做网络 I/O） |
| B37 | 内核候选定位必须过滤非本地盘 |
| B38 | 时间戳不得执行外部进程 |
| B39 | 互斥锁不得用裸 unwrap（中毒恢复） |

#### 验证

- 壳测试 **50 项全通过**（bootstrap_flow 38 + updater_artifacts 6 + 单元 6）；
- `--env-plan` 50ms 命中；`--service-plan --service-apply` 在干净 HOME 下真实建立 unit；
- 全量源码扫描确认**零**裸 `.output()`/`.status()` 残留。

### 修复（架构级）：环境探测被无界系统调用卡死 —— 1.0.3/1.0.4 两次修复都未触及根因

用户真机反馈：1.0.4 **仍然**卡在「检测环境」，诊断信息为
`node=unknown | shell=unknown | error=环境检测超时（探针无响应…）`，
并指出「完全没有自动适配最佳镜像源」。要求从**底层架构**找根因、从架构层解决。

#### 为什么前两次修复都没解决

前两次都在**加超时**（Rust 侧 5 秒/候选、20 秒全局；前端 45 秒）。
但根因有两层，加超时对两层都无效：

**根因一：探测内含无法被自身预算约束的阻塞系统调用。**

原实现的 5 秒/20 秒预算，只在**候选之间**、以及 `spawn` **返回之后**才被检查：

```rust
for dir in split_paths(PATH) {
    if started.elapsed() >= NODE_PROBE_TOTAL_BUDGET { break; }   // ← 只在循环头检查
    let cand = dir.join(node_exe());
    if is_usable_candidate(&cand) { ... }   // is_file()/metadata() —— 无上限
    if let Some(v) = node_version(&cand) { ... }  // spawn —— 无上限
}
```

- `is_file()` / `metadata()` 底层 `GetFileAttributesW`：断开的网络盘、部分过滤驱动下可阻塞数十秒；
- `Command::spawn()` 底层 `CreateProcessW`：应用执行别名、网络路径、杀软实时扫描下同样可长时间不返回。

**单个阻塞调用即可击穿全部预算 —— 那不是真正的上限，只是提示。**

**根因二：探测任务被 Tauri 命令 `await`，于是「探测阻塞」等价于「命令永不返回」。**

前端 45 秒超时只是**停止等待**，并不解除挂起：Rust 侧那条线程永久悬挂，
每次重试再添一条，而每次结果都一样（仍卡在同一个候选）—— 用户被**永久挡住**。
这正好解释了诊断串里 `node=unknown` **且** `shell=unknown`：两条命令都没回来。

#### 架构修复（分层）

**① 新增 `src/nodeprobe.rs` —— 有界探测运行时（治本）**

```
命令线程 ──recv_timeout(budget)──> 探测线程（分离）
                                      └─ 可能永久阻塞，但**与命令返回时间无关**
```

- `std::sync::mpsc` + `recv_timeout`：**命令的返回时间与任何系统调用无关**。
  这是唯一能给 `CreateProcessW` / `GetFileAttributesW` 加界的手段；
- 状态机 `Idle / Running / Done`，**只允许一个在飞探测** —— 重试不再堆积线程；
- `STALE_AFTER`（90 秒）：阻塞若**永久**挂起，允许重启探测，
  避免「探测能力被永久剥夺」（代价受用户重试次数限制）；
- 结果缓存 + `invalidate()`：装完 Node 后必须能重新发现；
- `current_stuck()`：**记录当前卡在哪个候选、已多久** ——
  环境特有根因无法靠读代码确定，这是唯一可靠的定位手段。

**② 候选来源改为「廉价优先」**

顺序从「猜 PATH」改为：

```
① runtime.json 记录过的路径   ← 一次本地读（我们**装过**，却从未回读）
② 已知安装落点                ← 少数几次本地 stat（含 nvm/volta/fnm/scoop 等版本管理器，取最高版本）
③ PATH 扫描（最后）           ← 执行外部二进制是最贵也最危险的探测方式
```

这同时修掉一个隐藏缺陷：壳安装 Node 后写了 `runtime.json`，但**从未回读** ——
于是每次启动仍去猜 PATH，也就可能出现「装了却找不到」。

**③ PATH 扫描有界 + 跳过可能阻塞的路径**

- 加 `PATH_SCAN_BUDGET`（10 秒快速失败）；
- Windows 上跳过**非固定盘与 UNC**（`GetDriveTypeW` 判定，自身不触网）——
  从根上避开「断开的映射盘导致 `GetFileAttributesW` 挂起」这一整类问题。

**④ `node_status` 改为纯本地 + 可轮询（治本的另一半）**

- 等待预算 ≈900ms，未完成即返回 `probing=true`，由引导页轮询；
- **移除其中的网络 I/O**：原实现顺带触网查最新 LTS，使「本地环境判定」被网络质量左右 ——
  而这两件事在因果上毫无关系。网络侧改由独立的 `node_latest` 提供。

**⑤ `setup()` 不再同步探测**

`setup` 在**窗口创建之前**运行，同步探测挂起会**连窗口都推迟出现**，
使失败现象表现为「启动慢/无窗口」，与真正病因相距极远。改为仅触发探测（分离线程）。

**⑥ 镜像适配显式可见化（直接回应「完全没有自动适配」）**

镜像探测此前藏在下载内部，用户无从得知「有没有选、选了谁」——
于是「没有自动适配」成为**无法证伪的观感**。现独立成一步：

```
正在测速并选择最佳镜像源…
镜像已选：npmmirror.com（311ms）· 目标 Node v26.8.2
```

并在诊断串中输出**逐源延迟**（`mirror_probes=`）。

**⑦ 环境失败不再是死胡同**

新增「跳过检测，直接安装 Node」出口 —— 仅在环境超时后出现。
存在理由：**下载 Node 不需要本机已有 Node**，故「检测不出来」不应该挡住用户。
另修正 `fail()`：不再把「超时」当网络问题（此前会误导性展开镜像设置）。

**⑧ 新增无头自检 `--env-plan`**

```
$ dsh-supervisor-gui --env-plan
平台          = linux
候选 13 个（记录 0 / 已知 5 / PATH 8）
完成          = true
耗时          = 50 ms
node          = v26.7.0 @ /usr/local/bin/node
逐候选追踪:
  [已知] /usr/local/bin/node 50 ms ok=true v26.7.0
```

卡住时也能看到「卡在谁、多久」——**Windows 上若再次出问题，请把该命令输出发我**。

#### 本机实测

```
--env-plan     完成=true  耗时=50ms  经「已知落点」命中（未走 PATH）
--node-plan    镜像选择与 10 个源的延迟全部可见；跨源取最高版本后选中 npmmirror
```

#### 双仓隔离收尾（本次顺带修正）

壳仓从内核仓目录移出后，`bootstrap_flow.rs` 中两条**跨仓断言**（经 `../../` 读取内核的
`bin/dsh-supervisor` 与 `src/platform/config.js`）自然失效 —— 这正是隔离应当暴露的问题。
等价覆盖已迁至**内核仓** `test/package-root-test.js`（P1/P2 系列）：
**断言应与它验证的代码同仓**，而不是靠目录布局巧合成立。

#### 回归防护
`bootstrap_flow.rs` 增至 **30 断言**，新增 B26–B31：
- B26 必须有界探测运行时（`recv_timeout` + 分离线程 + 陈旧判定 + 缓存失效 + 追踪）；
- B27 `node_status` 必须可轮询且纯本地（回传 `probing`/`stuck`/`trace`，预算短，网络分离）；
- B28 PATH 扫描必须有界且过滤非固定盘；
- B29 引导页必须轮询且提供跳检测出口；镜像选择必须显式可见化；
- B30 `setup` 不得同步探测（否则推迟窗口创建）；
- B31 诊断串必须含 `env_trace`/`env_stuck`/`env_candidates`/`mirror_probes`。

## [1.0.4]（2026-09-11）

### 新增：守卫服务定义自检入口（P0 修复的可诊断性）

P0（壳自持服务定义）是本版最关键的修复，但若它在某台机器上失败，用户会卡在「守卫就绪」
却**无从自查** —— GUI 进不去、日志分散、也没有命令行入口。

故新增两个无 GUI 自检入口（任何平台可用）：

```
dsh-supervisor-gui --service-plan                # 只报告，不写盘
dsh-supervisor-gui --service-plan --service-apply # 实际建立服务定义
```

输出服务定义路径、是否现存、守卫可执行文件定位结果；`--service-apply` 时实际建立并报告结果。

另新增 `DSH_GUARD_BIN` 环境变量覆盖守卫路径 —— 用于①自动定位失败的机器做诊断 ②测试隔离 HOME。

**功能验证**（干净 HOME + 真实守卫路径）：

```
平台          = linux
服务定义路径  = ~/.config/systemd/user/dsh-supervisor.service
现存          = 否
守卫可执行    = /home/bowen/.npm-global/lib/node_modules/@dsh-sup/dsh-core-linux-x64/bin/dsh-supervisor
守卫存在      = 是

建立结果      = 已建立并启用 ~/.config/systemd/user/dsh-supervisor.service
建立后现存    = 是
```

生成的 unit 正文（`ExecStart` 指向真实守卫路径）：

```ini
[Unit]
Description=dsh-supervisor - DSH lifecycle guard
After=network.target
StartLimitIntervalSec=600
StartLimitBurst=3

[Service]
Type=simple
ExecStart=/home/bowen/.npm-global/lib/node_modules/@dsh-sup/dsh-core-linux-x64/bin/dsh-supervisor daemon
Restart=always
RestartSec=5
KillMode=process

[Install]
WantedBy=default.target
```

幂等性验证：第二次执行返回「已存在」，不重复写入。
`--service-plan`（不带 `--service-apply`）**不写盘**（已断言）。

**这同时修复了一个结构性问题**：`locate_core_candidates` 原先要求 `AppHandle`，
导致无 GUI 场景无法复用同一套内核定位逻辑。现改为 `Option<PathBuf>` 资源目录参数，
CLI 路径传 `None` —— 保证「自检」与「运行时」走**同一套路径推导**，避免自检通过但运行时找不到。

回归防护：B24（自检入口存在）、B25（无 AppHandle 可定位）。

本次为**架构级修复 + 镜像适配**，按用户要求逐项确认后才构建。

### 新功能：壳内镜像源适配（三条下载链路全覆盖，用户要求）

#### 问题（用户指正的核心）

> 「你根本在安装完壳之后去做的所有的动作，它是没有镜像源的，它是没有内核的。
>  你要理解整个工程的逻辑，在初始安装完壳的那一瞬的时候，里面是没有内核的。」

**装机那一刻机器上没有内核**（内核是随后由壳自己安装的），因此壳的每一处下载都**不能**
依赖内核的 `registry.json`。审计确认改造前壳的三条下载链路几乎没有镜像适配：

| # | 链路 | 改造前 |
|---|---|---|
| ① | Node 运行时（index.json + 安装包 + SHASUMS） | 2 源硬编码，**官方优先、仅报错才回退**，无测速/配置/UI |
| ② | 内核 npm 包（元数据 + `npm install -g`） | 4 源但**串行先到**，`registry.json` 缺失时用内建默认 |
| ③ | 壳自更新（清单 + 安装包） | 2 源**编译期写死**于 tauri.conf.json |

关键后果：`nodejs.org` 在受限网络下常常**可达但极慢**，于是永远不会回退镜像 ——
而安装包有 30–90 MB（macOS `.pkg` 实测 89.4 MB），用户要等到超时才失败。
（注：上一轮把下载超时从 60 秒放宽到 15 分钟后，该缺陷的代价从 1 分钟变成 15 分钟。）

#### 实测支撑（决定了实现方式）

| Node 镜像 | index | 版本新鲜度 | SHASUMS | .pkg |
|---|---|---|---|---|
| npm 官方 | ✅ | v24.21.0 | ✅ | ✅ |
| npmmirror | ✅ | v24.21.0 | ✅ | ✅ |
| 华为云 | ✅ | v24.21.0 | ✅ | ✅ |
| 腾讯云 | ✅ | **v24.20.0（滞后一版）** | ✅ | ✅ |

→ 腾讯云滞后一版，故必须「**跨全部源取最高版本**」；若「首个成功即采用」会静默装到旧版。

#### 实现

**新增 `src/mirror.rs`（壳自持镜像模块）**：
- `NODE_PRESETS` / `NPM_PRESETS` / `SHELL_PRESETS` 三组预设（全部实测可用）；
- `~/.dsh/shell/mirrors.json` 壳自持配置（装机即可用，**不依赖内核**）；
- `probe_all()` 用 `std::thread::scope` **并行**探测（零新增依赖）；
- `export_to_kernel()` 把 npm 偏好导出为内核 `registry.json`，让内核**继承**同一份选择
  （若内核已进入 `manual` 模式则不覆盖——尊重用户在面板里的显式选择）；
- 测速缓存 TTL 30 分钟（与内核 `selectRegistry` 一致）。

**① Node 下载（`node.rs` 重写）**：并行探测全部镜像 → 跨源取**最高 LTS** →
在提供该版本的源中选**延迟最低**者下载 → SHASUMS256 强校验，校验失败换源重试。

**② 内核 npm（`core.rs` 重写）**：`registry_origins()` 优先级改为
「内核 manual > **壳自持配置** > 内核 auto 列表 > 内建默认」；
`latest_version()` 由串行先到改为**并行 + 跨源取最高**。

**③ 壳自更新（`main.rs`）**：用 `UpdaterBuilder::endpoints()` **运行时覆盖**编译期端点，
使更新源可在不重新编译的前提下切换。

**引导页镜像自助入口**（新增 `mirror_status` / `mirror_set` 命令 + UI）：
- 壳装机时面板（`RegistryCard`）不可用，用户若遇镜像不可达将**没有任何出口**——这是可用性缺口；
- 故引导页在**失败态**提供镜像设置：显示各镜像实测延迟、可手填 Node/内核镜像、保存后自动重试；
- **默认隐藏**（不干扰普通用户），仅当失败信息含网络/镜像/超时/下载等关键词时自动展开。

#### 实测验证

干净 HOME 下运行 `--node-plan`（等价首启）：

```
mirror_selected=https://mirrors.huaweicloud.com/nodejs   ← 自动选中最低延迟
mirror_probe=https://mirrors.huaweicloud.com/nodejs      ok=true latency_ms=276
mirror_probe=https://npmmirror.com/mirrors/node          ok=true latency_ms=278
mirror_probe=https://mirrors.cloud.tencent.com/nodejs-release ok=true latency_ms=356
mirror_probe=https://nodejs.org/dist                     ok=true latency_ms=1458  ← 最慢
```

生成文件：`~/.dsh/shell/mirrors.json`（壳自持）+ `~/.dsh/supervisor/registry.json`（导出给内核）。
两次运行分别选中 npmmirror 与华为云 —— 证明是**真实测速**而非写死。

#### 回归防护
`tests/bootstrap_flow.rs` 增至 **21 断言**，新增：
- B18 壳必须自持镜像适配（三组预设 + 并行探测 + 缓存 + 导出内核 + 保护 manual）；
- B19 三条下载链路都必须接入镜像（不能只做一处）；
- B20 引导页必须有失败态镜像自助入口，且**默认隐藏**；
- B21 不得再出现「并发尝试」这类与实现不符的失真注释。

### 修复：守卫服务定义从未被建立 —— 全新机器上流程必然断裂（架构级，用户质疑驱动）

#### 问题（本次审计最严重的发现）

**旧设计的死锁**：壳只**启动**服务（`systemctl --user start` / `launchctl kickstart` /
`schtasks /Run`），把「服务定义的建立」留给内核的「所有者」语义。该设计在**首次安装**场景下必然失败：

1. 首次启动时**唯一的在场组件是壳** —— 内核此时可能尚未安装；
2. 内核的 `install` 子命令**不会被任何环节自动调用**（壳只执行 `npm install -g`）；
3. 且 npm 发行包**不含** `systemd/`、`desktop/` 模板（发布 `files` 字段未包含），
   即便调用 `install` 也会打印「跳过系统服务部署」后直接返回；
4. 于是服务定义从未建立 → 壳的 start 必然失败 → 引导卡在「守卫就绪」。

**实测证据**：
- 已发布 npm 包内 `package/systemd/` 与 `package/desktop/` 均为 **0 个文件**，无 `postinstall`；
- 在干净 HOME 下实跑内核 `install`：打印「未找到 systemd 模板…跳过系统服务部署」后返回，
  之后 `~/.config/systemd/user/` 为空；
- 壳仓**全部历史**中：写 systemd unit（`WantedBy`/`ExecStart`）**0 个提交**、写 LaunchAgent
  （`RunAtLoad`）**0 个提交**、写 schtasks（`/Create`）**0 个提交** → **不是回归，是从未通过**。
  本机能跑只是因为曾从**源码仓**手工跑过一次 `install`。

**Windows 另有一层错位**：`DSH-Supervisor` 计划任务指向的是 **GUI 壳**，而壳的
`start_guard_service()` 执行 `schtasks /Run /TN DSH-Supervisor` —— 于是「启动守卫」实际只是
再开一次壳（被单实例插件折回焦点），**守卫永远不会被启动**。

#### 修复

**新增 `src/service.rs`**：壳自持三平台服务定义（幂等，已存在则跳过）。
理由是结构性的 —— 壳是首启时唯一在场的组件，服务定义必须在任何东西启动守卫之前存在。

| 平台 | 建立方式 | 说明 |
|---|---|---|
| Linux | 写 `~/.config/systemd/user/dsh-supervisor.service` | **模板内嵌**（不再依赖外部文件）→ `daemon-reload` + `enable` + `enable-linger` |
| macOS | 写 `~/Library/LaunchAgents/com.dsh.supervisor.plist` | `RunAtLoad` + `KeepAlive` → `launchctl bootstrap` |
| Windows | 创建计划任务 `DSH-Supervisor` | 指向**守卫守护进程**；用包装 `.cmd` 规避 `/TR` 引号转义地狱 |

**`ensure_guard` 重写为三段**：建立定义 → 请求服务管理器启动（等 30s）→
**spawn 兜底**（服务管理器不可用时直接拉起守护进程，再等 60s）。

spawn 兜底需**放宽**原设计约束「壳绝不直接 spawn 守卫」：该约束的理由是避免产生游离于
服务管理器的第二实例，但它不能凌驾于**可用性**之上 —— 容器、无 user systemd session、
`launchctl` 被策略拦截、`schtasks` 被组策略禁止等场景下服务管理器根本无法使用，
无兜底则用户被永久挡在门外。第二实例风险由「spawn 前已确认端口不存活 + 以端口就绪为唯一成功判据」规避。

**Windows 任务名职责分离**（内核侧 `autostart.js` 同步修改）：

```
DSH-Supervisor          -> 守卫守护进程（壳建立；壳的 /Run 指向它）
DSH-Supervisor-GUI      -> 登录时打开桌面壳（面板 autostart 开关管理）
DSH-Supervisor-Watchdog -> 每 5 分钟保活（崩溃自拉）
```

关闭 autostart 时**不删除**守卫任务（它是服务定义），只 `/DISABLE` 其开机自启语义。

### 修复：macOS 上「运行环境」自动安装必然失败（格式不匹配）
- **根因**：`platform_file()` 下载 `node-v<ver>-darwin-<arch>.tar.gz`（tarball），
  却交给 `installer -pkg` 执行 —— 格式不匹配，必然失败。
- **修复**：改用官方 **`.pkg`**（`node-v<ver>.pkg`，实测通用包，arm64/x64 通用）。
  已验证官方可达（HTTP 200）且 `SHASUMS256.txt` 含其条目。
  （注：npmmirror 镜像的 SHASUMS 不含 `.pkg` 条目，故 macOS 实际依赖官方源；官方为主源，可接受。）

### 修复：Node 安装包下载超时过小（30-90 MB 却只给 60 秒）
- `ureq` 的 `timeout()` 覆盖**整次调用**（含响应体读取），而安装包体积：
  Linux 30.4 MB / Windows 31.7 MB / **macOS .pkg 89.4 MB**。
- 原值 60 秒在网络稍慢时必然超时，表现为「看似网络问题」实为超时配置过小。
- 修复：提到 **15 分钟**。

### 修复：Node 最低门槛被计算却从未生效
- 后端一直回传 `minOk`（DSH 要求 Node >= v22.12），但**前端从未使用** ——
  用户装了旧版 Node（如 v18）时流程照常放行，直到内核真正启动才失败，现象离根因很远。
- 修复：前端消费 `minOk`，不达标即触发升级；并回传 `minRequired` 供提示显示（避免前端硬编码漂移）。

### 修复：npm 发行态的包根解析 off-by-one
- `bin/dsh-supervisor` 会被 esbuild 打成 `core.cjs` 的 178 字节 launcher `require` 执行，
  此时 `__dirname` = **包根**（core.cjs 旁），而旧实现硬写 `path.join(__dirname, "..")` →
  指向**包外**：systemd/desktop 模板路径错位、`BIN_PATH` 指向不存在的文件。
  （干净 HOME 实测：`~/.local/bin/dsh-supervisor -> <pkg>/dsh-supervisor`，该文件不存在。）
- 修复：从 `__dirname` 逐级向上找含 `package.json` 的目录作为包根，两种形态均正确；
  `bin` 目标改为 `path.join(ROOT, "bin", "dsh-supervisor")`。

### 修复：内核 --version 探测无界（与「检测环境卡死」同类）
- `core::installed_version()` 用 `Command::output()` **无限阻塞**，且 `locate_core` 会对
  **每个候选**都调用一次 → 任一候选不可执行（损坏 shim / 被安全软件拦截 / 架构不符）即永久卡住。
- 修复：复用既有的有界执行器（10 秒上限）。

### 修复：Windows .cmd 垫片导致 --prefix 丢失
- npm 全局垫片位于 `%APPDATA%\npm\dsh-supervisor.cmd`，路径**不含 `node_modules` 段**，
  故 `global_prefix_for` 返回 None → `install_version` 丢失 `--prefix`，
  可能装到 npm 默认前缀而非内核当前所在前缀（旧内核遮蔽新内核）。
- 修复：① `global_prefix_for` 增加「父目录含 `node_modules` 即前缀」的兜底；
  ② `locate_core_candidates` 优先加入包内真实脚本路径，使版本读取与前缀推导都正常。

### 回归防护
`tests/bootstrap_flow.rs` 增至 **17 断言**，新增：
- B13 壳必须自持三平台服务定义（且模板内嵌）+ spawn 兜底；
- B14 macOS 安装格式必须与 `installer -pkg` 匹配（`.pkg`）；
- B15 下载超时必须足够大（禁止 60 秒）；
- B16 前端必须消费 Node 最低门槛；
- B17 包根解析必须形态无关。


### 修复：卡在「检测环境」不动（Windows 真机，1.0.3 仍复现）

**根因（两层叠加）**：

1. **Windows 应用执行别名存根**：Windows 的 PATH 默认含
   `%LOCALAPPDATA%\Microsoft\WindowsApps`，其中的 `node.exe` 是**别名存根**（重解析点，
   指向 Microsoft Store），并非真实 Node。`env::find_in_path` 仅用 `is_file()` 判定即选中它。
2. **探测无超时**：`env::node_version` 用 `Command::output()` **无限阻塞** —— 执行该存根会尝试
   唤起 Store 并永不返回，引导页从此永久停在「检测系统环境…」。
   另：`node_status` 是**同步** Tauri 命令，由主线程执行，探测变慢时连 UI 一起拖住。

**修复（四层防护）**：
- `probe_system_node` **遍历全部候选**而非取第一个（PATH 靠前的坏候选不再掩盖后面可用的 Node）；
- 过滤 Windows 别名存根（`\WindowsApps\`）与 0 字节文件；
- 有界执行：单候选 5 秒、整个 PATH 扫描 20 秒硬上限（超时即 kill，视为不可用）；
- `node_status` 改为 async + `spawn_blocking`，不再占主线程；
- 前端 `stepEnv` 补 `withTimeout`（45 秒）——此前只给桌面/内核步骤加了超时，**漏了第一步**，
  而第一步恰恰最容易卡。

### 修复：托盘右键不可用（Windows 真机）
- `on_tray_icon_event` 原先匹配 `Click { .. }`（**任意键、任意状态**）→ **右键**也会执行
  `show_main()`，把刚要弹出的右键菜单顶掉/抢走焦点。
- 同时 `show_menu_on_left_click(true)` 让左键也弹菜单，与「左键显示窗口」的预期冲突。
- 修复：左键抬起才显示窗口（`MouseButton::Left` + `MouseButtonState::Up`），
  右键交系统弹菜单（`show_menu_on_left_click(false)`）。
  注：上游文档明确 Linux 不支持该开关（菜单由桌面环境决定），属平台限制。

### 修复：Windows 窗口四周有「隐形框框」
- **根因**：Tauri 的 `shadow` 默认 `true`，官方文档明确写明：Windows 上 `true` 会让**无边框**
  窗口多出 **1px 白色边框**（Win11 还会加圆角）。我们的窗口是 `decorations:false`，正中此条。
- 修复：新增 **平台配置** `tauri.windows.conf.json`（Tauri 自动按平台合并），仅覆盖 `shadow:false`。
  已验证合并语义为 RFC 7386 JSON Merge Patch（对象递归、数组替换），
  故 `security.csp`、`withGlobalTauri`、更新端点等全部保留，仅 `shadow` 改变。
  未在 `tauri.conf.json` 直接改是因其对 Linux/macOS 同样生效，而那两个平台的 shadow 语义不同。

### 语义统一：步骤名与产品概念对齐

用户明确指出「桌面更新」是错误表达。步骤条现为：

```
检测环境 → 运行环境 → 桌面版本 → 内核版本 → 守卫就绪 → 进入控制面板
```

- 「桌面更新」→「**桌面版本**」（该步是"检查/对齐桌面版本"，不是"更新"这一动作）；
- 「进入面板」→「**进入控制面板**」；
- 其余文案同步（跳过按钮、检查中/超时/失败提示、下载进度、选择页标题、托盘菜单「显示控制面板」）。

### 回归防护
`tests/bootstrap_flow.rs` 增至 **12 断言**，新增：
- B9 步骤标签必须与要求语义逐字一致（防文案被改回）；
- B10 托盘必须区分左右键；
- B11 Windows 配置必须 `shadow:false` 且与基础配置**除 shadow 外完全一致**（防两处漂移）；
- B12 用户可见文案不得再出现「桌面更新」。

## [1.0.3]（2026-09-11）

### 修复：内核步骤同样无界 —— 与引导卡死属同一类缺陷（主动审查发现）

修完桌面更新的超时后，主动审查其余网络步骤，发现内核路径存在**同类且更隐蔽**的问题：

**① `npm install` 无限阻塞（Rust）**
- `core::install_version()` 用 `cmd.output()`，**没有任何超时** —— npm 因网络停滞或 registry
  无响应而挂起时，引导页永久停在「正在安装内核…」。
- 修复：改为有界执行（15 分钟上限，超时即 kill 并如实报错，供引导页重试/回退）。
- **实现细节（易踩坑）**：子进程输出重定向到**临时文件**而非管道。若用 `Stdio::piped()` 且不读取，
  npm 的冗长输出会填满约 64KB 的 OS 管道缓冲区，导致子进程阻塞 —— 本想修超时却引入死锁。
  临时文件无此风险，且超时后还能保留最后输出用于诊断。

**② 前端对内核步骤无兜底（bootstrap.html）**
- `core_plan`（多镜像回退查询）与 `core_apply`（安装）均未包 `withTimeout`。
- 修复：分别加 90 秒 / 17 分钟预算（后者覆盖 Rust 侧 15 分钟上限），超时明确失败而非无限等待。

### 回归防护
`tests/bootstrap_flow.rs` 新增 B8：断言前端两个内核预算常量与 `withTimeout` 包裹、
Rust 侧 `NPM_INSTALL_TIMEOUT` 与有界执行函数存在、且**代码中不再出现** `cmd.output()`。
（断言只检查代码行，注释中对旧实现的说明不算违规。）

### 验证
壳仓全部 Rust 测试通过：`bootstrap_flow` 8 项、`updater_artifacts` 6 项、内置单元测试 3 项。
## [1.0.2]（2026-09-11）

### 修复：桌面壳引导顺序错误导致首次启动卡死（Windows 真机实测）

#### 现象
用户在 Windows 真机安装桌面壳后，引导页**第一步就是「壳更新」并卡住不动**，无法进入产品。

#### 根因（两个叠加缺陷）

**① 步骤顺序设计错误**（用户直接质疑的点）

原顺序为：`壳更新 → 检测环境 → 运行环境 → 内核版本 → 守卫就绪 → 进入面板`。
把「桌面壳自更新」当成第一步的理由是「新壳才可能带有新的 Node/内核安装要求」，
**该前提不成立**：
- 环境检测与 Node 探测都是**本地**判定，与壳版本无关；
- 用户刚手动安装完桌面壳，此时壳本就是最新，再强制检查更新毫无意义；
- 壳更新是**网络**操作（最慢、最不可靠），放在首位意味着「最可能失败的操作挡住所有后续步骤」。

**② 网络请求无超时 → 永久挂起**

`tauri-plugin-updater` 的 `Config`（tauri.conf.json）**没有 timeout 字段**，只能在 Builder 上设；
而底层 `reqwest` **默认无总超时**。于是网络不可达/连接挂起时（本项目端点用 unpkg CDN，
在部分网络环境下连接会长时间停滞），`check()` 既不返回也不报错 → 前端 Promise 既不 resolve
也不 reject → `.catch` 不触发 → 页面**永久停在「正在检查桌面更新」**，且当时**没有跳过入口**。

#### 修复

**顺序重构**：`检测环境 → 运行环境 → 桌面更新 → 内核版本 → 守卫就绪 → 进入面板`。
本地、快、确定的检查在前；网络类更新放在环境就绪之后。

**多重超时兜底**（三层，任一层生效即不会卡死）：
- Rust：`check()` 用 `updater_builder().timeout(20s)` **加** `tokio::time::timeout` 外层兜底
  （reqwest 的 request timeout 不保证覆盖 DNS 等阶段）；
- 下载用 20 分钟长超时（大安装包 + 慢网）；
- 前端：`withTimeout()` 再包一层（检查 45s / 下载 5 分钟无进展即判定失败）。

**用户随时可跳过**：新增「跳过桌面更新，直接启动」按钮 —— 任何网络类步骤都必须有即时出口，
否则一旦底层挂起，用户除了杀进程别无选择。

**下载进度可视化**：此前进度回调体是空的（`let _ = (chunk, total);`），4MB+ 安装包在慢网下
长时间零反馈，用户无法区分「正在下载」与「卡死」。现每秒级上报已下载/总字节并按百分比显示。

#### 附带修复

- **Windows 上护栏账本失效**：`tauri-plugin-updater` 在 Windows 安装时执行
  `ShellExecuteW` 启动安装程序后**立即 `std::process::exit(0)`**，其后的代码永不执行。
  原实现把 `mark_pending()` 放在 `download_and_install()` 之后 → Windows 上「更新成功确认 /
  连续失败拉黑」机制**完全失效**。现改为**安装之前**记录。
- **`bump.sh --shell` 在开发工作流下必然失败**：该分支硬编码 `./src-tauri`，假设在壳仓根执行；
  而本项目的实际流程是在内核仓根执行（壳仓由 `export-shell.sh` 同步）。现同时支持
  `./src-tauri` 与 `./.shell-work/src-tauri`，并同步更新 `Cargo.lock` 中的包版本。
- **移除「门 0」这一内部概念**：它从未出现在任何需求中，是我自行引入的编号并直接暴露给了用户。
  已全部改为「桌面更新」等直白表述，并在回归测试中禁止其回到用户可见文案里。

#### 回归防护

新增 `src-tauri/tests/bootstrap_flow.rs`（7 断言），锁定：步骤顺序、HTML 步骤条与 JS `stepNames`
一致、引导从环境检测启动、网络步骤必有超时与跳过出口、下载进度已接线、Rust 侧超时与
`mark_pending` 顺序、用户文案不含内部概念。
