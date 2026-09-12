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
    if let Some(p) = test_state_dir_override() {
        return p;
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".dsh").join("shell")
}

/// 状态目录的**测试注入点**（沿用 nodeprobe::set_hard_deadline_ms 的既有惯例）。
///
/// 为什么需要：护栏的四个写入方（init_identity / mark_pending / reset_guard /
/// set_phase）都落在 state_dir() 下。行为级测试必须能把它改写到临时目录 ——
/// 否则测试会**改写开发者真实的 ~/.dsh/shell/identity.json**，
/// 那是「测试污染真实状态」，不可接受。
#[cfg(test)]
static TEST_STATE_DIR: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

#[cfg(test)]
fn test_state_dir_override() -> Option<PathBuf> {
    TEST_STATE_DIR.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(not(test))]
fn test_state_dir_override() -> Option<PathBuf> {
    None
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
        // ⚠ 上限裁剪（2026-09-13，P3 修复）：pinned 此前**只增不剪**。
        //
        //   缺陷：每次「尝试失败达阈值」都会 push 一个**目标版本**（见 init_identity 情况 B），
        //     而全仓**唯一**的移除点是 init_identity 里那一处 —— 它只解除与**当前运行版本**
        //     相等的一项。于是失败过的旧目标版本永久留在列表里（及其在 identity.json 的投影）。
        //     长期反复更新失败的机器上该列表单调增长：文件越来越长，
        //     而其中绝大多数旧版本**早已不可能再成为更新目标**。
        //
        //   修法：在**唯一的持久化点**（本函数）统一裁剪，保留**最近** MAX_PINNED 项。
        //     放在这里而非各调用点 —— 否则又是「纪律只在一条路径上执行」（失效模式 g）。
        //     保留最近的（而非最早的）：越新的失败越相关，且更新目标总趋于最新版本。
        //
        //   ⚠ 裁剪必须经 pruned_pinned()，**不能在本函数里各写一遍** ——
        //     否则 update-guard.json 裁剪了，而 write_identity_for 的 identity.json
        //     投影用未裁剪的 self.pinned（同一事实两处实现再次分叉）。
        write_json(
            &guard_path(),
            &serde_json::json!({
                "attempt": self.attempt,
                "pendingVersion": self.pending_version,
                "pinned": self.pruned_pinned(),
                "lastAttemptAt": self.last_attempt_at,
                "updatedAt": now_secs(),
            }),
        );
    }
    /// 裁剪后的本地黑名单（保留**最近** MAX_PINNED 项，丢弃更早的）。
    ///
    /// **唯一**的裁剪实现：update-guard.json 的落盘（save）与 identity.json 的
    /// 护栏投影（write_identity_for）都经它 —— 否则两处会各自裁剪/不裁剪而分叉。
    fn pruned_pinned(&self) -> Vec<String> {
        let n = self.pinned.len();
        let start = n.saturating_sub(MAX_PINNED);
        self.pinned[start..].to_vec()
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

/// 本地黑名单（pinned）的**上限** —— 超过时保留最近 MAX_PINNED 项（见 Guard::save）。
///
/// 取 8：足够覆盖「近期若干次失败的目标版本」，又让文件与 identity.json 投影有明确上界。
/// 被裁掉的都是**很早以前**的目标版本 —— 更新目标总趋于最新，它们不会再被尝试，
/// 故裁剪不损失任何抑制语义。
const MAX_PINNED: usize = 8;

/// 写 identity.json —— **身份文件的唯一写入点**。
///
/// ## 为什么要收敛（D-3 第一步，2026-09-13）
///
/// 缺陷（第二状态源）：init_identity 把 attempt / pinned / pendingVersion **抄进**
/// identity.json，而 reset_guard() / mark_pending() **只写** update-guard.json、
/// 不回写 identity.json。于是同一事实有两份存储，且**已被证明会分叉**：
///
///   · 护栏账本（update-guard.json）是真正的事实：Guard::save() 是唯一推进入口；
///   · identity.json 的护栏字段是它的**手工副本**，只在壳启动时刷新一次。
///
/// 两个消费方都读 identity.json 里这份**陈旧副本**：
///   · 壳 commands/mod.rs:416 shell_identity（引导页展示 attempt/pinned）；
///   · 内核 domains/shell/index.js:119-121 把 id.attempt 当「失败次数」权威来源（决定回滚）。
///
/// 后果示例：用户点【立即恢复】（reset_guard：attempt=0、pinned 清空）后，
/// 引导页与内核**仍看到旧 attempt/pinned** —— 恢复动作在 UI 与回滚判定上都不生效，
/// 直到下次壳重启才纠正（若壳因更新失败反复被抑制，可能很久）。
///
/// 修法：把 identity.json 的**所有**写入收敛到本函数，四个写入方
/// （init_identity / set_phase / reset_guard / mark_pending）全部经它；
/// 护栏字段一律取自传入的 Guard，**调用方无法自行传入** ——
/// 使 identity.json 永远是 Guard 的投影，不可能分叉。
///
/// 注意：这是**第一步**（单一写入路径，不动任何消费方，零跨仓风险）。
/// 第二步（从 identity.json 移除这三个字段、消费方改读 update-guard.json）
/// 是跨仓契约变更，必须两侧协调发版，另行定夺。
///
/// runtime_fields 描述**本次调用的运行时上下文**（version / phase / pid /
/// startedAt / exe 等）；其余字段从现有文件保留。
fn write_identity_for(g: &Guard, runtime_fields: &serde_json::Value) -> serde_json::Value {
    let mut v = read_json(&identity_path());
    if !v.is_object() {
        v = serde_json::json!({});
    }
    let map = v.as_object_mut().expect("identity.json 必须是对象");
    if let Some(rf) = runtime_fields.as_object() {
        for (k, val) in rf {
            map.insert(k.clone(), val.clone());
        }
    }
    // 护栏字段：**只**来自 Guard，调用方无法覆盖（即便 runtime_fields 里写了同名键，
    // 也会被下面这三行盖掉）—— 这正是「不可能分叉」的实现保证。
    map.insert("attempt".into(), serde_json::json!(g.attempt));
    // ⚠ pinned 用**裁剪后**的值（与 update-guard.json 同源）—— 否则文件受上限约束
    //   而 identity.json 投影不受，同一事实两处实现分叉。
    map.insert("pinned".into(), serde_json::json!(g.pruned_pinned()));
    map.insert("pendingVersion".into(), serde_json::json!(g.pending_version));
    let out = serde_json::Value::Object(map.clone());
    write_json(&identity_path(), &out);
    out
}

/// 写 identity.json，仅同步护栏投影（reset_guard / mark_pending 用）。
fn write_identity(g: &Guard) -> serde_json::Value {
    write_identity_for(g, &serde_json::json!({}))
}

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
    // ⚠ D-3 第一步：护栏字段（attempt/pinned/pendingVersion）**不在此处手抄** ——
    //   本 json! 只描述**运行时上下文**，护栏投影由 write_identity_for 从传入的 Guard 注入，
    //   杜绝「这里写一份、reset_guard/mark_pending 忘记回写」的分叉。
    let runtime = serde_json::json!({
        "version": version,
        "platform": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "installKind": kind,
        "selfUpdateCapable": capable,
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
    let id = write_identity_for(&g, &runtime);
    log(&format!("壳启动 v{} kind={} 可自更新={} attempt={}", version, kind, capable, g.attempt));
    id
}

/// 更新阶段上报（引导页各步骤调用，保持 identity.json 与 shell.log 同步）。
pub fn set_phase(phase: &str) {
    // ⚠ D-3 第一步：经统一写入点。**必须重新 load Guard**（而非沿用内存中的旧值）——
    //   mark_pending / init_identity 可能在本命令之前刚刚改过账本，
    //   用旧值会把护栏投影**写回陈旧状态**。
    let g = Guard::load();
    write_identity_for(&g, &serde_json::json!({ "phase": phase }));
    log(&format!("阶段 → {}", phase));
}

/// 标记「已安装待重启」的新版本。
pub fn mark_pending(version: &str) {
    let mut g = Guard::load();
    g.pending_version = Some(version.to_string());
    g.save();
    // ⚠ D-3 第一步：账本变更后**同步** identity.json 的护栏投影。
    //   旧实现只写 update-guard.json，故 identity.json 的 pendingVersion 一直是旧值 ——
    //   而壳的 shell_identity 快照与内核 domains/shell/index.js 都读它。
    write_identity(&g);
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
    // ⚠ D-3 第一步（本缺陷的核心症状）：复位后**必须**同步 identity.json。
    //   旧实现只写 update-guard.json —— 于是用户点【立即恢复】之后，
    //   shell_identity 快照与内核回滚判定读到的仍是**复位前的旧 attempt/pinned**，
    //   「恢复」在 UI 与回滚判定上都不生效，直到下次壳重启。
    write_identity(&g);
    log(&format!("更新护栏已手动复位（此前 attempt={}，pinned={:?}）", before.attempt, before.pinned));
    snapshot
}

pub fn identity_snapshot() -> serde_json::Value { read_json(&identity_path()) }

#[cfg(test)]
mod d3_tests {
    //! D-3 第一步门禁：identity.json 的护栏字段必须是 Guard 的**投影**（单一写入路径）。
    //!
    //! ## 修复的缺陷
    //!
    //! init_identity 把 attempt / pinned / pendingVersion 抄进 identity.json，
    //! 而 reset_guard() / mark_pending() **只写** update-guard.json、不回写 identity.json。
    //! 于是同一事实有两个真源，且**必然分叉**（后者只在壳启动时刷新）。
    //!
    //! 两个消费方读的都是 identity.json 那份陈旧副本：
    //!   · 壳 commands/mod.rs shell_identity（引导页展示 attempt/pinned）；
    //!   · 内核 domains/shell/index.js id.attempt（回滚判定的权威失败计数）。
    //!
    //! ## 为什么这是**行为级**测试（不是源码字符串断言）
    //!
    //! 「所有写入都经同一个 helper」用 grep 断言很容易被自己写的注释骗过
    //! （本仓已发生过两次：断言命中说明文字里的字符串）。此处直接**调用真实写入方**
    //! 并读回 identity.json —— 注入任何一处「漏写 / 写回旧值」都会让它失败。
    //!
    //! ## 目录隔离
    //!
    //! 经 state_dir() 的注入点把状态目录指向临时目录，绝不触碰开发者真实的
    //! ~/.dsh/shell/identity.json（测试污染真实状态是不可接受的）。
    //!
    //! ## 串行
    //!
    //! 默认测试线程池会让多个用例并发改写**同一个** TEST_STATE_DIR，
    //! 故统一用一把静态锁把本模块的用例串行化。
    use super::*;
    use std::sync::MutexGuard;

    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct Env {
        dir: PathBuf,
        _g: MutexGuard<'static, ()>,
    }

    impl Env {
        fn new(tag: &str) -> Self {
            let g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
            let dir = std::env::temp_dir().join(format!("dsh-d3-{}-{}", tag, std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("建临时状态目录");
            *TEST_STATE_DIR.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir.clone());
            Env { dir, _g: g }
        }
        fn guard(&self) -> serde_json::Value {
            read_json(&self.dir.join("update-guard.json"))
        }
    }

    impl Drop for Env {
        fn drop(&mut self) {
            *TEST_STATE_DIR.lock().unwrap_or_else(|e| e.into_inner()) = None;
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    /// 造一份账本，模拟「更新失败达阈值」后的护栏状态（attempt=2、pinned 有项）。
    fn seed_guard(dir: &Path, attempt: u32, pinned: &[&str], pending: Option<&str>) {
        write_json(
            &dir.join("update-guard.json"),
            &serde_json::json!({
                "attempt": attempt,
                "pendingVersion": pending,
                "pinned": pinned,
                "lastAttemptAt": now_secs(),
            }),
        );
    }

    /// D-3-a（核心症状）：reset_guard 必须让 identity.json 的护栏字段同步归零。
    ///
    /// 注入：删掉 reset_guard 里的 write_identity(&g) →
    /// identity.json 仍显示复位前的 attempt=2 / pinned=["9.9.9"] → FAIL。
    #[test]
    fn d3a_reset_guard_syncs_identity_projection() {
        let env = Env::new("reset");
        // 先让 identity.json 存在且带着「复位前」的护栏值（旧实现正是停在这里）。
        seed_guard(&env.dir, 2, &["9.9.9"], Some("9.9.9"));
        write_json(
            &env.dir.join("identity.json"),
            &serde_json::json!({
                "version": "1.0.8",
                "attempt": 2,
                "pinned": ["9.9.9"],
                "pendingVersion": "9.9.9",
            }),
        );
        let before = identity_snapshot();
        assert_eq!(before["attempt"], serde_json::json!(2), "前置：identity 应带着旧 attempt");

        reset_guard();

        // 账本已清零
        let g = env.guard();
        assert_eq!(g["attempt"], serde_json::json!(0), "update-guard attempt 应归零");
        assert_eq!(g["pinned"], serde_json::json!([]), "update-guard pinned 应清空");
        // identity.json 必须**同步**投影（这正是缺陷所在）
        let id = identity_snapshot();
        assert_eq!(
            id["attempt"],
            serde_json::json!(0),
            "D-3-a FAIL reset_guard 未回写 identity.json —— 引导页与内核仍看到旧 attempt"
        );
        assert_eq!(
            id["pinned"],
            serde_json::json!([]),
            "D-3-a FAIL reset_guard 未回写 identity.json 的 pinned"
        );
        assert_eq!(
            id["pendingVersion"],
            serde_json::Value::Null,
            "D-3-a FAIL reset_guard 未回写 identity.json 的 pendingVersion"
        );
    }

    /// D-3-b：mark_pending 必须把新的 pendingVersion 同步进 identity.json。
    ///
    /// 注入：删掉 mark_pending 里的 write_identity(&g) → identity.json 仍是旧值 → FAIL。
    #[test]
    fn d3b_mark_pending_syncs_identity_projection() {
        let env = Env::new("pending");
        seed_guard(&env.dir, 1, &[], None);
        init_identity("1.0.8");
        assert_eq!(
            identity_snapshot()["pendingVersion"],
            serde_json::Value::Null,
            "前置：尚无待生效版本"
        );

        mark_pending("1.0.9");

        assert_eq!(
            identity_snapshot()["pendingVersion"],
            serde_json::json!("1.0.9"),
            "D-3-b FAIL mark_pending 未回写 identity.json —— 内核看不到待确认的更新"
        );
        // 反向：账本本身也必须已更新（两处一致，不是只有一处对）
        assert_eq!(env.guard()["pendingVersion"], serde_json::json!("1.0.9"));
    }

    /// D-3-c：set_phase 不得把**陈旧**的护栏投影写回 identity.json。
    ///
    /// 这是统一写入路径**引入的新风险**：set_phase 原实现「读回旧文件再改 phase」，
    /// 若改成「用内存里的旧 Guard」，就会把护栏字段退回旧值。
    /// 注入：把 set_phase 里的 Guard::load() 换成空 Guard::default() → FAIL。
    #[test]
    fn d3c_set_phase_keeps_guard_projection_fresh() {
        let env = Env::new("phase");
        seed_guard(&env.dir, 2, &["9.9.9"], Some("9.9.9"));
        // 让 identity.json 处于「陈旧」状态（旧 attempt=0），复现真实分叉场景。
        write_json(
            &env.dir.join("identity.json"),
            &serde_json::json!({
                "version": "1.0.8",
                "attempt": 0,
                "pinned": [],
                "pendingVersion": serde_json::Value::Null,
            }),
        );

        set_phase("shell-update-check");

        let id = identity_snapshot();
        assert_eq!(id["phase"], serde_json::json!("shell-update-check"), "phase 必须被写入");
        assert_eq!(
            id["attempt"],
            serde_json::json!(2),
            "D-3-c FAIL set_phase 把陈旧护栏投影写回（应用 Guard::load 的最新值）"
        );
        assert_eq!(
            id["pinned"],
            serde_json::json!(["9.9.9"]),
            "D-3-c FAIL set_phase 未从账本投影 pinned"
        );
        // 运行时字段不得被 set_phase 破坏
        assert_eq!(id["version"], serde_json::json!("1.0.8"), "运行时字段应保留");
    }

    /// D-3-d：守卫字段只能来自 Guard —— 调用方**无法**通过 runtime_fields 覆盖。
    ///
    /// 这是「不可能分叉」的机制保证：若哪天有人图省事在 runtime_fields 里塞了
    /// 一个 attempt，也不该生效。
    ///
    /// 注入：删掉 write_identity_for 里三行 map.insert 护栏字段的**最后一行**（attempt）
    /// → 调用方传入的 999 会留下 → FAIL。
    #[test]
    fn d3d_guard_fields_cannot_be_overridden_by_caller() {
        let env = Env::new("override");
        seed_guard(&env.dir, 1, &["1.1.1"], Some("1.1.1"));
        let g = Guard {
            attempt: 7,
            pending_version: None,
            pinned: vec!["2.2.2".into()],
            last_attempt_at: 0,
        };
        let out = write_identity_for(
            &g,
            &serde_json::json!({
                "version": "1.0.8",
                // 恶意/图省事：调用方试图直接指定护栏字段
                "attempt": 999,
                "pinned": ["999.0.0"],
                "pendingVersion": "999.0.0",
            }),
        );
        assert_eq!(
            out["attempt"],
            serde_json::json!(7),
            "D-3-d FAIL 调用方传的 attempt 覆盖了 Guard —— 投影保证被破坏"
        );
        assert_eq!(out["pinned"], serde_json::json!(["2.2.2"]));
        assert_eq!(out["pendingVersion"], serde_json::Value::Null);
        // 落盘文件同样必须是 Guard 的值
        assert_eq!(identity_snapshot()["attempt"], serde_json::json!(7));
    }

    /// D-3-e（反向）：运行时字段与**未知**字段必须被保留 —— 收敛写入不能变成丢字段。
    ///
    /// 旧实现是「read → 改几个键 → write」，preserve 其它键是既有契约
    /// （例如 shell.html 依赖的 installKind / selfUpdateCapable / exe）。
    ///
    /// 注入：把 write_identity_for 里的 read_json 换成 json!({})（不读旧文件）→ FAIL。
    #[test]
    fn d3e_helper_preserves_runtime_and_unknown_fields() {
        let env = Env::new("preserve");
        write_json(
            &env.dir.join("identity.json"),
            &serde_json::json!({
                "version": "1.0.8",
                "installKind": "deb",
                "selfUpdateCapable": true,
                "exe": "/opt/dsh/gui",
                "phase": "boot",
                "futureFieldFromNewerShell": "keep-me",
            }),
        );
        let g = Guard::default();
        // 只更新 phase，其余全部保留
        write_identity_for(&g, &serde_json::json!({ "phase": "ready" }));

        let id = identity_snapshot();
        assert_eq!(id["phase"], serde_json::json!("ready"), "phase 应被更新");
        assert_eq!(id["version"], serde_json::json!("1.0.8"), "version 应保留");
        assert_eq!(id["installKind"], serde_json::json!("deb"), "installKind 应保留");
        assert_eq!(id["selfUpdateCapable"], serde_json::json!(true), "selfUpdateCapable 应保留");
        assert_eq!(id["exe"], serde_json::json!("/opt/dsh/gui"), "exe 应保留");
        assert_eq!(
            id["futureFieldFromNewerShell"],
            serde_json::json!("keep-me"),
            "D-3-e FAIL 未知字段被丢弃（收敛不得变成裁剪）"
        );
    }

    /// A-6：pinned 不得只增不剪 —— 持久化点必须裁剪到 MAX_PINNED（保留最近）。
    ///
    /// 注入：把 Guard::save 里的 take(MAX_PINNED) 改回 self.pinned → 本测试 FAIL。
    #[test]
    fn a6_pinned_is_pruned_at_persistence() {
        let env = Env::new("pinned");
        // 造一个「长期反复失败」的账本：远超上限的旧目标版本
        let many: Vec<String> = (0..40).map(|i| format!("0.0.{}", i)).collect();
        let mut g = Guard::default();
        g.pinned = many.clone();
        g.last_attempt_at = now_secs();
        g.save();

        let saved = env.guard();
        let pinned = saved["pinned"].as_array().expect("pinned 应为数组");
        assert!(
            pinned.len() <= MAX_PINNED,
            "A-6 FAIL pinned 未裁剪（{} 项，上限 {}）—— 只增不剪",
            pinned.len(),
            MAX_PINNED
        );
        assert_eq!(pinned.len(), MAX_PINNED, "应恰好保留到上限");
        // 保留的必须是**最近**的（列表尾部），而不是最早的
        let kept: Vec<String> = pinned.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect();
        let expected: Vec<String> = many[many.len() - MAX_PINNED..].to_vec();
        assert_eq!(kept, expected, "A-6 FAIL 裁剪保留的不是最近的项");

        // 反向：未超上限时不得被裁剪（不能变成「最多只记 1 项」）
        let mut g2 = Guard::default();
        g2.pinned = vec!["1.0.0".into(), "1.0.1".into()];
        g2.save();
        assert_eq!(env.guard()["pinned"], serde_json::json!(["1.0.0", "1.0.1"]));
    }

    /// A-6（闭环）：init_identity 写入的 identity.json 投影同样受上限约束。
    #[test]
    fn a6_pinned_projection_also_bounded() {
        let _env = Env::new("pinned-proj");
        let many: Vec<String> = (0..30).map(|i| format!("0.0.{}", i)).collect();
        let mut g = Guard::default();
        g.pinned = many;
        g.save();
        // 以「已在 pinned 中的版本」启动：解除该项后仍应有界
        init_identity("0.0.29");
        let id = identity_snapshot();
        let n = id["pinned"].as_array().map(|a| a.len()).unwrap_or(0);
        assert!(n <= MAX_PINNED, "A-6 FAIL identity.json 的 pinned 投影未受上限约束（{} 项）", n);
        assert!(
            !id["pinned"].as_array().unwrap().iter().any(|x| x == "0.0.29"),
            "A-6 FAIL 运行中的版本应被解除拉黑"
        );
    }

    /// A-6（投影同源）：identity.json 的 pinned 必须与 update-guard.json **用同一份裁剪结果**。
    ///
    /// 注入判据：把 `write_identity_for` 里的 `g.pruned_pinned()` 改回 `g.pinned` ——
    ///   此时落盘文件受 MAX_PINNED 约束、而投影不受 → **两处不一致** → 本测试 FAIL。
    ///   （注意必须让**写入方**产生分叉：只把 pruned_pinned 改成返回全量会同时改变两边，
    ///     那种注入由 a6_pinned_is_pruned_at_persistence 捕获，不是本用例的职责。）
    #[test]
    fn a6_projection_and_file_use_same_pruning() {
        let _env = Env::new("pinned-samesrc");
        // 不变量：无论 pinned 多长，投影长度 == 文件长度 == 裁剪后长度。
        for n in [0usize, 1, MAX_PINNED - 1, MAX_PINNED, MAX_PINNED + 1, 40] {
            let mut g = Guard::default();
            g.pinned = (0..n).map(|i| format!("0.0.{}", i)).collect();
            g.last_attempt_at = now_secs();
            g.save();
            let file_pinned = _env.guard()["pinned"].clone();
            let projected = write_identity(&g)["pinned"].clone();
            assert_eq!(
                file_pinned, projected,
                "A-6 FAIL n={} 时 identity.json 投影与 update-guard.json 不同源：文件={} 投影={}",
                n, file_pinned, projected
            );
            assert_eq!(
                projected.as_array().map(|a| a.len()).unwrap_or(0),
                n.min(MAX_PINNED),
                "A-6 FAIL n={} 时裁剪长度不符",
                n
            );
        }
    }

    /// D-3-f：init_identity 启动即把护栏投影写正确（正向闭环）。
    #[test]
    fn d3f_init_identity_projects_guard_from_scratch() {
        let env = Env::new("init");
        seed_guard(&env.dir, 2, &["9.9.9"], Some("9.9.9"));
        // 以别的版本启动 → 情况 B：attempt+1、达到阈值 → pin 目标、清 pending
        init_identity("1.0.8");
        let id = identity_snapshot();
        assert_eq!(id["version"], serde_json::json!("1.0.8"));
        assert_eq!(id["attempt"], serde_json::json!(3), "attempt 应 +1");
        assert!(
            id["pinned"].as_array().map(|a| a.iter().any(|x| x == "9.9.9")).unwrap_or(false),
            "D-3-f FAIL identity.json 的 pinned 未反映账本"
        );
        assert_eq!(id["pendingVersion"], serde_json::Value::Null);
    }
}

