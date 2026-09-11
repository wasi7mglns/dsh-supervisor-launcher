//! 桌面壳自更新 + 可观测性基座（2026-09-11）
//!
//! 设计要点（与《发布与更新机制总纲》《跨平台构建与自更新方案》一致）：
//!
//! 1. **壳直连公网自更新**：不依赖本机内核是否在运行（冷启动即可用）。
//!    端点 = npm CDN 上的静态清单（见 tauri.conf.json plugins.updater.endpoints）。
//! 2. **平台差异全部交给 Tauri 插件**：
//!    Linux  deb  → pkexec dpkg -i（一次密码）
//!    macOS  .app → 替换 ~/Applications 下的 .app
//!    Windows NSIS→ passive 静默安装
//!    壳侧**不写任何平台分支**，三平台行为一致。
//! 3. **有界失败即放行**（用户已确认的选择页语义）：
//!    离线 / 不可自更新 / 已达尝试阈值 → 不阻断，放行进入引导；
//!    失败时由引导页显示【重试】【继续】。
//! 4. **循环护栏**：身份文件记录 attempt 与 pendingVersion；
//!    若以「非 pending 版本」启动且尝试超限 → 将该版本加入本地黑名单（pinned），
//!    此后不再尝试（防「更新成功但版本比对仍认为需更新」导致无限重启）。
//! 5. **落盘日志**：壳此前零日志（无法诊断任何「打不开」问题），此处补齐。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// 壳状态目录（与内核状态目录物理隔离：内核用 ~/.dsh/supervisor）。
pub fn state_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".dsh").join("shell")
}

fn identity_path() -> PathBuf { state_dir().join("identity.json") }
fn log_path() -> PathBuf { state_dir().join("shell.log") }
fn guard_path() -> PathBuf { state_dir().join("update-guard.json") }

/// 落盘日志（滚动：超过 1MB 时保留后半部分）。绝不 panic、绝不阻塞启动。
pub fn log(line: &str) {
    let dir = state_dir();
    let _ = fs::create_dir_all(&dir);
    let p = log_path();
    if let Ok(md) = fs::metadata(&p) {
        if md.len() > 1024 * 1024 {
            if let Ok(s) = fs::read_to_string(&p) {
                let n = s.chars().count();
                let keep: String = s.chars().skip(n.saturating_sub(512 * 1024)).collect();
                let _ = fs::write(&p, keep);
            }
        }
    }
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&p) {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "[{}] {}", ts, line);
    }
}

/// 运行时安装形态（判断「能否自更新」与诊断用）。
pub fn install_kind() -> String {
    match tauri::utils::platform::bundle_type() {
        Some(tauri::utils::config::BundleType::Deb) => "deb",
        Some(tauri::utils::config::BundleType::Rpm) => "rpm",
        Some(tauri::utils::config::BundleType::AppImage) => "appimage",
        Some(tauri::utils::config::BundleType::Msi) => "msi",
        Some(tauri::utils::config::BundleType::Nsis) => "nsis",
        Some(tauri::utils::config::BundleType::App) => "app",
        _ => "unknown",
    }
    .to_string()
}

/// 是否存在可用的提权通道（Linux deb/rpm 的 pkexec 更新需要）。
/// **不主动执行提权**，只探测命令存在性，用于「不可自更新」的提前判定。
#[cfg(target_os = "linux")]
fn has_privilege_channel() -> bool {
    ["pkexec", "sudo"].iter().any(|c| {
        std::env::var("PATH")
            .ok()
            .map(|path| std::env::split_paths(&path).any(|d| d.join(c).is_file()))
            .unwrap_or(false)
    })
}
#[cfg(not(target_os = "linux"))]
fn has_privilege_channel() -> bool { true }

/// 能否自更新：形态受支持 且 有提权通道（Linux）。
pub fn self_update_capable() -> bool {
    match install_kind().as_str() {
        "deb" | "rpm" => has_privilege_channel(),
        "appimage" | "nsis" | "msi" | "app" => true,
        _ => false,
    }
}

fn read_json(p: &Path) -> serde_json::Value {
    fs::read_to_string(p)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

fn write_json(p: &Path, v: &serde_json::Value) {
    let dir = p.parent().unwrap_or(Path::new("."));
    let _ = fs::create_dir_all(dir);
    let tmp = p.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(v).unwrap_or_default();
    if fs::write(&tmp, body + "\n").is_ok() {
        let _ = fs::rename(&tmp, p);
    }
}

/// 更新护栏状态（壳本地；内核的 update-journal 是另一份权威账本）。
#[derive(Default)]
struct Guard {
    attempt: u32,
    pending_version: Option<String>,
    pinned: Vec<String>,
}

impl Guard {
    fn load() -> Self {
        let v = read_json(&guard_path());
        Guard {
            attempt: v.get("attempt").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
            pending_version: v.get("pendingVersion").and_then(|x| x.as_str()).map(|s| s.to_string()),
            pinned: v
                .get("pinned")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default(),
        }
    }
    fn save(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        write_json(
            &guard_path(),
            &serde_json::json!({
                "attempt": self.attempt,
                "pendingVersion": self.pending_version,
                "pinned": self.pinned,
                "updatedAt": now,
            }),
        );
    }
}

/// 最大连续失败次数（达到即放弃强制，并把该版本拉黑）。
const MAX_ATTEMPTS: u32 = 2;

/// 启动时调用：写身份文件 + 推进护栏状态。
pub fn init_identity(version: &str) -> serde_json::Value {
    let mut g = Guard::load();

    // 情况 A：当前运行版本 == 上次安装的 pendingVersion → 更新成功
    let confirmed = g.pending_version.as_deref() == Some(version) && !version.is_empty();
    if confirmed {
        g.pending_version = None;
        g.attempt = 0;
        g.save();
        log(&format!("更新确认：已运行 {} （清零尝试计数）", version));
    } else if g.pending_version.is_some() {
        // 情况 B：装过新版本，但以别的版本启动 → 更新未生效/启动失败
        let target = g.pending_version.clone().unwrap_or_default();
        g.attempt = g.attempt.saturating_add(1);
        if g.attempt >= MAX_ATTEMPTS {
            if !target.is_empty() && !g.pinned.contains(&target) {
                g.pinned.push(target.clone());
            }
            g.pending_version = None;
            log(&format!("更新失败达阈值（{} 次）：已拉黑 {}，本轮不再尝试", g.attempt, target));
        } else {
            log(&format!("更新疑似未生效：期望 {}，实际 {}（第 {} 次）", target, version, g.attempt));
        }
        g.save();
    }

    let kind = install_kind();
    let capable = self_update_capable();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let id = serde_json::json!({
        "version": version,
        "platform": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "installKind": kind,
        "selfUpdateCapable": capable,
        "attempt": g.attempt,
        "pinned": g.pinned,
        "pendingVersion": g.pending_version,
        "phase": "boot",
        "pid": std::process::id(),
        "startedAt": now,
    });
    write_json(&identity_path(), &id);
    log(&format!("壳启动 v{} kind={} 可自更新={} attempt={}", version, kind, capable, g.attempt));
    id
}

/// 更新阶段上报（引导页各步骤调用，保持 identity.json 与 shell.log 同步）。
pub fn set_phase(phase: &str) {
    let mut v = read_json(&identity_path());
    v["phase"] = serde_json::json!(phase);
    write_json(&identity_path(), &v);
    log(&format!("阶段 → {}", phase));
}

/// 标记「已安装待重启」的新版本。
pub fn mark_pending(version: &str) {
    let mut g = Guard::load();
    g.pending_version = Some(version.to_string());
    g.save();
    log(&format!("已安装 {}，待重启生效", version));
}

/// 前置判定：是否**应当**尝试检查更新。返回 (should_check, reason)。
pub fn should_check(version: &str) -> (bool, String) {
    let g = Guard::load();
    if !self_update_capable() {
        return (false, format!("安装形态 {} 不可自更新（或缺提权通道）", install_kind()));
    }
    if !version.is_empty() && g.pinned.iter().any(|p| p == version) {
        return (false, format!("当前版本 {} 已被拉黑（此前更新失败）", version));
    }
    if g.attempt >= MAX_ATTEMPTS {
        return (false, format!("连续失败已达 {} 次上限，本轮放行", g.attempt));
    }
    (true, String::new())
}

pub fn identity_snapshot() -> serde_json::Value { read_json(&identity_path()) }
