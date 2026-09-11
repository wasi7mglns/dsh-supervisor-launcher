# 桌面壳升级方案（数据安全评估，讨论稿）

> 你的问题：现有 DSH-SUP 要不要卸载？数据（路由账号/实例）能保留吗？
> **结论：不需要卸载，数据零风险。但有一个必须避开的操作陷阱（见 §3）。**

---

## 一、为什么数据绝对安全（三条硬证据）

### 证据 1：deb 包只拥有 `/usr` 下的文件

`dpkg -L dsh-supervisor` 的完整清单只有四类：

```
/usr/bin/dsh-supervisor-gui                              ← 二进制
/usr/share/applications/dsh-supervisor.desktop           ← 桌面入口
/usr/share/icons/hicolor/*/apps/dsh-supervisor-gui.png   ← 图标
/usr/lib/dsh-supervisor/**                               ← 包内资源
```

**没有任何 `/home` 下的路径。**

### 证据 2：所有数据目录都不属于任何 deb 包

逐个验证（`dpkg -S <dir>` 全部无归属）：

| 目录 | 归属 |
|---|---|
| `~/.dsh` | ✅ 不属于任何 deb 包 |
| `~/.dsh/supervisor` | ✅ 不属于任何 deb 包 |
| `~/.npm-global` | ✅ 不属于任何 deb 包 |
| `~/.tauri` | ✅ 不属于任何 deb 包 |
| `~/.config/autostart` | ✅ 不属于任何 deb 包 |

### 证据 3：没有 maintainer scripts（卸载不执行任何自定义钩子）

`/var/lib/dpkg/info/dsh-supervisor.*` 只有两个文件：

```
dsh-supervisor.list       ← 文件清单
dsh-supervisor.md5sums    ← 校验和
```

**没有 `prerm` / `postrm` / `preinst` / `postinst`** → 卸载/升级时不会跑任何脚本，
更不可能去动 `~/.dsh`。

### 而且：包名相同 → `dpkg -i` 是**原地升级**，不是卸载重装

已安装 `dsh-supervisor` 0.1.0，新包也叫 `dsh-supervisor` 1.0.1 → dpkg 识别为同一包，
**原地替换** `/usr/bin/dsh-supervisor-gui`。整个过程不涉及删除数据。

---

## 二、要保留的数据清单（实际盘点）

| 数据 | 位置 | 体积 | 内容 |
|---|---|---|---|
| **智能路由账号** | `~/.dsh/supervisor/providers.json` | 56K | **2 个供应商 / 25 个账号** |
| 路由用量 | `router-usage-totals.json` | 5K | 用量统计 |
| 路由证据 | `router-upstream-evidence.jsonl` | 88K | 上游证据链 |
| **实例定义** | `~/.dsh/supervisor/instances.json` | 4.6K | **4 个实例**（插件开发/测试/抖音发布/开发） |
| **实例数据** | `~/.dsh/supervisor/instances/` | **3.2G** | 各实例独立 DSH 副本 + 数据 |
| 受管对象（权威） | `managed-objects.json` | 5K | desired/guardian/phase |
| 守卫配置 | `config.json` | 1.4K | 端口/镜像/开关 |
| 任务历史 | `tasks.json` | 88K | 安装/升级历史 |
| 事件日志 | `events/` | 14M | 可审计回放 |
| DSH 会话数据 | `~/.dsh/sessions`（136M）、`~/.dsh/storages`（980K） | — | 对话与存储 |
| 内核子包 | `~/.npm-global/lib/node_modules/@dsh-sup` | 1.7M | 守卫本体 |
| **签名私钥** | `~/.tauri/` | 28K | 壳自更新签名（新增） |

**以上全部在用户目录，dpkg 一个都不会碰。**

---

## 三、⛔ 必须避开的陷阱：不要用壳的「退出管家」菜单

这是本次评估**最重要的发现**。

壳的托盘菜单「退出管家」会执行：

```rust
"quit" => {
    shutdown_all(port);   // 见下
    app.exit(0);
}
```

而 `shutdown_all()` 会：

1. `POST /session/stop` → 守卫的 `shutdownAll()` → **停掉全部被管对象**（含所有实例与主 DSH）
2. 轮询等 `sessionState == stopped`
3. `systemctl --user stop dsh-supervisor` → **停掉守卫**

**后果**：

| 影响 | 说明 |
|---|---|
| **会杀死我的会话** | 我运行在实例 `inst-…-203`（端口 3085）内 → 会被停掉 |
| 会停掉全部 4 个实例 | 其中包括你正在用的「开发」实例 |
| 会停掉主 DSH 与守卫 | 整个服务链下线 |

**所以：升级壳绝不能用这个入口。**

---

## 四、正确的升级步骤（推荐）

```bash
# 1) 确认守卫与实例保持运行（不动它们）
systemctl --user status dsh-supervisor   # 应 active

# 2) 原地升级 deb（同包名 → dpkg 视为升级；只替换 /usr 下文件）
sudo dpkg -i dsh-supervisor_1.0.1_amd64.deb

# 3) 只重启「壳进程」——注意：不是退出管家
#    壳是独立 GUI 进程，重启它不影响守卫/实例/会话数据
kill 2930            # 旧壳进程（持有旧 inode）
# 或直接在窗口上点关闭（若 closeAction=hide 则只是隐藏）

# 4) 启动新壳（用新二进制）
setsid /usr/bin/dsh-supervisor-gui >/dev/null 2>&1 &
```

**关键点**：

- 步骤 2 的 `dpkg -i` **不会**触碰 `~/.dsh`（已由 §1 三条证据保证）
- 步骤 3 **只结束壳进程**，不经过 `/session/stop`，因此实例与我的会话都不受影响
- Linux 上替换运行中的二进制是安全的：旧进程持有旧 inode，新进程读新文件

### 一个替代方案（更省事）

由于自启动已指向 `/usr/bin/dsh-supervisor-gui`：

```bash
sudo dpkg -i dsh-supervisor_1.0.1_amd64.deb
kill 2930                  # 关掉旧壳
# 然后重新登录，或手动启动一次；之后开机自启就是 1.0.1
```

---

## 五、关于「先卸载再装」

**不必要，且有害无益**：

| 方式 | 结果 |
|---|---|
| `sudo dpkg -i` 直接升级（推荐） | 原地替换，数据零接触，一条命令 |
| `sudo dpkg -r` 卸载再装 | 多一步，且**没有任何好处**（deb 不拥有数据，卸载同样不删数据） |
| `sudo dpkg -P` 彻底清除 | 仍不删 `~/.dsh`（配置不在包里），但**没必要冒这个险** |

---

## 六、顺带发现的小问题（非阻塞）

`~/.config/autostart/dsh-supervisor-gui.desktop` 的内容是：

```
Comment=DSH 监管面板(官方 0.1.0)
Exec=/usr/bin/dsh-supervisor-gui
```

- 这个文件是**用户级**的（由内核的 autostart 功能生成），**不是 deb 拥有**
- 因此升级 deb **不会**更新它 → 版本注释会一直停留在「官方 0.1.0」
- `Exec` 路径正确，不影响功能；仅是显示文案陈旧
- 可在升级后顺手修正（改 Comment），或由内核的 autostart 逻辑按实际版本重写

---

## 七、验收自动更新所需的完整前置

要真正验证「自动更新」，需要两个条件同时成立：

| # | 条件 | 现状 |
|---|---|---|
| 1 | 运行的壳**含 updater 代码** | ❌ 当前是 0.1.0（无门 0 代码）→ **必须手工装一次 1.0.1** |
| 2 | 存在**更新的版本** | ❌ 当前最新即 1.0.1 → 需再发一个 1.0.2 |

**这是 minisign 机制的固有特性**：新公钥/新机制必须随一次**手动安装**进入用户端；
此后（1.0.1 → 1.0.2 → …）才能全自动。

### 建议的验证路径

1. 手工安装 1.0.1（本文档 §4）
2. 重启壳 → 确认门 0 报告「已是最新 v1.0.1」（验证**检查链路**通）
3. 发 1.0.2（`git tag v1.0.2 && push`）→ CI 自动构建并发布
4. 重启壳 → 应自动完成「下载 → 验签 → pkexec 安装 → 重启」（验证**更新链路**通）

---

## 八、需要你确认

| # | 事项 | 说明 |
|---|---|---|
| 1 | 是否按 §4 原地升级（推荐） | 不卸载、不动数据 |
| 2 | **安装需要 sudo 密码** | 我无法执行 `sudo`（本机需密码）；这一步需你运行，或授权我通过其它方式 |
| 3 | 重启壳的时机 | 若你正通过**壳窗口**看面板，重启会中断视图（数据不丢）；若通过**浏览器**访问 3085 则完全无感 |

---

## 九、⚠ 一次自我纠错（重要，避免误导你）

排查过程中我一度以为「系统属主被大规模改坏」——`/usr`、`/etc`、`/var/lib/dpkg` 下有 19104 个文件
显示属主为 `nobody:nogroup`，`sudo` 也报错。**这个判断是错的**，现更正：

### 真实原因：我（AI 会话）运行在 DSH 沙箱的 user namespace 内

| 视角 | `/proc/self/uid_map` | 含义 |
|---|---|---|
| **我（沙箱内）** | `1000 1000 1` | **仅映射 uid 1000**；其余 uid 一律显示为 `65534`（未映射溢出值） |
| **守卫（宿主机）** | `0 4294967295 4294967295` | 完整映射 → `root=0` 正常显示 |

验证：
- `/` 在宿主机必为 `root(0)`，在我这里显示 `65534` → 正是「未映射」的典型表现
- `/home/bowen` 属主 `1000` → 正常显示 `1000`（因为被映射）
- 守卫进程的 uid_map 是**完整映射** → 它看到的属主是真实的

### 结论

| 我的误判 | 事实 |
|---|---|
| 「19104 个文件属主损坏」 | ❌ 它们**本来就是 `root:root`**，只是在我的 userns 里显示为 nobody |
| 「`sudo` 损坏」 | ❌ `sudo` 正常；只是**在 user namespace 内无法提权**（userns 固有行为） |
| 「`/usr/bin/dsh-supervisor-gui` 属主异常」 | ❌ 在宿主机上是 `root:root` —— **这正是 deb 正确安装的结果** |

### 对你意味着什么

- **你的系统没有损坏**，不需要任何修复。
- **你在真实终端里 `sudo` 是正常的** —— §4 的 `sudo dpkg -i` 完全可行。
- **我无法执行 `sudo`**：不是权限配置问题，而是**我所在的沙箱从设计上就不允许**
  （这也正是沙箱隔离的意义）。所以安装这一步需要你运行。

### 教训（记录以免重复）

在沙箱内观察宿主机文件系统时，**属主/权限必须先看 `uid_map` 再下结论**；
单 UID 映射会把所有非映射属主显示成 `nobody`，看起来极像「系统被 chown 坏了」。

