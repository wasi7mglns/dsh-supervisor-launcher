//! 镜像源适配（**壳自持**，2026-09-11）。
//!
//! == 为什么必须在壳内做（用户指正） ==
//!
//! 用户装机的那一刻，机器上**没有内核** —— 内核是随后由壳自己安装的。
//! 因此壳的每一处下载（Node 运行时、内核 npm 包、壳自身更新）都**不能**依赖
//! 内核的 registry.json（那时它还不存在）。镜像适配必须由壳自带。
//!
//! == 三条下载链路 ==
//!   ① Node 运行时   —— node.rs  （index.json + 安装包 + SHASUMS256）
//!   ② 内核 npm 包   —— core.rs  （包元数据 + npm install -g）
//!   ③ 壳自更新      —— main.rs  （Tauri updater 清单与安装包）
//!
//! == 策略：并行测速 + 缓存 + 取最高版本 ==
//!
//! 1. **并行**探测全部候选（串行会让最慢的源拖死整体；旧实现名为"并发"实为串行）；
//! 2. Node 版本发现**取全部可达源中的最高版本** —— 实测腾讯云镜像会**滞后一个版本**，
//!    若"首个成功即采用"会静默装到旧版；
//! 3. 选择**最快**且确实提供该版本的源做下载；
//! 4. 结果写入 ~/.dsh/shell/mirrors.json 缓存（TTL），并**导出给内核**
//!    （registry.json）以便后续继承同一份镜像偏好。
//!
//! == 为什么探测 Node 用 index.json ==
//! 我们无论如何都要拉它来发现版本，故一次并行拉取同时得到「延迟」与「版本」，
//! 不额外增加请求。

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// npm registry 预设（与内核 config.registries 同集合）。
pub const NPM_PRESETS: [&str; 4] = [
    "https://registry.npmmirror.com",
    "https://registry.npmjs.org",
    "https://mirrors.cloud.tencent.com/npm",
    "https://repo.huaweicloud.com/repository/npm/",
];

/// Node 发行镜像预设。**每一项实测可用**（index.json + SHASUMS256 + 各平台产物齐备，
/// 含 macOS 所需的 .pkg）。
pub const NODE_PRESETS: [&str; 4] = [
    "https://nodejs.org/dist",
    "https://npmmirror.com/mirrors/node",
    "https://mirrors.huaweicloud.com/nodejs",
    "https://mirrors.cloud.tencent.com/nodejs-release",
];

/// 壳自更新清单预设（Tauri updater 的 endpoints；此处存完整清单 URL）。
pub const SHELL_PRESETS: [&str; 2] = [
    "https://unpkg.com/@dsh-sup/shell-release@latest/shell-manifest.json",
    "https://cdn.jsdelivr.net/npm/@dsh-sup/shell-release@latest/shell-manifest.json",
];

/// 测速缓存有效期（与内核 selectRegistry 的 30 分钟一致）。
const CACHE_TTL_SECS: u64 = 30 * 60;

/// 单次探测的总超时（元数据很小，8 秒足够；避免坏源拖慢整体）。
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 壳自持镜像配置（~/.dsh/shell/mirrors.json）。
#[derive(Clone)]
pub struct Mirrors {
    pub node: Vec<String>,
    pub npm: Vec<String>,
    pub shell: Vec<String>,
    pub selected_node: Option<String>,
    pub selected_npm: Option<String>,
    pub checked_at: Option<u64>,
}

impl Default for Mirrors {
    fn default() -> Self {
        Mirrors {
            node: NODE_PRESETS.iter().map(|s| s.to_string()).collect(),
            npm: NPM_PRESETS.iter().map(|s| s.to_string()).collect(),
            shell: SHELL_PRESETS.iter().map(|s| s.to_string()).collect(),
            selected_node: None,
            selected_npm: None,
            checked_at: None,
        }
    }
}

fn cfg_path() -> PathBuf {
    crate::update::state_dir().join("mirrors.json")
}

/// 读取壳镜像配置；缺失或损坏时返回内置预设（**绝不失败** —— 装机首启必须可用）。
pub fn load() -> Mirrors {
    let mut m = Mirrors::default();
    if let Ok(s) = std::fs::read_to_string(cfg_path()) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
            let take = |key: &str| -> Option<Vec<String>> {
                v.get(key).and_then(|x| x.as_array()).map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str())
                        .map(|x| x.to_string())
                        .filter(|x| !x.is_empty())
                        .collect::<Vec<_>>()
                })
            };
            if let Some(list) = take("node") {
                if !list.is_empty() {
                    m.node = list;
                }
            }
            if let Some(list) = take("npm") {
                if !list.is_empty() {
                    m.npm = list;
                }
            }
            if let Some(list) = take("shell") {
                if !list.is_empty() {
                    m.shell = list;
                }
            }
            if let Some(s2) = v.get("selectedNode").and_then(|x| x.as_str()) {
                m.selected_node = Some(s2.to_string());
            }
            if let Some(s2) = v.get("selectedNpm").and_then(|x| x.as_str()) {
                m.selected_npm = Some(s2.to_string());
            }
            m.checked_at = v.get("checkedAt").and_then(|x| x.as_u64());
        }
    }
    m
}

/// 写壳镜像配置（原子写）。
pub fn save(m: &Mirrors) -> Result<(), String> {
    let path = cfg_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建壳状态目录失败: {}", e))?;
    }
    let v = serde_json::json!({
        "node": m.node,
        "npm": m.npm,
        "shell": m.shell,
        "selectedNode": m.selected_node,
        "selectedNpm": m.selected_npm,
        "checkedAt": m.checked_at,
    });
    let body = serde_json::to_string_pretty(&v).unwrap_or_default();
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body + "\n").map_err(|e| format!("写入镜像配置失败: {}", e))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("提交镜像配置失败: {}", e))?;
    Ok(())
}

/// 导出镜像偏好给内核（~/.dsh/supervisor/registry.json）。
///
/// 目的：壳在无内核时就完成了镜像选择，装完内核后**不应重新盲选** ——
/// 内核读取该文件即可继承同一份偏好（格式与内核 DistributionManager 一致：
/// mode / origins / manualOrigin）。
pub fn export_to_kernel(npm: &[String]) -> Result<(), String> {
    if npm.is_empty() {
        return Ok(());
    }
    let path = crate::env::supervisor_dir().join("registry.json");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建内核状态目录失败: {}", e))?;
    }
    // 若内核已写入过（含手动模式），**不覆盖**用户/内核的选择。
    if let Ok(s) = std::fs::read_to_string(&path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
            if v.get("mode").and_then(|x| x.as_str()) == Some("manual") {
                return Ok(());
            }
        }
    }
    let v = serde_json::json!({
        "mode": "auto",
        "origins": npm,
        "manualOrigin": npm.first().cloned().unwrap_or_default(),
    });
    let body = serde_json::to_string_pretty(&v).unwrap_or_default();
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body + "\n").map_err(|e| format!("写入内核 registry.json 失败: {}", e))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("提交内核 registry.json 失败: {}", e))?;
    Ok(())
}

/// 一次并行探测的结果。
pub struct Probe {
    pub source: String,
    pub ok: bool,
    pub latency_ms: u128,
    pub body: Option<String>,
}

/// **并行**探测全部候选：对每个源请求 path，记录延迟与响应体。
///
/// 用 std::thread::scope（std 自带，无需新依赖）实现并发；
/// 单源超时 PROBE_TIMEOUT，整体耗时约为其中最慢者而非累加。
pub fn probe_all(sources: &[String], path: &str) -> Vec<Probe> {
    let out: Mutex<Vec<Probe>> = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for src in sources {
            let out = &out;
            scope.spawn(move || {
                let url = format!(
                    "{}/{}",
                    src.trim_end_matches("/"),
                    path.trim_start_matches("/")
                );
                let started = Instant::now();
                let mut probe = Probe {
                    source: src.clone(),
                    ok: false,
                    latency_ms: 0,
                    body: None,
                };
                match ureq::get(&url).timeout(PROBE_TIMEOUT).call() {
                    Ok(resp) => {
                        let mut buf = Vec::new();
                        use std::io::Read;
                        if resp.into_reader().read_to_end(&mut buf).is_ok() {
                            probe.latency_ms = started.elapsed().as_millis();
                            probe.ok = true;
                            probe.body = Some(String::from_utf8_lossy(&buf).into_owned());
                        }
                    }
                    Err(_) => {
                        probe.latency_ms = started.elapsed().as_millis();
                    }
                }
                out.lock().unwrap().push(probe);
            });
        }
    });
    let mut v = out.into_inner().unwrap_or_default();
    v.sort_by(|a, b| match (a.ok, b.ok) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.latency_ms.cmp(&b.latency_ms),
    });
    v
}

/// 缓存是否仍然新鲜。
pub fn cache_fresh(m: &Mirrors) -> bool {
    match m.checked_at {
        Some(t) => now_secs().saturating_sub(t) < CACHE_TTL_SECS,
        None => false,
    }
}
