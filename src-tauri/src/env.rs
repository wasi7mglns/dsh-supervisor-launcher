//! 环境探测：Node 定位 / 版本 / 产品状态根。
//!
//! == 为什么本文件里的子进程必须先经 `bounded::prepare`（2026-09-16） ==
//!
//! 壳在 release 下是 **GUI 子系统**（见 main.rs 顶部 `windows_subsystem = "windows"`），
//!   自身不创建控制台；但 Windows 上子进程**默认会新建/继承控制台** ——
//!   一个 GUI 进程去拉起控制台程序（node.exe 等）时，若不带 `CREATE_NO_WINDOW`，
//!   系统就会为它弹出一个黑色控制台窗口。
//! 本文件的 `node_version()` 每探测一个 Node 候选就执行一次，于是表现为
//!   「引导期反复闪黑框」（NO-CONSOLE-WINDOW-STANDARD §1 记录的 S-W2 缺口）。
//!
//! `bounded::prepare` 是 `CREATE_NO_WINDOW` 的**唯一封装点**：平台分支只允许出现在
//!   `bounded.rs` 与 `platform/`（门禁 G1），所以这里**只能调用它**，
//!   不得在本文件写 `#[cfg(windows)]` + `creation_flags`。
//! 为什么复用 prepare 而不改成 `bounded::run`：run 是「阻塞式收集输出 + 超时」，
//!   而本函数必须保留「spawn 后轮询 try_wait 并自行计时」的既有非阻塞行为
//!   （Windows 的 Store 别名存根会挂起，任何阻塞式等待都会把调用方拖死），
//!   故只借用它「加标志」的能力，执行结构保持不变。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// Node 版本探测的时间上限。
///
/// Windows 的 `WindowsApps\node.exe` 是 Store 应用执行别名存根，执行会挂起 —— 必须有界。
const NODE_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// 兼容包装 probe_system_node 的有界预算（真正执行在 nodeprobe 的分离线程里）。
const NODE_PROBE_TOTAL_BUDGET: Duration = Duration::from_secs(20);

/// Node 可执行名 —— 平台知识已下沉到 trait（P2/G1：原为 `cfg!()` 宏，门禁 G1 只拦 `#[cfg(` 属性，看不见它）。
pub fn node_exe() -> &'static str {
    crate::platform::current().node_exe_name()
}

/// 候选是否可用（平台判定）。
///
/// 实现已下沉到 platform 层（2026-09-11）。Windows 必须过滤两类**伪可执行**：
///   · `\WindowsApps\` 下的应用执行别名存根（执行它会挂起或唤起 Store）；
///   · 0 字节文件。
pub fn is_usable_candidate(cand: &Path) -> bool {
    crate::platform::current().is_usable_executable(cand)
}

/// PATH 扫描的总预算：即使 PATH 里存在会阻塞的路径，也不会把调用方拖成分钟级。
/// 说明：这是「快速失败」，不是唯一防线 —— nodeprobe 的分离线程机制才是硬保证。
const PATH_SCAN_BUDGET: Duration = Duration::from_secs(10);

/// PATH 扫描（**有界 + 安全过滤**）。
///
/// 预算 PATH_SCAN_BUDGET；Windows 跳过非固定盘与 UNC（本地判定，不触网）。
pub fn find_in_path(name: &str) -> Option<PathBuf> {
    let started = Instant::now();
    for dir in path_dirs_local_only() {
        if started.elapsed() >= PATH_SCAN_BUDGET { break; }
        let cand = dir.join(name);
        if is_usable_candidate(&cand) { return Some(cand); }
    }
    None
}

/// PATH 中的目录，已过滤掉「可能阻塞」的项（非固定盘 / UNC）。
/// 供 nodeprobe 与 find_in_path 共用，保证探测与定位走同一套安全判定。
pub fn path_dirs_local_only() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Some(path) = std::env::var_os("PATH") else { return out };
    for dir in std::env::split_paths(&path) {
        if !is_local_fixed_dir(&dir) { continue; }
        out.push(dir);
    }
    out
}

/// 该目录是否位于本地固定盘（平台判定；在触网操作之前完成，按盘符缓存 GetDriveTypeW）。
pub fn is_local_fixed_dir(dir: &Path) -> bool {
    crate::platform::current().is_local_fixed_dir(dir)
}

pub fn recorded_node_path() -> Option<PathBuf> {
    let p = supervisor_dir().join("runtime.json");
    let s = std::fs::read_to_string(&p).ok()?;
    let v: serde_json::Value = serde_json::from_str(&s).ok()?;
    let node = v.get("nodePath").and_then(|x| x.as_str())?;
    if node.is_empty() { return None; }
    let cand = PathBuf::from(node);
    if is_usable_candidate(&cand) { Some(cand) } else { None }
}

/// 有界执行 `<node> --version`。
/// 超时/失败一律返回 None（视为「不可用」），**绝不阻塞调用方** ——
/// 这是「引导页不会因某个坏的可执行文件而永久卡住」的根本保证。
pub fn node_version(node: &Path) -> Option<String> {
    use std::process::Stdio;
    // 先建命令再显式 prepare：不加该标志时，GUI 子系统下的每次探测都会弹控制台窗口。
    let mut cmd = Command::new(node);
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // 与 bounded::run 同源：CREATE_NO_WINDOW 只在此封装点加（平台分支不得外溢到本文件）。
    crate::bounded::prepare(&mut cmd);
    let mut child = cmd.spawn().ok()?;
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {
                if start.elapsed() >= NODE_PROBE_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() { return None; }
    let mut s = String::new();
    if let Some(mut out) = child.stdout.take() {
        use std::io::Read;
        let _ = out.read_to_string(&mut s);
    }
    let s = s.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// 系统 PATH 中的 Node：缺失返回 None。
///
/// 必须**遍历全部候选**而非取第一个：PATH 靠前的候选可能是不可用的存根，
///   若直接返回它就会掩盖后面真正可用的 Node 安装。
/// 兼容入口：委托给**有界探测运行时**（nodeprobe）。
///
/// 架构说明（2026-09-11 二次修复）：原先本函数自行扫描 PATH 并执行候选二进制，
/// 而它内含无法被自身预算约束的阻塞系统调用（见 env.rs 顶部与 nodeprobe.rs 的根因分析），
/// 且被 setup() 同步调用 —— 一旦挂起，窗口创建都会被推迟。
/// 现统一走 nodeprobe：分离线程执行 + 有界等待 + 结果缓存 + 全程追踪。
/// 保留本函数是为了让所有既有调用点**自动**获得该保证，无需逐个改写。
pub fn probe_system_node() -> Option<(PathBuf, String)> {
    crate::nodeprobe::resolve(NODE_PROBE_TOTAL_BUDGET)
}

/// 安装后已知候选路径（官方安装的标准落点）。
///
/// Windows 不得硬编码 `C:\Program Files`（2026-09-11 审计）：
///   真实路径随**系统盘符**与**系统语言**变化（中文系统是 `Program Files` 的本地化目录名），
///   也可能装在 `Program Files (x86)`。故一律经 `ProgramFiles` / `ProgramFiles(x86)`
///   环境变量推导 —— 这也是 `nodeprobe::known_locations()` 采用的口径，两处必须一致。
/// 用户级 Node 安装根（**零权限**）：<状态根>/node。
///
/// 为什么是用户级（2026-09-18 权限模型重写）：系统级安装（Windows MSI / macOS pkg /
///   Linux /usr/local）都需要提权；而 Windows 上 UAC 提升到管理员账户后，常读不到
///   当前用户 profile 下的安装包（msiexec 退出码 1619 = 安装包无法打开）。用户级归档
///   解包三平台一致、**完全不需要授权**，也永不产生跨账户路径不可读问题。
pub fn node_install_root() -> PathBuf {
    state_root().join("node")
}

/// **安装后** Node 可执行文件应出现的位置（平台判定；用于校验安装成功）。
///
/// 实现已下沉到 platform 层（2026-09-11）。
/// Windows 不得硬编码 `C:\Program Files`：真实路径随**系统盘符**与
///   **系统语言**变化（中文系统是本地化目录名），也可能装在 `Program Files (x86)`，
///   故一律经 `ProgramFiles` / `ProgramFiles(x86)` 环境变量推导。
pub fn known_install_node_path() -> Option<PathBuf> {
    let p = crate::platform::current().node_bin_after_install();
    if p.is_file() {
        Some(p)
    } else {
        None
    }
}

/// 产品状态根 schema（与内核 src/platform/state-root.js 的 SCHEMA 握手；门禁锁定）。
pub const STATE_ROOT_SCHEMA: u32 = 1;

/// 产品状态根（**独立于 DSH 的 ~/.dsh**）：`DSH_SUPERVISOR_HOME` 覆盖，否则平台默认。
///
/// 为什么独立：本产品**管控** DSH，把状态放在被管控对象的 ~/.dsh 下是概念错位 ——
///   DSH 卸载/清理/迁移数据目录会把我们的 config/state/ports/logs 一并带走。
pub fn state_root() -> PathBuf {
    if let Ok(v) = std::env::var("DSH_SUPERVISOR_HOME") {
        if !v.trim().is_empty() {
            return PathBuf::from(v.trim());
        }
    }
    crate::platform::current().state_root_default()
}

/// 内核状态目录（config/state/ports/logs/契约）：<状态根>/supervisor。
pub fn supervisor_dir() -> PathBuf {
    state_root().join("supervisor")
}

/// 桌面壳状态目录（identity/mirrors/shell.log）：<状态根>/shell。
pub fn shell_dir() -> PathBuf {
    state_root().join("shell")
}

/// 前向自愈迁移：旧位置（DSH 数据目录下）→ 产品状态根，按条目合并（不覆盖已存在文件）。
/// 在壳启动早期调用一次；失败不阻断（下次启动再试）。
pub fn migrate_legacy() {
    let home = home();
    let root = state_root();
    for (from, to) in [
        (home.join(".dsh").join("supervisor"), root.join("supervisor")),
        (home.join(".dsh").join("shell"), root.join("shell")),
    ] {
        if !from.is_dir() {
            continue;
        }
        let _ = std::fs::create_dir_all(&to);
        if let Ok(entries) = std::fs::read_dir(&from) {
            for e in entries.flatten() {
                let dst = to.join(e.file_name());
                if dst.exists() {
                    continue;
                }
                let _ = std::fs::rename(e.path(), &dst);
            }
        }
        let _ = std::fs::remove_dir(&from);
    }
}

/// 读取并解析守卫配置 <产品状态根>/supervisor/config.json（serde_json，**壳读内核配置的唯一解析入口**）。
/// ⚠ 路径已迁离 DSH 数据目录（旧注释写的 `~/.dsh/supervisor/…` 是过时说法，见 state_root()）。
/// 契约 ARCHITECTURE-CONTRACT-phase0 §3.5：一律真 JSON 解析，禁止字符串扫描
/// （格式微调——空白/换行/转义差异——即会让扫描失效；此前 apiPort/closeAction 各有一份扫描实现）。
fn config_json() -> Option<serde_json::Value> {
    std::fs::read_to_string(supervisor_dir().join("config.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
}

/// 守卫本地 API 基址：读 config.json 的 apiPort（用户可改），失败/缺失回退默认端口。
/// 壳极少更新但内核配置可演进——硬编码会让改过 apiPort 的用户导航到死端口（F7）。
pub fn api_base_url() -> String {
    // 默认端口来自**唯一常量**（见 DEFAULT_API_PORT）—— 此前这里是字面量 36360，
    // 而 api_port() 的回退是另一个字面量 3100，同一事实两处默认。
    let default_port = DEFAULT_API_PORT;
    let port = config_json()
        .and_then(|v| v.get("apiPort").and_then(|x| x.as_u64()))
        .filter(|n| *n > 0 && *n <= u16::MAX as u64)
        .map(|n| n as u16)
        .unwrap_or(default_port);
    format!("http://127.0.0.1:{}/", port)
}

/// 读守卫 config.json 里的一个**布尔开关**（缺失/非布尔/解析失败 = false）。
///
/// 为什么单独抽出来：契约 §5 的灰度开关（`canary: true`）与灰度名单 opt-in
/// （`canaryAllowlist: true`）都是`config.json`里的普通字段，而`config_json`是**私有**的
/// —— 若各自再写一遍"读文件 + 解析"，就又出现第二份`config.json`解析实现（契约 §3.5 明令禁止）。
/// 判定规则**只认真 JSON 布尔 true**（字符串 `"true"` 不算）：避免手改配置时把
/// 灰度机悄悄变成灰度机（少读一位 = 稳定版用户被装上灰度版）。
pub fn config_flag(key: &str) -> bool {
    config_json()
        .and_then(|v| v.get(key).and_then(|x| x.as_bool()))
        .unwrap_or(false)
}

/// 关闭窗口时的行为（读守卫 config.closeAction；'exit'=退出管家全关，其余=隐藏至托盘）。
pub fn close_action() -> String {
// 真 JSON 解析（契约 §3.5）。语义：exit=退出管家；其余（含缺失/解析失败）= 隐藏至托盘。
    // serde_json 已是壳依赖（见 Cargo.toml），零新增依赖。
    config_json()
        .and_then(|v| v.get("closeAction").and_then(|x| x.as_str()).map(|s| s.to_string()))
        .filter(|v| v == "exit" || v == "hide")
        .unwrap_or_else(|| "hide".into())
}

/// 守卫 API 的**默认端口**（单一事实源）。
///
/// 原有两个默认端口（36360 vs 3100）且已分叉；现两者共用本常量（单一事实源）。
pub const DEFAULT_API_PORT: u16 = 36360;

/// 壳可用性探测用守卫端口（与 api_base_url 同源解析）。
pub fn api_port() -> u16 {
    let u = api_base_url();
    // 从派生出的 URL 反解端口；回退到**同一个**默认端口常量（而非另一个字面量）。
    u.trim_end_matches('/')
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_API_PORT)
}

/// 内核持久化的**实际** API 端口（ports.json 的 supervisor-api 记录）。
///
/// 为什么必须读实际值：内核在 EADDRINUSE 时会自动顺延端口并持久化
///   （supervisor.js 的端口自动避让）。只认 config.json 的期望值会让壳
///   永远等一个没人监听的端口 —— 表现为「守卫启动失败」，即使守卫已健康运行。
pub fn discovered_api_port() -> Option<u16> {
    let s = std::fs::read_to_string(supervisor_dir().join("ports.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&s).ok()?;
    for r in v.get("records")?.as_array()? {
        if r.get("role").and_then(|x| x.as_str()) != Some("supervisor-api") {
            continue;
        }
        if let Some(port) = r.get("port").and_then(|x| x.as_u64()) {
            if port > 0 && port <= u16::MAX as u64 {
                return Some(port as u16);
            }
        }
    }
    None
}

/// 当前应使用的内核 API 端口：**实际绑定值优先**，退回配置期望值（单一入口）。
pub fn current_api_port() -> u16 {
    discovered_api_port().unwrap_or_else(api_port)
}

pub fn home() -> PathBuf {
    std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}


#[cfg(test)]
mod tests {
    //! A-3 门禁：守卫 API 的「默认端口」必须是**单一事实源**（2026-09-13）。
    //!
    //! 缺陷：api_base_url() 的默认是 36360，而 api_port() 的回退是 3100 ——
    //! 同一事实两个默认值且已分叉。正常路径下 api_port 由 api_base_url 解析而来，
    //! 故 3100 那条回退**不可达**；但它是潜伏的第二默认：一旦 api_base_url 的返回
    //! 形态变化（或有人改动解析），就会回退到一个早就退役的端口，
    //! 现象是「守卫未运行」而非「端口取错」，排错方向被带偏。
    //!
    //! 断言的是「只有一个默认端口字面量」+「回退指向那个常量」——
    //! 这正是该缺陷的判据。针脚在**运行时拼出**，避免命中本测试自己的源码。
    use super::*;

    #[test]
    fn a3_default_api_port_is_single_source() {
        let raw = include_str!("env.rs");
        // 剥离注释：本仓两次被自己写的说明文字骗过（AUDIT-HANDOFF 9.2）
        let code: String = raw
            .lines()
            .map(|l| if l.trim_start().starts_with("//") { "" } else { l })
            .collect::<Vec<_>>()
            .join("\n");

        // 36460 与 3100 在测试源码里**不得以字面量出现**，否则自匹配。
        //   针脚在运行时拼出，且**注释里也不写**那两个字面量 ——
        //   本测试源码经 include_str! 被读入，任何逐字出现都会自匹配。
        let expected_default = (3636 * 10).to_string(); // 默认端口 = 3636 乘 10
        let old_stale_port = (31 * 100).to_string();    // 旧端口 = 31 乘 100

        assert_eq!(
            code.matches(&expected_default).count(),
            1,
            "A-3 FAIL 默认端口字面量出现 {} 次（应为 1 —— 单一事实源）",
            code.matches(&expected_default).count()
        );
        assert!(
            !code.contains(&old_stale_port),
            "A-3 FAIL 仍存在旧端口 {} 的回退 —— 同一事实两个默认",
            old_stale_port
        );
        // 正向：回退必须指向那个常量
        let api_port = code.split("pub fn api_port").nth(1).expect("A-3 FAIL 未找到 api_port");
        let body = api_port.split("\n}").next().unwrap_or(api_port);
        assert!(
            body.contains("unwrap_or(DEFAULT_API_PORT)"),
            "A-3 FAIL api_port 回退未指向 DEFAULT_API_PORT"
        );
        assert!(
            body.contains("api_base_url()"),
            "A-3 FAIL api_port 未与 api_base_url 同源"
        );
    }

    /// A-3（行为）：默认端口常量确实等于契约里的高位段起始，且 url 由它派生。
    #[test]
    fn a3_default_port_constant_value_and_url_derivation() {
        assert_eq!(DEFAULT_API_PORT, 3636 * 10, "A-3 FAIL 默认端口常量值被改动");
        // 在一个**空 HOME** 下：无 config.json → 必须落回 DEFAULT_API_PORT。
        // 用锁串行，避免与其它读 HOME 的用例并发互踩。
        static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let saved = std::env::var_os("HOME");
        let dir = std::env::temp_dir().join(format!("dsh-a3-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("HOME", &dir);
        let url = api_base_url();
        let port = api_port();
        match &saved {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            url,
            format!("http://127.0.0.1:{}/", DEFAULT_API_PORT),
            "A-3 FAIL api_base_url 未按默认常量生成"
        );
        assert_eq!(port, DEFAULT_API_PORT, "A-3 FAIL api_port 与默认端口不一致");
    }
}
