# Changelog（桌面壳）

本文件记录桌面壳（`dsh-supervisor-gui`，公开仓 `wasi7mglns/dsh-supervisor-launcher`）的重要变更。

## [未发布]

（下一版本待记）

## [1.0.7]（2026-09-11）

### 修复：引导页 JS 语法错误导致引导完全静默（本轮真正的根因）+ 把真机验证变成常规手段

#### 一、决定性发现：一个逗号让整个引导页失效

在 diagText() 里新增数组元素时**漏了行尾逗号**。下面是修复前后的对照（
**修复前那段是错误代码**，仅为说明；当前源码已是「修复后」形态）：

```js
// ❌ 修复前（错误）：第 310 行末尾没有逗号，导致整个 <script> 块语法错误
//     ... : (warmTimer ? 'A' : 'B')))      <-- 此处应有逗号
//     'mirror_node_best=' + ...
//
// ✅ 修复后（当前源码，见 src-tauri/bootstrap/bootstrap.html 第 310 行）：
//     ... : (warmTimer ? 'A' : 'B'))),     <-- 行尾逗号
//     'mirror_node_best=' + ...
```

**后果是整段 script 块语法错误，导致所有 JS 都不执行、boot() 从不运行**，
于是页面永远停在 HTML 里的静态文案「正在检测系统环境…」：

- **不报错**（没有任何 JS 在执行，也就没人报错）；
- **不推进**（boot() 未被调用）；
- **诊断全 none**（diagText() 也没执行）；
- Rust 侧日志只有「壳启动」一行（前端从未调用 shell_set_phase）。

这个现象与「探测卡住」**表面完全一致**，但病因截然相反 —— 一个在前端一行 JS，
一个在 Rust 探测逻辑。我因此在 Rust 侧来回排查了三轮。

#### 二、更该反省的：我提交前明明跑了检查，却没看结果

我在提交前执行了 node --check，它**失败了**，但命令用了 && 串联，
失败导致后续「成功提示」未打印 —— 而**我没有核对输出就继续往下走**。

教训：**「跑过检查」不等于「检查通过」。必须核对结果，且最好是自动化的。**
故本次把 JS 语法检查固化为门禁 B53（见下）。

#### 三、方法论的突破：用真实 GUI 验证，而不是只查源码

此前我一直在「读代码 + 静态断言」的层面验证，而本轮改用**真实运行**：

```bash
Xvfb :95 -screen 0 1400x900x24 &
dbus-run-session -- bash -c "HOME=$WORK DSH_BOOT_TRACE=1 ./dsh-supervisor-gui"
# 然后读 $WORK/.dsh/shell/shell.log 看 phase 推进
```

修复前 shell.log 只有 1 行；修复后：

```
[boot] main enter
[boot] building app
[boot] setup enter
[boot] init_identity done
[boot] setup: building tray
壳启动 v1.0.7
阶段 -> env                <- 检测环境 OK
阶段 -> shell-update       <- 桌面版本 OK
阶段 -> kernel             <- 内核版本 OK
阶段 -> guard              <- 守卫就绪 OK
守卫服务定义: 已建立并启用 ~/.config/systemd/user/dsh-supervisor.service
```

**这才是「链路是否真的通」的可核对证据**，而不是我的判断。

#### 四、把启动里程碑日志固化为常开能力（而非临时脚手架）

shell.log 若只有「壳启动」一行，**无法区分**两种截然不同的病因：

| 现象 | 病因 | 修复方向 |
|---|---|---|
| setup 从未执行 | Rust 侧插件/DBus 层失败 | 查插件初始化 |
| setup 正常但前端无日志 | 前端 JS 未执行 / IPC 失败 | 查前端 |

两者方向相反，而我为此来回三轮。现改为**常开**：
main enter -> building app -> setup enter -> init_identity done -> building tray，
每次启动 6 行（shell.log 超 1MB 自动滚动），打开日志一眼即可定位到**哪一层**。

> 成本极低、收益极高：这是用一次真实事故换来的诊断能力。

#### 五、新增门禁 B53：前端 JS 必须语法正确

提取 HTML 全部内联 script，逐个跑 node --check；
node 缺失时**明确 SKIP**（而非静默通过 —— 否则门禁形同虚设）。
失败信息直接打印语法错误与行号，**不依赖人的注意力**。

#### 六、版本号合并（用户要求）

原计划分两次发布（1.0.7 / 1.0.8），但两者**均未发布**。
按用户要求「不要为每个修复递增版本号」，已**合并为单一 1.0.7**：
tauri.conf.json / Cargo.toml / Cargo.lock 三处一致，CHANGELOG 两段合并为一段。

#### 验证（本轮全部用真实运行）

- **真实 GUI（Xvfb + 全新 dbus session）**：引导链路完整推进至 guard，
  **P0 服务定义真实建立**（systemctl --user unit 落盘），镜像选出 mirrors.huaweicloud.com；
- 壳测试 **66 项全通过**（bootstrap_flow 52 + updater_artifacts 6 + 单元 8，含新增 B53）。


# 本版合并了原计划分两次发布的修复（1.0.7 / 1.0.8）——
# 两者均未发布，故合并为单一版本，避免无意义的版本号膨胀。

### 修复：镜像「一等公民」化 + 数据流审计（用户质疑驱动：能力没丢，可见性丢了）

用户指出：「**你连镜像源都看不到**，根本就不会去选择镜像源…我严重怀疑你的探针和镜像
配置没放在壳里，是丢失状态」。

#### 一、先用实物证据回答「能力是否丢失」：**没有丢失**

我下载了**用户手上那个已发布 1.0.6 的安装包**、解包、与本地从同一源码构建的 release
二进制对照：

| | 本地 release | 已发布 1.0.6 |
|---|---|---|
| 二进制大小 | 10,497,408 | 10,500,608（差 3KB，构建环境差异）|
| 探针/镜像/引导页标记 | 有 | 有 |

壳内实际代码（全部编译进二进制）：

```
nodeprobe.rs  638 行   环境探针        mirror.rs  282 行  18 个镜像源
node.rs       425 行   Node 下载+安装   core.rs    465 行  内核 npm 安装
service.rs    257 行   三平台服务定义   bounded.rs 190 行  有界执行
bootstrap.html 815 行  引导页 UI（压缩嵌入二进制）
```

`tauri.conf` 的 `frontendDist=bootstrap`、窗口 `url=shell.html`、
`shell.html` 内 `iframe src="bootstrap.html"` —— **引导页是本地资源，不依赖内核**。

按引导页调用顺序，八个能力**全部由壳提供**：探针 / 镜像测速 / Node 下载校验 / Node 安装 /
内核 npm 安装 / 服务定义 / 守卫启动 / 引导 UI。**内核是被安装的对象，不是能力提供者。**

> 附：我最初用 `strings` 查二进制，结果"找不到"标记 —— 那是**我的测量方法错了**
> （Tauri 压缩嵌入资源，明文不可见）。改用本地/已发布对照后结论反转。

#### 二、但用户的观察是对的：**镜像可见性确实丢失**

代码路径上的必然结果：

```js
// bootstrap.html afterEnv
nodeVer = st.installed;
return stepNodeDone();   // ← 直接放行，不调 probeMirrorThen
```

**只要 Node 已达标（主力用户就是这样），镜像探测根本不执行** → `lastMirror` 恒为 null
→ 诊断串必然 `mirror=none | mirror_probes=none`。

而 `core_plan` **早已回传 `registry`（命中的镜像）**，前端**从未使用** —— 数据链路是断的。

#### 三、全字段数据流审计：**9 个字段后端产出、前端从未使用**

```
registry  updateAvailable  nodeBest  npmBest  path
package   hint            selectedNode  selectedNpm
```

这说明问题不是「能力放在内核里」，而是「**壳有能力却完全不展示**」。
外壳做了大量工作，用户什么都看不到 —— 这会让人合理地怀疑能力根本不存在。

#### 四、修复：把镜像提升为一等公民

镜像不是「下载 Node 的辅助」，而是**壳所有网络动作的基础设施**（内核安装同样依赖它）。

1. **预热**：引导开始即后台并行测速（`mirror_warmup`，**立即返回不阻塞**）；
2. **全程可读**：`mirror_cached` 纯读缓存（**无网络 I/O**，可安全高频轮询）；
3. **全步骤可见**：
   - 环境步骤：显示「镜像已就绪：npmmirror.com（185ms）」；
   - 内核步骤：显示「内核已是最新（v0.1.5）· 源 mirrors.huaweicloud.com」（**消费 `p.registry`**）；
   - 安装步骤：「未安装内核 · 正在安装标准产品包…（源 xxx）」；
4. **诊断串始终带镜像**：
   `mirror_npm_best=... | mirror_probes=源1(88ms) > 源2(x) > ...`，
   并将「预热中」与「全部不可达」**区分开**（而非一律 none）。

#### 五、顺带查出一个真实缺陷：npm 探测用了根路径

`probe_all(&npm, "")` 实际请求 `https://<源>/`，而多数 registry 根路径返回 **404** ——
**健康的源被判「不可达」**。

实测：腾讯云 npm 镜像连测 3 次均 HTTP 200、能正确返回我们的包，
却因根路径 404 被显示为「不可达」并排除在选择之外。**探测方法错误让壳无谓地少一个镜像。**

已改用真实包名做探针。修复后 npm 侧 **6/6 全部可达**（修复前 5/6）。

#### 六、新增无头自检 + 门禁

**`--mirror-plan`**（无 GUI 验证镜像，任何平台可用）：

```
=== 镜像测速自检 ===
Node 候选 10 个 / npm 候选 6 个
https://mirror.nju.edu.cn/nodejs-release          可达    108 ms
https://registry.npmmirror.com                    可达    290 ms
...
Node 选中: https://mirror.nju.edu.cn/nodejs-release (108 ms)
npm  选中: https://registry.npmmirror.com (290 ms)
```

新增门禁 B49–B52：
- **B49** 镜像必须与引导并行预热（不得只在下载分支里产生），诊断必须区分「预热中」；
- **B50** `core_plan` 回传的 `registry` 必须被前端消费（数据链路不得断裂）；
- **B51** 内核步骤必须显示当前镜像；
- **B52** npm 探测必须用真实包名（不得用根路径）。

#### 验证

- 壳测试 **65 项全通过**（bootstrap_flow 51 + updater_artifacts 6 + 单元 8）；
- `--mirror-plan`：Node 10 源全可达、npm 6 源全可达，正确选出最快源。

### 修复：卡住的第三层根因 —— 候选枚举落在进度上报之外（诊断三项全空的真正原因）

用户反馈 1.0.6 仍卡在检测环境，诊断串为：

```
env_candidates=none | env_stuck=none | env_trace=none | error=环境检测超时
```

#### 三个 none 由同一件事解释 —— 这是决定性线索

`detect()` 的执行顺序是：

```rust
fn detect() {
    let cands = candidates();          // ← 整个枚举在这里，**在进度上报之外**
    if ... { l.summary = summary; }    // ← 永远到不了
    for (source, cand) in cands {
        live_stage(...)                // ← 永远到不了
```

而 `candidates()` 内部会依次做三件可能阻塞的事：

| 步骤 | 阻塞点 |
|---|---|
| ① 读 `runtime.json` | 漫游配置/网络用户目录上的 `read` |
| ② 枚举已知落点 | `read_dir`（nvm / fnm / volta 目录） |
| ③ 过滤 PATH | **每个 PATH 条目一次 `GetDriveTypeW`**（微软文档明确提示该 API 可能慢） |

**一旦卡在上述任一步（最可能是 ③），summary / stage / trace 三项同时为空。**
这与用户看到的 `env_candidates=none | env_stuck=none | env_trace=none` **完全吻合**。

#### 这是同一类错误的第三次出现（诚实记录）

| 轮次 | 我做的事 | 留下的盲区 |
|---|---|---|
| 1.0.5 | 把探测搬进分离线程 | **候选枚举本身没搬** —— 仍在命令路径上做 I/O |
| 1.0.6 | 把枚举搬进线程 + 摘要改缓存 | **枚举阶段没有 stage** —— 卡住时三项全空 |
| 1.0.7 | 见下 | —— |

规律很清楚：**「让调用不阻塞」不等于「卡住时看得见」**。
两次我都只做了前者。故本次不只修代码，还把它变成**机械化断言**（B47）。

#### 真正的结构性修复：**边枚举边探测，且 PATH 过滤放到最后**

上一版的顺序是「先把**全部**候选枚举完，再逐个探测」。这有一个致命后果：

> **只要枚举阶段慢/卡（PATH 过滤要逐盘符调 `GetDriveTypeW`），
> 就连已经枚举好的廉价候选都永远试不到** ——
> 用户本可瞬间命中「已知安装落点」，却因 PATH 过滤卡住而**完全失败**。

现改为**交错（interleaved）**：

```
① 读 runtime.json 记录路径  → 立即探测
② 已知安装落点（含 nvm/volta/fnm/scoop）→ 列一个、试一个
③ PATH 过滤 → **最后才做**（唯一需要逐盘符系统调用的阶段）
```

于是「Node 装在标准位置」的绝大多数用户（含本项目的目标场景）
**根本不会走到 PATH 过滤** —— 那个可疑的系统调用连一次都不会被调用。

> 这比「把它加进进度上报」更根本：**不上报不如不调用。**

#### 修复（两条硬规则）

**规则一：任何可能阻塞的调用之前，必须先 `stage()`。没有例外。**

现在枚举的每一阶段都有独立阶段标记，且**增量更新**候选摘要：

```
① 读取 runtime.json 记录路径
② 枚举已知安装落点
③ 过滤 PATH（跳过网络盘/UNC）
③ 收集 PATH 候选 N/M
探测候选 <来源>（<路径>）
```

摘要也改为增量写入，故即使卡在阶段 ③，`env_candidates` 也会显示
`候选 N 个（记录 0 / 已知 5 / PATH 0）（进行中：已完成 ①②）` ——
**一眼看出卡在哪一步、已收集多少**，而不是一片空白。

**规则二：Rust 侧必须有硬上限（25 秒），超限即明确失败并给出原因。**

前端预算 45 秒、Rust 硬上限 25 秒 —— 于是**总是 Rust 先给出结论**：

```
环境探测超过 25000 ms 未完成（卡在「③ 过滤 PATH（跳过网络盘/UNC）」已 24800 ms）
```

这消除了「probing 永远为真」：`node_status` 现在有三种确定的返回 ——
完成 / 进行中（附当前阶段）/ **明确失败（附卡住阶段与耗时）**。

#### 附带改进

- **`GetDriveTypeW` 按盘符缓存**：原实现对**每个 PATH 条目**都调一次，
  而 PATH 里数十个条目往往集中在同一两个盘符 —— 一旦该盘有问题就重复付出阻塞代价。
  现每个盘符最多查询一次（≤26 次）。
- **PATH 条目上限 64**：极端长的 PATH 不应把探测拖成分钟级。
- **`catch_unwind` 包裹探测**：worker 内部 panic 也必须产出结论，
  而不是让线程静默死亡（那会表现为「永不结束的 probing」）。
- **`Disconnected` 视为明确失败**：不再静默停在进行中。
- 前端新增 `env_probe_error` 诊断字段，并据此**立即**给出可操作结论，
  不等自己的预算耗尽（那只得到一句没有信息量的「超时」）。

#### 本次最重要的产出：性质测试 + 机械化门禁

新增**性质测试**（`nodeprobe::tests`），用真实注入复刻线上故障：

```rust
// 注入：让枚举阶段永久阻塞；硬上限压到 500ms 便于快速验证
set_hang_in_enumerate(true);
// 断言 1：卡住时**阶段必须可见**（否则无从排障）
assert!(current_stuck().unwrap().0.contains("模拟枚举"));
// 断言 2：必须在硬上限内给出**明确失败**，且原因包含卡住的阶段
assert!(last.finished);
assert!(err.contains("模拟枚举"));
```

> 这个测试是**直接针对线上故障**写的：它证明「卡在枚举时，用户一定能拿到带阶段的原因」，
> 而不再是三项全空的静默卡死。

新增门禁 **B47**（规则一的机械化检查）：
断言 `enumerate_staged()` 中每一处可能阻塞的调用（`recorded_node_path` /
`known_locations_staged` / `path_dirs_staged`）**之前都存在 `stage(`**，
并断言硬上限存在且能产出可读原因。

#### 验证

- 壳测试 **60 项全通过**（bootstrap_flow 46 + updater_artifacts 6 + 单元 8）；
- 含两条新增性质测试：`hard_deadline_yields_actionable_failure_when_enumeration_hangs`、
  `normal_probe_completes`；
- `--env-plan` 正常（14 个候选，50ms）。

## [1.0.6]（2026-09-11）

### 修复：我上一轮引入的致命回归（命令路径做 I/O）+ 三平台适配逐项核对

用户真机反馈：**1.0.5 仍卡在检测环境，而且不报错**。并要求逐平台核对适配是否正确
——「不要拍脑袋，要真实地看代码」。

#### 一、致命：我上一轮修复时自己引入的缺陷

为让诊断串显示候选数量，我在 `node_status`（**命令路径**）里调了 `candidate_summary()`，
而它内部会**枚举候选** —— 那要做 `read_dir`（版本管理器目录）并对每个 PATH 条目
调 `GetDriveTypeW`。

**这正是我声称已经消除的那类无界阻塞 I/O。我把刚搬走的石头又搬了回来。**

而更糟的是第二个缺陷让它表现为「不报错」：

```js
// 旧：轮询只在 invoke 的 .then 里再调度
function poll() { core.invoke('node_status').then(... setTimeout(poll, 400) ...) }
```

`invoke` 一旦不返回，`poll()` 就**再也不会被调度** —— 既不报错、也不推进。
**轮询循环必须有独立于被调方的心跳**，而单次调用（`withTimeout`）与轮询是两回事。
这解释了用户看到的「卡住且不报错」：不是探测慢，是**轮询自己停摆了**。

修法：
- `candidate_summary()` 改为**只读缓存**（由探测线程在开始时写入），命令路径只读一个 `String`；
- 前端**每次**查询都包 `withTimeout`，单次无响应则继续轮询到总预算耗尽再给出口；
- `stepNodeWait` / `stepGuardReady` 的轮询同样加独立心跳。

#### 二、平台适配逐项核对（真实读码 + 实测，非推测）

**① macOS `.pkg` 的判定标签与产物语义不一致（同类缺陷）**

代码用 `osx-arm64-tar` 标签判定，却下载 `.pkg` —— 靠两者恰好都存在而**侥幸可用**。

实测（解包官方 `node-v24.21.0.pkg`）：payload 中同时含 x86_64 与 arm64 两个 Mach-O 切片
（fat 二进制），**确证 .pkg 是通用包**。又逐版本核对官方 `index.json` 的 `files[]`：

```
osx-x64-pkg    —— 所有 LTS 版本都存在（通用 pkg 的标签）
osx-arm64-pkg  —— 从不存在
osx-arm64-tar  —— 存在，但那是 tarball 的标签
```

已改为 `osx-x64-pkg`，使判定与产物一致。

**② Linux 标签硬编码 `linux-x64`（同一类缺陷，审计新发现）**

arm64 上 `platform_file()` 返回 `linux-arm64.tar.xz`，标签却是 `linux-x64`。
它能通过 `has` 检查只因「x64 标签恰好在 files[] 里」—— 若某版本只有 x64 而无 arm64，
代码仍会判定可用，随后去下载不存在的文件（404）。已改为按架构给出。

**③ Windows arm64 无官方 msi（如实记录限制）**

官方 `files[]` **没有** `win-arm64-msi`（只有 `win-arm64-7z` / `win-arm64-zip`）。
故 Windows arm64 只能装 x64 msi（依赖系统模拟执行）—— 已在代码中**明确记录**这是
有意折中，而非静默忽略。

**④ Windows 经 `cmd /C` 启动的引号不足以承受含空格路径**

`%APPDATA%` 含 Windows 用户名，而用户名**可以含空格**（如 "John Smith"）。
旧写法 `.args(["/C", path, "daemon"])` 会让 cmd 拆错 → 守卫启动失败且错误难解读。
已改用 `raw_arg` 给出 cmd 的经典双引号形式 `""<path>" daemon"`。

**⑤ Windows `schtasks /RL HIGHEST` 可能因权限被拒**

创建「以最高权限运行」的计划任务在非提权会话下可能失败。已改为**失败即降级重试**
（去掉 `/RL HIGHEST`）—— 守卫本身不需要管理员权限。
原则：**权限不足时应降级而非彻底失败**。

**⑥ Windows 落点硬编码 `C:\Program Files`**

真实路径随**系统盘符**与**系统语言**变化（中文系统的目录名被本地化），也可能在
`Program Files (x86)`。已改为经 `ProgramFiles` / `ProgramFiles(x86)` 环境变量推导，
与 `nodeprobe::known_locations()` 口径一致；同时补上 macOS Homebrew 落点 `/opt/homebrew/bin`。

**⑦ `guard_ready` 是同步命令且做网络 I/O**

同步命令在**主线程**执行：最多 400ms TCP 探测 + 3 秒 HTTP 往返，而引导页每 500ms
轮询一次、最多 40 次 —— 合计可占住主线程十几秒，界面**无法重绘**。已改为 async + 阻塞线程池。

**⑧ 三处 `TcpStream::connect` 没有连接超时**

`connect` **无超时**：端口被防火墙 DROP（而非 REJECT）时会等到 OS SYN 重试耗尽
（Windows 默认 20+ 秒）。已统一为 `connect_local()`（`connect_timeout` 800ms）。
**原则：不能依赖「回环地址正常时很快」来省略上限。**

**⑨ `--env-plan` 的候选摘要打印顺序错误**

摘要由探测线程写入缓存，在 `status()` **之前**读取必然为空 —— 诊断输出出现空白，
易被误读为「没有候选」。已调整为先探测后打印。

#### 三、门禁（防同类问题再犯）

新增 B40–B46：

| 断言 | 内容 |
|---|---|
| **B40** | **命令路径不得枚举候选**（摘要必须来自缓存）—— 直接针对本次回归 |
| **B41** | **轮询循环必须有独立心跳**（每次查询都要包超时） |
| B42 | macOS 标签必须与 `.pkg` 产物语义一致 |
| B43 | Windows `cmd /C` 引号必须能承受含空格路径 |
| B44 | 本地 TCP 必须用 `connect_timeout`，且 `guard_ready` 必须 async |
| B45 | 平台标签必须**按架构**给出（不得硬编码 x64） |
| B46 | Windows 路径必须来自环境变量（不得硬编码） |

#### 验证

- 壳测试 **57 项全通过**（bootstrap_flow 45 + updater_artifacts 6 + 单元 6）；
- release 形态编译干净；
- `--env-plan` 候选摘要正常显示（14 个候选），连续 3 次稳定 88–89ms。


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
