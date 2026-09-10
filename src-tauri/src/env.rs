use std::path::{Path, PathBuf};
use std::process::Command;

pub fn node_exe() -> &'static str {
    if cfg!(windows) { "node.exe" } else { "node" }
}

pub fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if cand.is_file() { return Some(cand); }
    }
    None
}

pub fn node_version(node: &Path) -> Option<String> {
    let out = Command::new(node).arg("--version").output().ok()?;
    if out.status.success() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !s.is_empty() { return Some(s); }
    }
    None
}

/// 系统 PATH 中的 Node：缺失返回 None。
pub fn probe_system_node() -> Option<(PathBuf, String)> {
    let exe = find_in_path(node_exe())?;
    let v = node_version(&exe)?;
    Some((exe, v))
}

/// 安装后已知候选路径（官方安装的标准落点）。
pub fn known_install_node_path() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    { let p = PathBuf::from("/usr/local/bin/node"); if p.is_file() { return Some(p); } }
    #[cfg(target_os = "macos")]
    { let p = PathBuf::from("/usr/local/bin/node"); if p.is_file() { return Some(p); } }
    #[cfg(target_os = "windows")]
    { let p = PathBuf::from(r"C:\Program Files\nodejs\node.exe"); if p.is_file() { return Some(p); } }
    None
}

/// 产品用户数据根（~/.dsh/supervisor）。
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


