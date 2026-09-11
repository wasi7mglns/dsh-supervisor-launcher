# 桌面壳引导逻辑修复 —— 方案 B（强制更新 + 失败回退）· 跨平台规范

> 对应 AUDIT-SHELL-BOOTSTRAP.md 的 K1–K11 修复。壳仓 dsh-supervisor-launcher。
> 本环境**无 cargo**，Rust 未编译；已做结构核验 + JS 语法校验 + 算法实证（见 §5）。**须在壳仓 cargo build 最终验证**。

---

## 1. 方案 B 的语义（落地行为）

    启动壳 → 检测环境（Node）
          → 内核版本治理：
              读本地已装版本 → 查镜像全量最高版本 → 比较
              ├─ 无内核         → 安装最新
              ├─ latest > 已装  → **强制更新**（无跳过）
              └─ latest ≤ 已装  → 保持（**绝不降级**）
          → 校验安装确实生效（防「装到别的前缀」）
          → 启动守卫（申请所有者）→ 就绪双探针（TCP+HTTP）
               └─ 不就绪且本次升级过 → **回退旧版本**再试一次
                    └─ 仍不成功 → 失败态：原因 + 重试 + 诊断
          → 进入面板

---

## 2. 跨平台规范决策（特别要求的部分）

| 关注点 | 规范做法 | 依据/实证 |
|---|---|---|
| **安装前缀** | 从已定位内核的**真实路径反推**（Unix prefix/lib/node_modules/...；Windows prefix/node_modules/...），npm install -g --prefix | 实证：npm prefix -g = nvm 路径，内核却在 ~/.npm-global —— 直接用默认前缀会装错位置，旧内核继续遮蔽 |
| **版本来源** | dist-tags + versions **全量取最高** | 实证：官方 dist-tags.latest = 0.1.1-BETA.1，实际最高 0.1.2-BETA.7 —— 只信 latest 会**降级** |
| **镜像顺序** | 读内核 ~/.dsh/supervisor/registry.json（manual→manualOrigin；否则 origins），逐个回退 | 与内核 dist 域同源，尊重用户镜像选择 |
| **包名** | 单一真源 core::package_name()：@dsh-sup/dsh-core-{linux,darwin,win}-{x64,arm64} | 消除散落硬编码（原发现 @dsh-core 错误串） |
| **npm 可执行** | Windows = npm.cmd，其余 = npm | npm 在 Windows 的 shim 形态 |
| **候选路径** | PATH + %APPDATA%\npm (Win) + /opt/homebrew/bin、/usr/local/bin (mac) + ~/.npm-global/bin、~/.local/bin (Unix) + 资源目录 | 覆盖三平台标准安装位置 |
| **符号链接** | fs::canonicalize 解析（~/.local/bin 是软链） | 前缀反推需真实路径 |
| **候选仲裁** | 多候选**按版本最高**（K5） | 旧内核不得遮蔽新内核 |
| **进程无窗** | Windows creation_flags(CREATE_NO_WINDOW) | GUI 调 npm 不弹控制台 |
| **版本比较** | semver（release>prerelease；数字段<字符串段；忽略 build） | 与内核 semverCompare 同语义 |

---

## 3. 代码结构变更

### 新增 src-tauri/src/core.rs（内核版本治理）
- package_name() / npm_exe() / registry_origins() / latest_version()
- semver_cmp() / is_valid_version() / parse_version_output()
- installed_version()（优先包内 package.json，兜底 --version）
- global_prefix_for()（跨平台前缀反推）
- install_version()（npm 安装，含退出码/stderr 错误）
- build_plan() / plan_text()（引导决策 / --core-plan 无头自检）

### main.rs 命令面（契约对账：注册 == 调用，无孤儿）

| 命令 | 作用 |
|---|---|
| node_status | Node 探测（原有） |
| start_node_install | Node 安装（原有） |
| core_status | 本地内核：installed/version/path/真实包名 |
| core_plan | **新增**：版本规划（installed/latest/action/registry/error） |
| core_apply(version) | **新增**：安装/升级/回退（带前缀、镜像回退、如实报错） |
| guard_start | **新增**：申请所有者启动守卫（唯一权威） |
| guard_ready | **新增**：TCP+HTTP 双探针就绪确认 |
| finish_boot / win_ctl | 原有 |

**删除**：core_install（吞错的 fire-and-forget）、skip_env_upgrade（死命令）—— K4/K9。

### bootstrap.html（重写）：5 步 + 失败恢复
- 步骤：检测环境 → 运行环境 → **内核版本** → **守卫就绪** → 进入面板。
- **消除假成功**（K2）：超时不再置成功；失败明确进入失败态。
- **失败恢复 UI**（K7）：原因 + 重试 + 复制诊断。
- **安装后校验**（新）：core_status.version 必须等于目标版本，否则判定「前缀不一致」失败。
- **事件闭环**：env_error 现被消费（不再无人监听，K3 残留）。

### shell.html：导航健壮性（K6）
- force 标志：用户重新显示窗口时强制重载取最新 UI。
- iframe 加载失败最多重试 2 次（一次性 nonce 破缓存）。

### main.rs 单流程（K3）
- 删除 setup 线程里的并行 ensure_guard 与无人监听的 env_ready；引导页成为**唯一**驱动。

---

## 4. 缺陷 → 修复对照

| 编号 | 缺陷 | 修复 |
|---|---|---|
| K1 | 无内核版本检测/强制更新 | core_plan + core_apply + 引导页强制更新步骤 |
| K2 | stepGuardWait 超时假成功 | 删除假成功，明确失败态 |
| K3 | 双流程 + 事件断裂 | 单流程（引导页驱动）；env_error 被消费 |
| K4 | core_install 吞错 | core_apply 回传退出码/stderr |
| K5 | 多候选无版本仲裁 | locate_core 按版本最高 |
| K6 | 就绪竞态 / 同 URL 不重载 | guard_ready 双探针 + force + 失败重试 |
| K7 | 无失败恢复入口 | 失败态：原因+重试+诊断 |
| K8 | 无版本兼容检查 | 版本治理 + 安装后校验生效 |
| K9 | skip_env_upgrade 死命令 | 删除 |
| K10 | Node 仅最低门槛 | 维持（设计使然）；内核改为强制更新 |
| K11 | latest_lts 无节流 | 保留（busy 期间不重复触发）；后续可加 in-flight 标记 |

---

## 5. 验证（本环境可做的部分）

| 验证 | 结果 |
|---|---|
| 命令/事件契约对账 | 注册 9 == 调用 9；无孤儿命令、无未注册调用 |
| 硬编码包名扫描 | 无 @dsh-core / dsh-core-linux-x64 残留 |
| 括号平衡（4 个 Rust 文件） | 全部 OK |
| HTML 内嵌 JS 语法（node --check） | bootstrap / shell 均 OK |
| **算法实证**（与 core.rs 同构的 JS 参考实现 × 真实 registry） | origin=npmmirror（尊重 registry.json）；installed=0.1.2-BETA.7；latest(max)=0.1.2-BETA.7；**action=none（不降级）**；A1 证明取全量最高而非 dist-tags.latest(=0.1.1-BETA.1) |

---

## 6. 交接（必须）

1. **壳仓 cargo build 编译验证**（本环境无 cargo）。
2. 冒烟：dsh-supervisor-gui --core-plan（新增无头自检入口）应输出 package/origins/latest/latest_origin。
3. 三平台实测：Linux（systemd）、macOS（launchctl）、Windows（schtasks）各跑一次「升级→就绪→（人为制造失败）→回退」。
4. 与内核 registry.json 的一致性：manual 模式下壳与内核必须选同一镜像。

---

## 7. 未做（明确边界）

- K10：Node 版本策略仍为「最低门槛放行」（未改为强制最新 LTS）——与内核强制更新策略不同，属产品取舍。
- K11：latest_lts 网络请求的 in-flight 去重（当前靠 busy 抑制，失败重试期仍可能重复）。
- 离线/无网场景：强制更新在无网时会失败并进入失败态（可重试），**不会**静默使用旧内核——这是方案 B 的明确选择。
