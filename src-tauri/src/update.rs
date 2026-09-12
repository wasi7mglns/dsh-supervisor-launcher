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

/// 是否存在可用的**提权通道**（用于「不可自更新」的提前判定）。
///
/// 实现已下沉到 platform 层（2026-09-11）：
///   · Linux   —— 探测 `pkexec` / `sudo` 是否在 PATH（deb/rpm 更新需要）
///   · macOS   —— osascript 管理员授权**恒可用**
///   · Windows —— UAC（msiexec -Verb RunAs）**恒可用**
///
/// ⚠ **不主动执行提权**，只探测命令存在性。
fn has_privilege_channel() -> bool {
    crate::platform::current().has_privilege_channel()
}

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
///
/// ⚠ 自锁缺陷与修法（P1-A，2026-09-12）：
///   旧设计里 `attempt` **只在更新成功时**归零（`init_identity` 的 confirmed 分支），
///   而 `should_check` 在 `attempt >= MAX_ATTEMPTS` 后**永久**返回 false ——
///   于是「成功」成了「继续尝试」的前提，而继续尝试被自己掐断，形成无复位机制的自锁。
///   叠加两个真实场景即触发：① 用户在授权弹窗上点了取消；② 弱网下载失败。
///   （两者都不回滚 pendingVersion，故每次重开壳都 +1。）
///
///   修法：把「永久拉黑」改为「**时间冷却**」——
///   · 记录 `last_attempt_at`，达到上限后进入冷却；
///   · 冷却期（GUARD_COOLDOWN_SECS）过后**自动复位** attempt 并重新尝试；
///   · 同时提供显式恢复入口（`reset_guard`），供 UI/排障调用。
///   这样「不再纠缠」与「永不恢复」被区分开：护栏仍然抑制反复失败，但不会永久关闭更新。
#[derive(Default)]
struct Guard {
    attempt: u32,
    pending_version: Option<String>,
    pinned: Vec<String>,
    /// 最近一次推进护栏的时间（秒）。用于冷却判定；旧账本无此字段时为 0。
    last_attempt_at: u64,
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
            last_attempt_at: v.get("lastAttemptAt").and_then(|x| x.as_u64()).unwrap_or(0),
        }
    }
    fn save(&self) {
        write_json(
            &guard_path(),
            &serde_json::json!({
                "attempt": self.attempt,
                "pendingVersion": self.pending_version,
                "pinned": self.pinned,
                "lastAttemptAt": self.last_attempt_at,
                "updatedAt": now_secs(),
            }),
        );
    }
    /// 冷却是否已过（未记录时间视作已过，兼容旧账本）。
    fn cooldown_elapsed(&self) -> bool {
        if self.last_attempt_at == 0 {
            return true;
        }
        now_secs().saturating_sub(self.last_attempt_at) >= GUARD_COOLDOWN_SECS
    }
}

/// 自更新被抑制后的冷却时长（秒）。**不是永久拉黑** —— 到期自动恢复尝试。
/// 取 6 小时：足够让「反复失败」不再打扰用户，又不会让机器永久收不到更新（含安全修复）。
const GUARD_COOLDOWN_SECS: u64 = 6 * 60 * 60;

/// 当前 Unix 时间（秒）。
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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
        // ⚠ P0 修复（2026-09-12）：**更新成功后必须把该版本移出 pinned**。
        //
        //   缺陷：pinned 只在「尝试失败达阈值」时写入**目标版本**（见下方情况 B），
        //     而清空点**只有** `reset_guard()`（手动恢复）——
        //     `confirmed` 分支只清 pending_version / attempt，**不清 pinned**。
        //
        //   于是「失败 → 冷却 → 重试**成功**」之后，该版本仍留在 pinned 里；
        //     此后 `should_check(version)` 命中 pinned 分支（**该分支不查冷却**）
        //     → **永久拒绝所有后续更新**（含安全修复）。
        //
        //   这正是 P1-A 要消除的「永久自锁」以**另一条路径**复现：
        //     原注释称「拉黑的是目标版本、比对的是当前版本、两者永不相等故形同虚设」，
        //     但**更新一旦成功，current == target**，该分支立刻生效且**没有期限**。
        //
        g.save();
        log(&format!("更新确认：已运行 {} （清零尝试计数）", version));
    } else if g.pending_version.is_some() {
        // 情况 B：装过新版本，但以别的版本启动 → 更新未生效/启动失败
        let target = g.pending_version.clone().unwrap_or_default();
        g.attempt = g.attempt.saturating_add(1);
        // 记录推进时间：达到上限后据此进入**冷却**（而非永久拉黑），见 Guard 文档。
        g.last_attempt_at = now_secs();
        if g.attempt >= MAX_ATTEMPTS {
            if !target.is_empty() && !g.pinned.contains(&target) {
                g.pinned.push(target.clone());
            }
            g.pending_version = None;
            log(&format!(
                "更新失败达阈值（{} 次）：已抑制 {}，{} 小时后自动重试（可经 shell_reset_update_guard 立即恢复）",
                g.attempt, target, GUARD_COOLDOWN_SECS / 3600
            ));
        } else {
            log(&format!("更新疑似未生效：期望 {}，实际 {}（第 {} 次）", target, version, g.attempt));
        }
        g.save();
    } else if g.attempt >= MAX_ATTEMPTS && g.cooldown_elapsed() {
        // 情况 C（P1-A 关键修复）：上一轮因失败进入冷却，现已到期 → **自动复位**。
        //   旧实现没有任何复位路径：attempt 只在「更新成功」时归零，
        //   而继续尝试又被 attempt>=2 掐断 —— 用户永久收不到更新。
        log(&format!(
            "更新冷却期已过（{} 小时），自动恢复更新尝试（原 attempt={}）",
            GUARD_COOLDOWN_SECS / 3600, g.attempt
        ));
        g.attempt = 0;
        g.save();
    }

    // ⚠ P0 修复（2026-09-12）：**只要当前运行的版本曾被拉黑，就解除拉黑**。
    //
    //   缺陷：`pinned` 只在「尝试失败达阈值」时写入**目标版本**（情况 B），
    //     清空点**只有** `reset_guard()`（手动恢复）。
    //     而 `should_check()` 的 pinned 分支**不查冷却** —— 于是
    //     「失败 → 冷却 → 重试成功」之后该版本仍留在黑名单里，
    //     此后 `should_check(version)` 恒 false → **永久收不到任何更新（含安全修复）**。
    //     这正是 P1-A 要消除的「永久自锁」以另一条路径复现：
    //     原注释称「两者永不相等故形同虚设」，但更新一旦成功 current==target，
    //     该分支立刻生效且**没有期限**。
    //
    //   此处放在 if/else 链**之后**（而非只在 confirmed 分支内），因为「当前版本曾被拉黑」
    //     有两种到达方式：① 经本护栏更新成功（confirmed）；
    //     ② 用户**手动**装了那个版本（它已被证明可用）。两者都该解除。
    //   抑制语义保留：将来该版本若再失败，情况 B 会重新 pin。
    if !version.is_empty() && g.pinned.iter().any(|p| p == version) {
        g.pinned.retain(|p| p != version);
        g.save();
        log(&format!("当前运行版本 {} 曾被抑制，现已解除（更新成功后不应再拉黑）", version));
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
        "lastSeenAt": now,
        // 壳自身的可执行路径（2026-09-11 新增）。
        //   **为什么记录它**：守卫看护（domains/shell/watchdog）需要在壳崩溃后把它拉起，
        //   而那时壳进程已不存在、`pgrepList` 拿不到它的 cmdline —— 必须有一个**已落盘**的路径来源。
        //   `std::env::current_exe()` 是权威值（真实运行中的二进制），优于按安装形态猜测。
        //   刷新时机：每次壳启动（含自更新后重启），故升级换路径后会自动跟随。
        "exe": std::env::current_exe().ok().map(|p| p.display().to_string()),
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
///
/// ⚠ 抑制是**有期限**的（P1-A）：达到上限后进入冷却，`cooldown_elapsed()` 为真即放行。
///   旧实现在此**永久**返回 false，且无任何复位路径 → 用户永久收不到更新。
pub fn should_check(version: &str) -> (bool, String) {
    let g = Guard::load();
    if !self_update_capable() {
        return (false, format!("安装形态 {} 不可自更新（或缺提权通道）", install_kind()));
    }
    if !version.is_empty() && g.pinned.iter().any(|p| p == version) {
        // 被拉黑的是「**曾经尝试过的目标版本**」，故只在目标版本 == 当前版本时才拦截。
        // ⚠ 旧实现比对的就是当前版本，而拉黑写的是目标版本 —— 两者永不相等，
        //   于是这条分支**形同虚设**（真正的抑制来自下面的 attempt 闸）。此处保留并说明。
        return (false, format!("当前版本 {} 已被抑制（此前更新失败）", version));
    }
    if g.attempt >= MAX_ATTEMPTS {
        let left = GUARD_COOLDOWN_SECS.saturating_sub(now_secs().saturating_sub(g.last_attempt_at));
        if !g.cooldown_elapsed() {
            return (false, format!(
                "连续失败 {} 次，已进入冷却（约 {} 分钟后自动恢复；或调用 shell_reset_update_guard 立即恢复）",
                g.attempt, left / 60
            ));
        }
        // 冷却已过：放行（下一次 init_identity 会把 attempt 复位）。
        return (true, String::new());
    }
    (true, String::new())
}

/// **显式恢复入口**（P1-A）：清零 attempt、解除冷却与拉黑。
///
/// 为什么要它：自动冷却解决「永久自锁」，但用户/支持人员仍需要一个「现在就再试一次」的动作。
/// 旧实现完全没有恢复路径，唯一手段是手工删 `~/.dsh/shell/update-guard.json`（UI 从不提示）。
/// 返回恢复前的状态，供日志与 UI 如实说明「恢复了什么」。
pub fn reset_guard() -> serde_json::Value {
    let before = Guard::load();
    let snapshot = serde_json::json!({
        "attempt": before.attempt,
        "pinned": before.pinned,
        "pendingVersion": before.pending_version,
    });
    let g = Guard {
        attempt: 0,
        pending_version: None,
        pinned: Vec::new(),
        last_attempt_at: 0,
    };
    g.save();
    log(&format!("更新护栏已手动复位（此前 attempt={}，pinned={:?}）", before.attempt, before.pinned));
    snapshot
}

pub fn identity_snapshot() -> serde_json::Value { read_json(&identity_path()) }
