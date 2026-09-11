# Changelog（桌面壳）

本文件记录桌面壳（`dsh-supervisor-gui`，公开仓 `wasi7mglns/dsh-supervisor-launcher`）的重要变更。

## [未发布]

以下修复**已完成代码与测试，尚未构建/发布**（按用户要求：先逐项确认后再构建）。

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
