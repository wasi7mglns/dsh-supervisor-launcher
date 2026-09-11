use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// Node 版本探测的时间上限。
///
/// ⚠ 为什么必须有（2026-09-11 用户 Windows 真机事故：引导卡在「检测系统环境」）：
///   Windows 的 PATH 默认包含 `%LOCALAPPDATA%\Microsoft\WindowsApps`，其中的 `node.exe`
///   是**应用执行别名存根**（AppExecLink 重解析点，指向 Microsoft Store），并非真实 Node。
///   原实现 `find_in_path` 用 `is_file()` 判定即选中它，而 `Command::output()` 无超时 ——
///   执行该存根会尝试唤起 Store 并**永不返回**，引导页从此永久停住。
const NODE_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// 兼容包装 probe_system_node 的有界预算（真正执行在 nodeprobe 的分离线程里）。
const NODE_PROBE_TOTAL_BUDGET: Duration = Duration::from_secs(20);

pub fn node_exe() -> &'static str {
    if cfg!(windows) { "node.exe" } else { "node" }
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
/// ⚠ 两处工程性修正（2026-09-11 架构修复）：
///   1. 原实现**没有任何时间上限** —— PATH 中若含断开的网络盘或 UNC，一次 is_file()
///      就可能阻塞数十秒；而本函数在**核心定位路径**上被调用（无 GUI 的 CLI 自检同样受影响）。
///   2. 增加磁盘类型过滤：Windows 上跳过非固定盘与 UNC。GetDriveTypeW 是**本地**判定，
///      不会像 exists()/metadata() 那样触网，故对断开的映射盘也安全。
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

/// 该目录是否位于「本地固定磁盘」。
///
/// 为什么需要：Windows 的 PATH 常含映射盘或 UNC；当网络盘断开时，
/// GetFileAttributesW / CreateProcessW 会阻塞到 SMB 超时（数十秒，且可能重试）。
/// 本判定在**执行任何可能触网的操作之前**完成，且自身不触网。
///
/// ⚠ **按盘符记忆（2026-09-11 二次修复）**：
///   `GetDriveTypeW` 对**断开的网络驱动器**可能阻塞（微软文档明确提示该 API 可能慢）。
///   原实现**对每个 PATH 条目都调一次** —— PATH 里数十个条目往往集中在同一两个盘符，
///   于是同一个盘被反复查询，一旦该盘有问题就重复付出阻塞代价。
///   现按「盘符」缓存：最多 26 次查询（且每个盘符只查一次）。
/// 该目录是否位于**本地固定盘**（平台判定）。
///
/// 实现已下沉到 platform 层（2026-09-11）。调用方在引导的**关键路径**上，
/// 而 `Path::is_file()` 在断开的映射盘/UNC 上会触网阻塞数十秒。
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
    let mut child = Command::new(node)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
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
/// ⚠ 必须**遍历全部候选**而非取第一个：PATH 靠前的候选可能是不可用的存根，
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
/// ⚠ Windows 不得硬编码 `C:\Program Files`（2026-09-11 审计）：
///   真实路径随**系统盘符**与**系统语言**变化（中文系统是 `Program Files` 的本地化目录名），
///   也可能装在 `Program Files (x86)`。故一律经 `ProgramFiles` / `ProgramFiles(x86)`
///   环境变量推导 —— 这也是 `nodeprobe::known_locations()` 采用的口径，两处必须一致。
/// **安装后** Node 可执行文件应出现的位置（平台判定；用于校验安装成功）。
///
/// 实现已下沉到 platform 层（2026-09-11）。
/// ⚠ Windows 不得硬编码 `C:\Program Files`：真实路径随**系统盘符**与
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

pub fn supervisor_dir() -> PathBuf {
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".dsh").join("supervisor")
}

/// 读取并解析守卫配置 ~/.dsh/supervisor/config.json（serde_json，**壳读内核配置的唯一解析入口**）。
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
    let default_port = 36360u16; // 高位段起始（3100 常用端口易冲突，动态端口 2026-09-07）
    let port = config_json()
        .and_then(|v| v.get("apiPort").and_then(|x| x.as_u64()))
        .filter(|n| *n > 0 && *n <= u16::MAX as u64)
        .map(|n| n as u16)
        .unwrap_or(default_port);
    format!("http://127.0.0.1:{}/", port)
}

/// 关闭窗口时的行为（读守卫 config.closeAction；'exit'=退出管家全关，其余=隐藏至托盘）。
pub fn close_action() -> String {
    // 契约 ARCHITECTURE-CONTRACT-phase0 §3.5：**真正的 JSON 解析**（旧实现用字符串扫描，
    // config.json 只要出现空白/换行/转义差异即解析失效——例如格式化写入后键值间无空格）。
    // 语义不变：'exit' = 退出管家（停全部服务）；其余（含缺失/解析失败/非法值）= 隐藏至托盘。
    // serde_json 已是壳依赖（见 Cargo.toml），零新增依赖。
    config_json()
        .and_then(|v| v.get("closeAction").and_then(|x| x.as_str()).map(|s| s.to_string()))
        .filter(|v| v == "exit" || v == "hide")
        .unwrap_or_else(|| "hide".into())
}

/// 壳可用性探测用守卫端口（与 api_base_url 同源解析）。
pub fn api_port() -> u16 {
    let u = api_base_url();
    u.trim_end_matches('/').rsplit(':').next().and_then(|p| p.parse().ok()).unwrap_or(3100)
}

pub fn home() -> PathBuf {
    std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}


