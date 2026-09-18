//! 平台适配层 —— **全仓唯一的平台分支所在地**（2026-09-11）
//!
//! 平台分支全部收敛到本层：新增平台只改本层，平台 bug 的定位不再跨 8 个文件。
//!
//! == 关键的分层违规（本层存在的直接动因）==
//!
//! 「服务定义」曾在顶层 `service.rs`（`ensure_defined`），而「服务启停」
//! (`start_guard_service` / `stop_guard_service`) 曾在 `main.rs` —— **同一个概念的 per-OS 知识分居两层**，
//! 各自带 4 份 `#[cfg]`。加一个平台要改两处**不同层**，且很容易只改一处。
//!
//! 现合并进同一个 [`service::ServiceControl`]：**定义与启停永远是同一个对象**。
//!
//!
//! ```text
//! commands ──▶ domain ──▶ platform ──▶ infra
//!                │                    ▲
//!                └────────────────────┘
//! ```
//!
//! · `platform` 可以依赖 `infra`（如 [`crate::bounded`]）；**不可**依赖 `domain`/`commands`。
//! · 平台分支只允许出现在**本目录内**（门禁 G1）。
//! · 每项能力**要么实现、要么显式 `Unsupported`**，绝不静默成功（门禁 G4）。

pub mod service;

/// 平台实现的共用超时（服务管理命令都很小，短超时足够）。
pub const SVC_QUICK: std::time::Duration = std::time::Duration::from_secs(8);
/// 服务管理命令的常规超时。
pub const SVC_NORMAL: std::time::Duration = std::time::Duration::from_secs(10);

/// 家目录（Windows 用 USERPROFILE，Unix 用 HOME）。
///
/// 放在平台层而非业务层：它是**平台事实**（环境变量名不同），
///   不是业务选择。原实现散在已删除的 `service.rs` 与 `env.rs` 各一份。
pub fn home_dir() -> std::path::PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
}

/// 当前用户名（Windows 用 USERNAME）。
pub fn user_name() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "user".into())
}

/// 系统代理 URL（环境变量之外的**第二来源**）。
///
/// 为什么需要：Windows 用户常用 Clash / v2ray 的**系统代理**（只写 WinINET 注册表，
///   不设 HTTP_PROXY）；macOS 的「网络 → 代理」同理只写 SystemConfiguration。
///   ureq 不读这些位置，于是出现「浏览器能上网，壳却全部镜像不可用」。
/// 返回 http://host:port；无系统代理返回 None。
pub fn system_proxy() -> Option<String> {
    #[cfg(target_os = "windows")]
    let v = windows_system_proxy();
    #[cfg(target_os = "macos")]
    let v = macos_system_proxy();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let v: Option<String> = None;
    v
}

#[cfg(target_os = "windows")]
fn windows_system_proxy() -> Option<String> {
    use std::process::Command;
    let query = |name: &str| -> Option<String> {
        // 必须经 bounded::run（B32：任何外部命令不得裸 .output()/status() 无界阻塞）。
        let mut cmd = Command::new("reg");
        cmd.args([
            "query",
            "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings",
            "/v",
            name,
        ]);
        let out = crate::bounded::run(&mut cmd, SVC_QUICK).ok()?;
        let line = out.stdout.lines().find(|l| l.contains(name))?;
        line.split_whitespace().last().map(|x| x.to_string())
    };
    let enabled = query("ProxyEnable").map(|v| v.ends_with('1')).unwrap_or(false);
    if !enabled {
        return None;
    }
    let server = query("ProxyServer")?;
    // ProxyServer 可能是 "host:port"，也可能是 "http=host:port;https=host:port"。
    let hostport = if server.contains('=') {
        server
            .split(';')
            .find_map(|p| p.split_once('='))
            .filter(|(k, _)| k.eq_ignore_ascii_case("https") || k.eq_ignore_ascii_case("http"))
            .map(|(_, v)| v.to_string())?
    } else {
        server
    };
    if hostport.trim().is_empty() {
        return None;
    }
    Some(if hostport.contains("://") { hostport } else { format!("http://{}", hostport) })
}

#[cfg(target_os = "macos")]
fn macos_system_proxy() -> Option<String> {
    use std::process::Command;
    let mut cmd = Command::new("scutil");
    cmd.arg("--proxy");
    let out = crate::bounded::run(&mut cmd, SVC_QUICK).ok()?;
    let s = out.stdout;
    let field = |k: &str| -> Option<String> {
        s.lines()
            .find(|l| l.trim_start().starts_with(k))
            .and_then(|l| l.split(':').nth(1))
            .map(|v| v.trim().to_string())
    };
    for (enable, host, port) in [
        ("HTTPSEnable", "HTTPSProxy", "HTTPSPort"),
        ("HTTPEnable", "HTTPProxy", "HTTPPort"),
    ] {
        if field(enable).as_deref() == Some("1") {
            if let Some(h) = field(host) {
                let p = field(port).unwrap_or_else(|| "80".into());
                return Some(format!("http://{}:{}", h, p));
            }
        }
    }
    None
}

/// 用户级 Node 安装的**原子落定**（三平台共用）。
///
/// 约定：调用方先把官方归档解到 `staging`。两种布局都由本函数统一处理：
///   · Unix：`tar --strip-components=1` → staging 下直接是 bin/lib/...；
///   · Windows：`Expand-Archive` → staging 下多一层 `node-vX-win-.../`。
/// 落定后返回 node 可执行路径。失败不留下半装状态（root 只在确认可执行后才替换）。
pub fn commit_user_node(
    staging: &std::path::Path,
    root: &std::path::Path,
    node_rel: &[&str],
) -> Result<std::path::PathBuf, String> {
    let probe = |b: &std::path::Path| -> std::path::PathBuf {
        node_rel.iter().fold(b.to_path_buf(), |p, s| p.join(*s))
    };
    let mut base = staging.to_path_buf();
    if !probe(&base).is_file() {
        // Windows：压缩包内多一层版本目录 → 若 staging 下只有一个目录且其中含 node，以此为准。
        if let Ok(entries) = std::fs::read_dir(&base) {
            let dirs: Vec<std::path::PathBuf> = entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.is_dir())
                .collect();
            if dirs.len() == 1 {
                base = dirs[0].clone();
            }
        }
    }
    let found = probe(&base);
    if !found.is_file() {
        return Err(format!("解包后未找到 Node 可执行（{}）", found.display()));
    }
    if let Some(parent) = root.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_dir_all(root);
    if let Err(e) = std::fs::rename(&base, root) {
        return Err(format!("落定 {} 失败: {}", root.display(), e));
    }
    let _ = std::fs::remove_dir_all(staging); // base != staging 时清掉剩余空壳
    let installed = probe(root);
    if !installed.is_file() {
        return Err(format!("{} 未就位", installed.display()));
    }
    Ok(installed)
}

/// 以当前平台的**正确方式**执行版本探针（`<prog> [args...] --version`），有界返回首个非空行。
///
/// 为什么必须在平台层：Windows 上的 .cmd / .bat（如官方 npm.cmd）**不能**被
///   CreateProcess 直接执行，必须经 `cmd /C`。这是平台知识，按门禁 G1 只能出现在本层。
/// 无输出 / 非零退出 / 超时一律 None（视为不可用）—— 「文件存在」不等于「可执行」。
pub fn run_version_probe(prog: &std::path::Path, args: &[String]) -> Option<String> {
    use std::process::{Command, Stdio};
    let mut cmd;
    #[cfg(target_os = "windows")]
    {
        let needs_shell = prog
            .extension()
            .map(|e| {
                let e = e.to_string_lossy().to_ascii_lowercase();
                e == "cmd" || e == "bat"
            })
            .unwrap_or(false);
        if needs_shell {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(prog);
            for a in args { c.arg(a); }
            c.arg("--version");
            cmd = c;
        } else {
            let mut c = Command::new(prog);
            for a in args { c.arg(a); }
            c.arg("--version");
            cmd = c;
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut c = Command::new(prog);
        for a in args { c.arg(a); }
        c.arg("--version");
        cmd = c;
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    crate::bounded::prepare(&mut cmd);
    let mut child = cmd.spawn().ok()?;
    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {
                if started.elapsed() >= std::time::Duration::from_secs(8) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(40));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() { return None; }
    let mut out = String::new();
    {
        use std::io::Read;
        if let Some(mut s) = child.stdout.take() {
            let _ = s.read_to_string(&mut out);
        }
    }
    out.lines().map(|l| l.trim().to_string()).find(|l| !l.is_empty())
}

/// 平台能力声明（供 `--platform-matrix` 自检与诊断，**不参与业务逻辑**）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct Capabilities {
    pub platform: &'static str,
    /// 是否有**原生**服务管理器（systemd / launchd / 计划任务）。
    pub native_service: bool,
    /// 是否有提权通道（**仅壳自更新/可选系统安装**需要；Node 安装已用户级、零权限）。
    pub privilege_channel: bool,
    /// Node 制品形态（`zip` / `tar.gz`，均为用户级零权限归档）。
    pub node_artifact: &'static str,
}

/// Node 官方制品的描述（**平台层解析**，业务层只消费）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeArtifact {
    /// `index.json` 的 `files[]` 标签（如 `linux-x64` / `osx-arm64-tar` / `win-x64-zip`）。
    pub tag: &'static str,
    /// 发布文件名（含版本号），如 `node-v22.12.0-linux-x64.tar.gz`。
    pub file: String,
}

/// 守卫启动所需的**已解析运行期事实**（来自 `runtime_contract`，见其头部的根因说明）。
///
/// 为什么单独成类型：守卫是 `#!/usr/bin/env node` 脚本；服务管理器与 spawn 的 ambient PATH
///   经常不含"壳解析出的 Node"（nvm/fnm/volta、GUI 最小 PATH）。把 node/guard/PATH 作为
///   **显式入参**交给平台实现，服务定义不再假设 ambient 环境。
#[derive(Clone, Debug)]
pub struct LaunchSpec {
    /// Node 可执行绝对路径。
    pub node: std::path::PathBuf,
    /// 内核守卫可执行文件。
    pub guard: std::path::PathBuf,
    /// 子进程应继承的 PATH（nodeBinDir 必在首位）。
    pub env_path: String,
    /// 产品状态根：**必须**注入服务/spawn 环境（`DSH_SUPERVISOR_HOME`），
    ///   否则壳与内核各自推导状态根（XDG 环境差异）→ 契约/端口写到两个目录 → 永远拉不起来。
    pub state_root: std::path::PathBuf,
    /// 壳自身可执行文件 —— 服务定义**唯一**指向的稳定入口（--run-guard）。
    ///
    /// 架构（2026-09-15 二次修正）：服务定义**不再固化 node/guard 路径**。
    ///   定义只写 `<壳> --run-guard`；node/guard 由 --run-guard 在**每次启动时**重新检测
    ///   （node 迁移 / 内核升级自动适配）。旧做法把检测结果写进每台机器现生成的脚本
    ///   （.cmd/.ps1/unit），必然过期，且被编码/前缀/垫片细节反复咬。
    pub shell: std::path::PathBuf,
}

impl LaunchSpec {
    /// 由运行期契约 + 已定位守卫组装（PATH 经 runtime_contract 单一实现）。
    /// 状态根取壳进程解析值（单一事实源），随服务定义与 spawn 注入内核。
    pub fn from_runtime(rt: &crate::runtime_contract::NodeRuntime, guard: std::path::PathBuf) -> Self {
        LaunchSpec {
            node: rt.node.clone(),
            guard,
            env_path: crate::runtime_contract::env_path(&rt.node_bin_dir),
            state_root: crate::env::state_root(),
            shell: std::env::current_exe().unwrap_or_default(),
        }
    }

    /// 服务定义应执行的**稳定入口**：壳自身 + --run-guard。
    ///
    /// 不变量（门禁锁定）：三平台服务定义**不得**出现 spec.node / spec.guard。
    pub fn service_command(&self) -> (&std::path::Path, &'static [&'static str]) {
        (self.shell.as_path(), &["--run-guard"])
    }
}

/// 把稳定入口组装成一行**服务定义命令**（值内部自带引号）。
///
/// systemd 的 `ExecStart`、launchd 的 `ProgramArguments`、schtasks 的 `/TR`
///   三处对引号/数组的要求不同，但「命令 = 壳 + --run-guard」这一事实必须单源。
pub fn service_exec_line(shell: &std::path::Path, args: &[&str]) -> String {
    let mut s = format!("\"{}\"", shell.display());
    for a in args {
        s.push(' ');
        s.push_str(a);
    }
    s
}

/// 以守卫身份运行：`--run-guard` 解析出 node/guard 后调用。
///
/// · Unix：`execvp` **替换**当前进程 —— systemd/launchd 直接追踪真实 node；
/// · Windows：分离启动（不等待）—— 计划任务实例即可结束，保活由看护任务按端口负责。
///
/// 平台分支只允许在本层（门禁 G1）。
#[cfg(unix)]
pub fn exec_guard(spec: &LaunchSpec) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let mut cmd = std::process::Command::new(&spec.node);
    cmd.arg(&spec.guard)
        .arg("daemon")
        .env("PATH", &spec.env_path)
        .env("DSH_SUPERVISOR_HOME", &spec.state_root);
    Err(format!("exec 守卫失败: {}", cmd.exec()))
}

#[cfg(windows)]
pub fn exec_guard(spec: &LaunchSpec) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let is_shim = spec
        .guard
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let e = e.to_ascii_lowercase();
            e == "cmd" || e == "bat"
        })
        .unwrap_or(false);
    let mut cmd = if is_shim {
        // 规范化后的包内 JS 入口不存在时的兜底：垫片必须由 cmd 执行。
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").arg(&spec.guard).arg("daemon");
        c
    } else {
        let mut c = std::process::Command::new(&spec.node);
        c.arg(&spec.guard).arg("daemon");
        c
    };
    cmd.env("PATH", &spec.env_path)
        .env("DSH_SUPERVISOR_HOME", &spec.state_root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        // DETACHED_PROCESS | CREATE_NO_WINDOW：分离运行，父进程（计划任务实例）即可退出。
        .creation_flags(0x0000_0008 | 0x0800_0000);
    cmd.spawn().map_err(|e| format!("启动守卫失败: {}", e))?;
    Ok(())
}

/// 平台契约。
///
/// **不变量 P1**：每个能力要么实现，要么显式 `Unsupported`（见 [`service`]）。
pub trait Platform: Send + Sync {
    /// 平台标识（`linux` / `macos` / `windows` / `unsupported`）。
    fn name(&self) -> &'static str;
    /// 服务控制（**定义 + 启停 + spawn 兜底**，同一对象）。
    fn service(&self) -> &'static dyn service::ServiceControl;
    /// 能力声明。
    fn capabilities(&self) -> Capabilities;

    /// 内核 npm 子包的**平台标签**（`linux-x64` / `darwin-arm64` / `win-x64` …）。
    ///
    /// 这是**平台事实**（OS × ARCH → 标签），必须在平台层解析；
    /// `core.rs::package_name()` 只负责拼 `@dsh-sup/dsh-core-<tag>`。
    /// 返回 `None` = 本平台/架构无对应组合（调用方如实报错，不得猜一个）。
    fn core_platform_tag(&self) -> Option<&'static str>;

    /// **Node 官方制品**：`index.json` 的 files 标签 + 发布文件名。
    ///
    /// 这是纯**制品解析**（平台 → 文件名映射），不含下载/安装逻辑 ——
    /// 正因为它「只回答平台问题」，才有资格进平台层。
    ///
    /// `version` 不带前导 `v`（如 `22.12.0`）。返回 `None` 表示本平台无可用制品。
    fn node_artifact(&self, version: &str) -> Option<NodeArtifact>;

    /// 该目录是否位于**本地固定盘**（Windows 需排除网络盘/可移动盘）。
    ///
    /// 为什么必须问平台：`Path::is_file()` / `canonicalize()` 在断开的映射盘或
    /// UNC 路径上**会触网并阻塞数十秒**，而调用点在引导的关键路径上。
    /// 判定本身（`GetDriveTypeW`）不触网。Unix 恒为 true。
    fn is_local_fixed_dir(&self, dir: &std::path::Path) -> bool;

    /// **Node 可执行文件的已知落点**（按优先级；含版本管理器布局）。
    ///
    /// 供环境探测枚举候选。业务层只消费这个列表，不关心它是 `Program Files`
    /// 还是 `/opt/homebrew` —— 那正是平台知识。
    fn node_candidate_paths(&self) -> Vec<std::path::PathBuf>;

    /// 内核可执行文件的**平台额外候选**（PATH 之外）。
    ///
    /// · Windows —— `%APPDATA%\npm`（npm 全局 bin）下的 `.cmd` 垫片，
    ///   以及包内真实脚本 `node_modules/<pkg>/bin/`；
    ///   后者不可省：`.cmd` 垫片无法被 `package_dir_of` 解析（父目录不是 `bin/`），
    ///   直接给出包内路径既能正确读 `package.json` 取版本，也能让前缀推导正常。
    /// · macOS   —— `/opt/homebrew/bin`（Apple Silicon）、`/usr/local/bin`（Intel）
    /// · Linux   —— 空（PATH 与 ~/.local/bin 已覆盖）
    ///
    /// `pkg` 为内核包名（如 `@dsh-sup/dsh-core-linux-x64`），用于 Windows 包内路径。
    fn core_extra_candidates(&self, names: &[&str], pkg: Option<&str>) -> Vec<std::path::PathBuf>;

    /// 给定 npm 全局 prefix，返回内核在该 prefix 下的候选 bin 路径（P2「安装后记录位置」用）。
    ///
    /// 默认（Unix）：`<prefix>/bin/<name>`。
    /// Windows 覆写：npm 垫片直接在 prefix 下（`<prefix>\<name>.cmd`），
    ///   以及包内真实脚本 `<prefix>\node_modules\<pkg>\bin\<name>`。
    ///
    /// 为什么需要：内核由 npm 装进 **npm 全局 prefix**（nvm/volta/fnm/自定义），
    ///   「装完立刻回读确切位置并写入 core.json」必须有跨平台的路径推导，
    ///   不能在业务层写平台分支（门禁 G1）。
    fn core_bin_candidates_in_prefix(
        &self,
        prefix: &std::path::Path,
        names: &[&str],
        _pkg: Option<&str>,
    ) -> Vec<std::path::PathBuf> {
        names.iter().map(|n| prefix.join("bin").join(n)).collect()
    }

    /// **安装后** Node 可执行文件应出现的位置（用于校验安装成功）。
    fn node_bin_after_install(&self) -> std::path::PathBuf;

    /// 该文件是否是**可用的可执行候选**。
    ///
    /// Windows 必须过滤两类伪可执行：
    ///   · `\WindowsApps\` 下的**应用执行别名存根**（执行它会挂起或唤起 Store）；
    ///   · 0 字节文件。
    /// 其它平台只判存在性。
    fn is_usable_executable(&self, cand: &std::path::Path) -> bool;

    /// 产品状态根的**平台默认**（不含 `DSH_SUPERVISOR_HOME` 覆盖；env.rs 负责覆盖）。
    ///   Linux/Unix：`$XDG_STATE_HOME/dsh-supervisor` 或 `~/.local/state/dsh-supervisor`
    ///   macOS：`~/Library/Application Support/dsh-supervisor`
    ///   Windows：`%LOCALAPPDATA%\dsh-supervisor`
    /// **独立于 DSH 的 ~/.dsh** —— 我们是管控 DSH 的产品，状态不得寄在其目录下。
    fn state_root_default(&self) -> std::path::PathBuf {
        if let Some(x) = std::env::var_os("XDG_STATE_HOME") {
            if !x.is_empty() {
                return std::path::Path::new(&x).join("dsh-supervisor");
            }
        }
        home_dir().join(".local").join("state").join("dsh-supervisor")
    }

    /// **安装 Node**（**用户级，零权限**；2026-09-18 权限模型重写）。
    ///
    /// 三平台统一：把官方归档解到 `<状态根>/node`，再原子替换。
    /// · Linux/macOS `tar -xzf <archive> -C <staging> --strip-components=1`
    /// · Windows     `powershell Expand-Archive`
    ///
    /// 为什么不再提权：系统级安装（MSI/pkg//usr/local）需要 UAC/pkexec/sudo，
    ///   而 UAC 提升到管理员账户后常读不到当前用户 profile 下的安装包（msiexec 1619）、
    ///   容器/WSL/SSH 常无可用 polkit agent 或 sudo。用户级解包在**任何**权限下都能成功。
    /// 提权只与**壳自更新**（替换安装程序）有关，由各平台自身通道完成。
    fn install_node(&self, file: &std::path::Path) -> Result<std::path::PathBuf, String>;

    /// 是否存在可用的**提权通道**（用于「不可自更新」的提前判定）。
    ///
    /// **不主动执行提权**，只探测命令存在性。
    fn has_privilege_channel(&self) -> bool;

    // ── 可执行**文件名**的平台差异（P2/G1 修复，2026-09-12）──
    //
    // 为什么这三个要进 trait：它们此前以 `cfg!()` 形式散落在 env.rs / core.rs /
    //   domain/coreloc.rs —— 而门禁 G1 只拦 `#[cfg(` **属性**，看不见 `cfg!()` **宏**，
    //   于是「平台分支只在 platform/」这条约束被绕过（三处生产代码在 platform/ 之外）。
    //
    // 语义差别也重要：`cfg!` **不做条件编译裁剪**，两个分支都参与类型检查，
    //   平台知识会随调用点扩散；`#[cfg]` 属性则按目标平台裁剪。
    //   进 trait 后由各平台文件实现，语义与 G1 的意图一致。

    /// Node 可执行**文件名**（Windows `node.exe` / 其余 `node`）。
    fn node_exe_name(&self) -> &'static str;

    /// npm 可执行**文件名**（Windows `npm.cmd` / 其余 `npm`）。
    ///
    /// 与内核侧 `platform/os/exec-path.js::npmBin()` 是**同一事实的两端**：
    ///   Windows 上 npm 是 `.cmd`，Node 的 spawn 不做 PATHEXT 解析（P1-C 已修）。
    fn npm_exe_name(&self) -> &'static str;

    /// 内核可执行文件的**候选名**（Windows 含 `.exe`/`.cmd` 垫片）。
    fn core_exe_names(&self) -> &'static [&'static str];
}

// ── 平台实现的选择：**本文件是全仓唯一出现平台分支的地方**（门禁 G1）──
//
// 每个平台一个独立文件（而非同一个文件里的 #[cfg] 块）：
//   · 加一个平台 = 加一个文件 + 在下面加一行，**不碰任何既有代码**；
//   · 平台代码无法互相污染（各自的 use / 私有助手完全隔离）；
//   · 未知平台显式落到 `unsupported`，而不是编译失败或静默成功。

#[cfg(target_os = "linux")]
pub(crate) mod linux;
#[cfg(target_os = "linux")]
pub(crate) use linux as imp;

#[cfg(target_os = "macos")]
pub(crate) mod macos;
#[cfg(target_os = "macos")]
pub(crate) use macos as imp;

#[cfg(target_os = "windows")]
pub(crate) mod windows;
#[cfg(target_os = "windows")]
pub(crate) use windows as imp;

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub(crate) mod unsupported;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub(crate) use unsupported as imp;

/// 当前平台。
pub fn current() -> &'static dyn Platform {
    imp::platform()
}

/// 当前平台的服务控制（**定义与启停的统一入口**）。
pub fn service() -> &'static dyn service::ServiceControl {
    imp::service()
}

// （`name()` 自由函数已移除：经 `current().name()` 使用 trait 方法即可，
//  保留一层多余包装只会成为 dead_code 噪声。）

/// 当前平台能力声明。
pub fn capabilities() -> Capabilities {
    current().capabilities()
}


/// 在**版本化安装目录**（形如 `v22.12.0` / `22.12.0`）中挑版本最高的一个，
/// 返回其中可执行文件的路径。
///
/// 三处版本管理器布局不同，故用 `suffix` 参数表达差异：
///   · nvm (Unix)   `<root>/<ver>/bin/node`            → suffix = ["bin", "node"]
///   · fnm (Unix)   `<root>/<ver>/installation/bin/node` → suffix = ["installation", "bin", "node"]
///   · nvm (Windows) `<root>/<ver>/node.exe`            → suffix = ["node.exe"]
///
/// 含 `read_dir`（可能落在漫游配置/慢速盘上）—— 调用方必须先 `stage()` 上报。
pub fn latest_versioned_node(root: &std::path::Path, suffix: &[&str]) -> Option<std::path::PathBuf> {
    let rd = std::fs::read_dir(root).ok()?;
    let mut best: Option<(Vec<u64>, std::path::PathBuf)> = None;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let trimmed = name.trim_start_matches('v');
        let nums: Vec<u64> = trimmed.split('.').map_while(|x| x.parse::<u64>().ok()).collect();
        if nums.is_empty() {
            continue;
        }
        let mut full = e.path();
        for seg in suffix {
            full = full.join(seg);
        }
        let better = match &best {
            None => true,
            Some((bv, _)) => nums > *bv,
        };
        if better {
            best = Some((nums, full));
        }
    }
    best.map(|(_, p)| p)
}

/// `--platform-matrix` 自检输出（无头可跑，供三平台 CI 比对）。
///
/// 目的：把「平台矩阵」从**文档承诺**变成**可执行断言** ——
///   文档说支持某能力但代码没实现，这一输出会在 CI 上暴露。
pub fn matrix_text() -> String {
    let c = capabilities();
    // 经 trait 对象取（而非仅用自由函数）：确保 `Platform::name` / `Platform::service`
    // 这两个契约方法**真的被调用过** —— 否则它们只是「声明了但没用」，
    // 与本项目反复出现的「注释声称、代码没有」是同一类风险。
    let plat = current();
    let svc = plat.service();
    let def = svc.definition_path();
    [
        format!("platform={}", plat.name()),
        format!("platform_field={}", c.platform),
        format!("native_service={}", c.native_service),
        format!("service_kind={}", svc.kind()),
        format!("privilege_channel={}", c.privilege_channel),
        format!("node_artifact={}", c.node_artifact),
        format!("definition_path={}", def.display()),
        format!("definition_exists={}", def.is_file()),
    ]
    .join(" | ")
}

#[cfg(test)]
mod tests {
    /// 平台标签契约（2026-09-14）：`core_platform_tag()` 必须与**当前构建 target** 一致。
    /// 在 CI 的 4 个 runner（linux-x64 / win-x64 / darwin-arm64 / darwin-x64）上各跑一次，
    /// 把「OS × ARCH → 内核包标签」这条跨平台事实钉死。
    #[test]
    fn core_platform_tag_matches_build_target() {
        let got = super::current().core_platform_tag();
        let want = match (std::env::consts::OS, std::env::consts::ARCH) {
            ("linux", "x86_64") => Some("linux-x64"),
            ("linux", "aarch64") => Some("linux-arm64"),
            ("macos", "x86_64") => Some("darwin-x64"),
            ("macos", "aarch64") => Some("darwin-arm64"),
            ("windows", "x86_64") => Some("win-x64"),
            ("windows", "aarch64") => Some("win-arm64"),
            _ => None,
        };
        assert_eq!(got, want, "core_platform_tag 必须与构建 target 一致");
    }
}

#[cfg(test)]
mod launch_spec_tests {
    //! 行为门禁：服务定义只指向**稳定入口** `<壳> --run-guard`（2026-09-15 架构修正）。
    use super::*;

    fn spec_with(shell: &str) -> LaunchSpec {
        LaunchSpec {
            node: std::path::PathBuf::from("/NODE SENTINEL/node"),
            guard: std::path::PathBuf::from("/GUARD SENTINEL/guard"),
            env_path: String::new(),
            state_root: std::path::PathBuf::from("/state"),
            shell: std::path::PathBuf::from(shell),
        }
    }

    #[test]
    fn service_command_is_shell_run_guard() {
        let s = spec_with("/opt/x/dsh-supervisor");
        let (shell, args) = s.service_command();
        assert_eq!(shell, std::path::Path::new("/opt/x/dsh-supervisor"));
        assert_eq!(args, &["--run-guard"]);
    }

    #[test]
    fn service_exec_line_quotes_shell_and_has_no_volatile_paths() {
        let s = spec_with("/home/John Smith/dsh-supervisor");
        let (shell, args) = s.service_command();
        let line = service_exec_line(shell, args);
        assert_eq!(line, "\"/home/John Smith/dsh-supervisor\" --run-guard");
        assert!(!line.contains("NODE SENTINEL") && !line.contains("GUARD SENTINEL"),
            "服务定义不得含 node/guard 路径：{}", line);
    }
}