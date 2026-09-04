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

/// 守卫本地 API 基址：读 ~/.dsh/supervisor/config.json 的 apiPort（用户可改），
/// 失败/缺失回退 3100。壳极少更新但内核配置可演进——硬编码 3100 会让
/// 改过 apiPort 的用户导航到死端口（2026-09 审计修复 F7）。
pub fn api_base_url() -> String {
    let default_port = 3100u16;
    let port = std::fs::read_to_string(supervisor_dir().join("config.json"))
        .ok()
        .and_then(|s| {
            // 极简 JSON 提取（无 serde 依赖）："apiPort": <num> 或 "apiPort":<num>
            for pat in ["\"apiPort\": ", "\"apiPort\":"] {
                if let Some(idx) = s.find(pat) {
                    let rest = &s[idx + pat.len()..];
                    let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                    if let Ok(n) = num.parse::<u16>() {
                        if n > 0 { return Some(n); }
                    }
                }
            }
            None
        })
        .unwrap_or(default_port);
    format!("http://127.0.0.1:{}/", port)
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


