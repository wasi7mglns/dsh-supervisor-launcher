//! 平台适配层 —— **全仓唯一的平台分支所在地**（2026-09-11）
//!
//! == 为什么需要它（用户指正：「壳是乱的，没有架构」）==
//!
//! 改造前实测：平台分支 **43 处散落在 8 个文件**（`node.rs` 11 / `main.rs` 10 /
//! 原 `service.rs` 9 / `env.rs` 6 / `bounded.rs` 2 / `nodeprobe.rs` 2 / `update.rs` 2 / `core.rs` 1）。
//! 加一个平台要翻 8 个文件；查一个平台 bug 要猜它在哪一层。
//!
//! 对照：内核（JS）把平台判断集中在 `platform/os/`（2026-09 实测约九成）——
//! 即**内核有这个层，而壳没有**。本模块就是补齐它。
//!
//! ⚠ 此处**刻意不写精确数字**：它是另一个仓的当前状态，会随内核演进而漂移。
//!   要精确值请直接数内核的 `platform/os/`（本仓的任何数字都只是快照）。
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
/// ⚠ 放在平台层而非业务层：它是**平台事实**（环境变量名不同），
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

/// 平台能力声明（供 `--platform-matrix` 自检与诊断，**不参与业务逻辑**）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct Capabilities {
    pub platform: &'static str,
    /// 是否有**原生**服务管理器（systemd / launchd / 计划任务）。
    pub native_service: bool,
    /// 是否有提权通道（Node 安装需要）。
    pub privilege_channel: bool,
    /// Node 制品形态（`tar.xz` / `pkg` / `msi`）。
    pub node_artifact: &'static str,
}

/// Node 官方制品的描述（**平台层解析**，业务层只消费）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeArtifact {
    /// `index.json` 的 `files[]` 标签（如 `linux-x64` / `osx-x64-pkg` / `win-x64-msi`）。
    pub tag: &'static str,
    /// 发布文件名（含版本号），如 `node-v22.12.0-linux-x64.tar.xz`。
    pub file: String,
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
    ///   ⚠ 后者不可省：`.cmd` 垫片无法被 `package_dir_of` 解析（父目录不是 `bin/`），
    ///   直接给出包内路径既能正确读 `package.json` 取版本，也能让前缀推导正常。
    /// · macOS   —— `/opt/homebrew/bin`（Apple Silicon）、`/usr/local/bin`（Intel）
    /// · Linux   —— 空（PATH 与 ~/.local/bin 已覆盖）
    ///
    /// `pkg` 为内核包名（如 `@dsh-sup/dsh-core-linux-x64`），用于 Windows 包内路径。
    fn core_extra_candidates(&self, names: &[&str], pkg: Option<&str>) -> Vec<std::path::PathBuf>;

    /// **安装后** Node 可执行文件应出现的位置（用于校验安装成功）。
    fn node_bin_after_install(&self) -> std::path::PathBuf;

    /// 该文件是否是**可用的可执行候选**。
    ///
    /// Windows 必须过滤两类伪可执行：
    ///   · `\WindowsApps\` 下的**应用执行别名存根**（执行它会挂起或唤起 Store）；
    ///   · 0 字节文件。
    /// 其它平台只判存在性。
    fn is_usable_executable(&self, cand: &std::path::Path) -> bool;

    /// **安装 Node**（含平台提权通道）。
    ///
    /// · Linux   `pkexec sh -c "tar -xJf … -C /usr/local"`
    /// · macOS   `osascript` + `installer -pkg … -target /`（带管理员授权）
    /// · Windows `powershell Start-Process msiexec … -Verb RunAs -Wait`
    ///
    /// ⚠ 这是**壳独有**的能力：装内核之前必须先把运行环境装好（引导顺序），
    ///   而提权需要人在场 —— 内核（无头服务）永远做不到这件事。
    fn install_node(&self, file: &std::path::Path) -> Result<std::path::PathBuf, String>;

    /// 是否存在可用的**提权通道**（用于「不可自更新」的提前判定）。
    ///
    /// **不主动执行提权**，只探测命令存在性。
    fn has_privilege_channel(&self) -> bool;
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
/// ⚠ 含 `read_dir`（可能落在漫游配置/慢速盘上）—— 调用方必须先 `stage()` 上报。
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