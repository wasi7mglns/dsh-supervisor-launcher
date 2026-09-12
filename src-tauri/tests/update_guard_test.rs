//! G6：自更新护栏**不得永久自锁**（P1-A 回归，2026-09-12）。
//!
//! ## 修复的缺陷
//!
//! `update.rs` 旧设计中 `attempt` **只在更新成功时**归零（`init_identity` 的 confirmed 分支），
//! 而 `should_check` 在 `attempt >= MAX_ATTEMPTS` 后**永久**返回 false：
//!
//! ```text
//!   :160  g.attempt = 0        ← 唯一归零点，仅当「更新成功」时到达
//!   :187  attempt >= 2 → 永久 false   ← 成功路径被自己掐断
//! ```
//!
//! 即「成功」成了「继续尝试」的前提，而继续尝试被自己掐断 —— **无复位机制的自锁**。
//! 两个真实场景叠加即触发：① 用户在授权弹窗点取消；② 弱网下载失败。
//! （两者都不回滚 pendingVersion，故每次重开壳 attempt+1。）
//!
//! 后果：用户**永久**收不到壳更新（含安全修复），UI 从不提示，
//! 唯一恢复手段是手工删 `~/.dsh/shell/update-guard.json`。
//!
//! ## 锁定不变量
//!   G6-a  存在**时间冷却**常量与判定（不是永久拉黑）
//!   G6-b  `init_identity` 必须有「冷却到期自动复位 attempt」的分支
//!   G6-c  `should_check` 在冷却到期后必须放行（不得无条件 false）
//!   G6-d  存在**显式恢复入口**（命令 + 前端按钮），且已注册到 invoke_handler
//!   G6-e  抑制状态对用户**可见**（前端有提示分支与 UI 节点）
//!
//! ⚠ 本测试是**静态源码断言**：它锁定「结构与接线存在」，不代替真机行为验证。
//!   之所以这样选：updater 的真实路径需要系统授权弹窗，无法在 CI 无头环境触发；
//!   而本缺陷的本质是「缺少复位分支」，静态断言恰好能精确锁定这一点。

use std::fs;
use std::path::PathBuf;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    fs::read_to_string(manifest_dir().join(rel)).unwrap_or_else(|e| panic!("读取 {} 失败: {}", rel, e))
}

#[test]
fn g6a_update_guard_has_time_based_cooldown() {
    let u = read("src/update.rs");
    assert!(
        u.contains("GUARD_COOLDOWN_SECS"),
        "G6-a FAIL 缺少冷却常量 —— 抑制又被写成永久拉黑"
    );
    assert!(
        u.contains("fn cooldown_elapsed"),
        "G6-a FAIL 缺少 cooldown_elapsed 判定"
    );
    // 冷却必须是**有限**值，不能是 u64::MAX 之类的「永久」变体
    let line = u
        .lines()
        .find(|l| l.contains("const GUARD_COOLDOWN_SECS"))
        .expect("G6-a FAIL 未找到 GUARD_COOLDOWN_SECS 定义");
    // 冷却必须是**有限正数**：解析出实际数值再判断（不用字符串包含 —— 那会把 `60;` 误判为 `0;`）。
    let num: u64 = line
        .split('=')
        .nth(1)
        .map(|s| s.trim().trim_end_matches(';'))
        .and_then(|s| {
            // 支持 `6 * 60 * 60` 这类表达式
            let mut acc: u64 = 1;
            let mut seen = false;
            for part in s.split('*') {
                if let Ok(v) = part.trim().parse::<u64>() { acc = acc.saturating_mul(v); seen = true; }
            }
            if seen { Some(acc) } else { None }
        })
        .expect("G6-a FAIL 无法解析冷却时长");
    assert!(
        num > 0 && num < 365 * 24 * 60 * 60,
        "G6-a FAIL 冷却时长不合理（{}s）—— 0 或超过一年都等于「永久」",
        num
    );
    eprintln!("G6-a PASS 冷却为有限时长: {}", line.trim());
}

#[test]
fn g6b_init_identity_resets_attempt_after_cooldown() {
    let u = read("src/update.rs");
    // 必须存在「冷却到期 → attempt 归零」的复位分支
    assert!(
        u.contains("cooldown_elapsed()") && u.contains("g.attempt = 0;"),
        "G6-b FAIL 缺少「冷却到期自动复位 attempt」的分支 —— 这正是自锁的根因"
    );
    // 复位必须发生在 attempt>=MAX_ATTEMPTS 的语境里（不能是别处的无关赋零）
    let reset_ctx = u.contains("g.attempt >= MAX_ATTEMPTS && g.cooldown_elapsed()");
    assert!(
        reset_ctx,
        "G6-b FAIL 复位条件不是「达到上限且冷却已过」"
    );
    eprintln!("G6-b PASS init_identity 有冷却复位分支");
}

#[test]
fn g6c_should_check_releases_after_cooldown() {
    let u = read("src/update.rs");
    // should_check 里 attempt 闸必须**有条件**放行，而不是无条件 return false
    let sc = u
        .split("pub fn should_check")
        .nth(1)
        .expect("G6-c FAIL 未找到 should_check");
    let body = sc.split("\n}").next().unwrap_or(sc);
    assert!(
        body.contains("cooldown_elapsed()"),
        "G6-c FAIL should_check 未检查冷却 —— 到期也不会放行（自锁不变）"
    );
    assert!(
        body.contains("return (true, String::new())"),
        "G6-c FAIL should_check 没有放行路径"
    );
    eprintln!("G6-c PASS should_check 在冷却到期后放行");
}

#[test]
fn g6d_explicit_recovery_entry_exists_and_is_registered() {
    let u = read("src/update.rs");
    let c = read("src/commands/mod.rs");
    let m = read("src/main.rs");
    assert!(u.contains("pub fn reset_guard"), "G6-d FAIL update.rs 缺 reset_guard");
    assert!(
        c.contains("fn shell_reset_update_guard"),
        "G6-d FAIL commands 缺 shell_reset_update_guard 命令"
    );
    assert!(
        m.contains("commands::shell_reset_update_guard"),
        "G6-d FAIL 命令未注册到 invoke_handler（前端永远调不到）"
    );
    eprintln!("G6-d PASS 显式恢复入口存在且已注册");
}

#[test]
fn g6e_suppression_is_visible_to_user() {
    let html = read("bootstrap/bootstrap.html");
    let js = read("bootstrap/js/40-shell-update.js");
    assert!(
        html.contains("id=\"updSuppressed\""),
        "G6-e FAIL HTML 缺 updSuppressed 节点 —— 抑制依然不可见"
    );
    assert!(
        html.contains("btnUpdRetrySuppressed"),
        "G6-e FAIL HTML 缺「立即恢复」按钮"
    );
    assert!(
        js.contains("showUpdateSuppressed"),
        "G6-e FAIL 前端未接入 showUpdateSuppressed"
    );
    assert!(
        js.contains("shell_reset_update_guard"),
        "G6-e FAIL 前端未调用恢复命令"
    );
    eprintln!("G6-e PASS 抑制对用户可见且有恢复按钮");
}

/// G6-f：**pinned 抑制不得在「更新成功后」变成永久自锁**（P0 回归，2026-09-12）。
///
/// ## 修复的缺陷
///
/// `pinned` 只在「尝试失败达阈值」时写入**目标版本**（情况 B），
/// 而清空点**只有** `reset_guard()`（手动恢复）——
/// `confirmed` 分支只清 `pending_version`/`attempt`，**不清 pinned**。
///
/// 而 `should_check()` 的 pinned 分支**不查冷却**：
///
/// ```text
///   ① 更新 v2 失败 → pin("v2")
///   ② 6h 冷却后重试 → **成功**跑到 v2（confirmed 清 attempt 但 pinned 仍 = ["v2"]）
///   ③ 此后 should_check("v2") 命中 pinned 分支 → 永久 false
/// ```
///
/// 即：P1-A 要消除的「永久自锁」以**另一条路径**复现。
/// 原注释称「拉黑的是目标版本、比对的是当前版本、两者永不相等故形同虚设」，
/// 但**更新一旦成功 current == target**，该分支立刻生效且**没有期限**。
///
/// ## 锁定不变量
///   G6-f  `init_identity` 必须在「当前运行版本曾被拉黑」时**解除拉黑**
///   G6-g  解除必须发生在 if/else 链**之后**（覆盖「手动装了该版本」的情形），
///          而非只在 confirmed 分支内
///   G6-h  抑制语义保留：情况 B 仍会在失败达阈值时重新 pin
#[test]
fn g6f_pinned_is_cleared_once_running() {
    let u = read("src/update.rs");

    // G6-f：存在「运行版本被拉黑 → 解除」的处理
    assert!(
        u.contains("曾被抑制，现已解除"),
        "G6-f FAIL init_identity 未在运行版本曾被拉黑时解除 —— 更新成功后变为永久自锁"
    );
    assert!(
        u.contains("g.pinned.retain(|p| p != version)"),
        "G6-f FAIL 缺少 pinned 的移除逻辑"
    );

    // G6-g：该解除必须在 else-if 链**结束之后**（不在 confirmed 分支内）
    let i_chain_end = u.find("g.attempt = 0;\n        g.save();\n    }\n").unwrap_or(usize::MAX);
    let i_unpin = u.find("曾被抑制，现已解除").unwrap_or(usize::MAX);
    assert!(
        i_unpin != usize::MAX && i_chain_end != usize::MAX && i_unpin > i_chain_end,
        "G6-g FAIL 解除逻辑应在 if/else 链之后（覆盖手动安装该版本的情形）"
    );

    // G6-h（反向）：失败达阈值时仍会 pin —— 抑制语义未被删掉
    assert!(
        u.contains("g.pinned.push(target.clone())"),
        "G6-h FAIL 失败达阈值时不再 pin —— 抑制语义被误删"
    );

    // G6-f（反向）：confirmed 分支内**不得**再有一份专用的 retain（避免两处实现分叉）
    let confirmed_block = u
        .split("// 情况 B")
        .next()
        .unwrap_or("");
    let head = confirmed_block.rsplit("if confirmed {").next().unwrap_or("");
    assert!(
        !head.contains("pinned.retain"),
        "G6-f FAIL confirmed 分支内不应有第二份 retain（同一事实一处实现）"
    );

    eprintln!("G6-f/g/h PASS pinned 在更新成功后被解除，且抑制语义保留");
}

/// G6-i：跨仓 `probe` 契约的 `timeoutMs` 必须与壳实际探测超时**同源**。
///
/// `probe` 字段的全部目的就是让壳与内核**选源一致**：
/// 壳按它测速并导出选择结果，内核按它复测。若两侧超时不同，
/// 落在两者之间的源会被一方判可达、另一方判不可达 → 选源分叉。
///
/// 原实现：`probe_all` 用 `PROBE_TIMEOUT`(8s)，而导出契约硬编码 `6000` —— 典型的
/// 「同一事实两处实现且已分叉」。现改为由同一常量派生。
#[test]
fn g6i_probe_timeout_is_single_source() {
    let m = read("src/mirror.rs");
    assert!(
        !m.contains("\"timeoutMs\": 6000"),
        "G6-i FAIL 契约仍硬编码 6000ms —— 与 PROBE_TIMEOUT(8s) 分叉，选源会不一致"
    );
    assert!(
        m.contains("PROBE_TIMEOUT.as_millis() as u64"),
        "G6-i FAIL timeoutMs 未由 PROBE_TIMEOUT 派生（单一事实源）"
    );
    // 反向：确认壳真的用该常量做探测（否则派生也没意义）
    assert!(
        m.contains(".timeout(PROBE_TIMEOUT)"),
        "G6-i FAIL probe_all 未使用 PROBE_TIMEOUT"
    );
    eprintln!("G6-i PASS 跨仓 probe 超时单一事实源");
}

/// G6-j：Node 安装的镜像回退**不得被 SHASUMS 失败中断**。
///
/// 原实现 `String::from_utf8(http_get_bytes(..SHASUMS256.txt..)?)` —— 外层 `?` 让
/// **网络失败直接 return**，下面的 `continue` 只覆盖非 UTF-8 情形。
/// `SHASUMS256.txt` 瞬时抽风即中断整条回退链，即使后续镜像健康。
/// 同处还有第二例：条目未找到时的 `ok_or_else(..)?` 也直接 return。
#[test]
fn g6j_shasums_failure_does_not_abort_fallback() {
    let n = read("src/node.rs");
    assert!(
        !n.contains("http_get_bytes(&format!(\"{}/{}/SHASUMS256.txt\", base, version))?"),
        "G6-j FAIL SHASUMS 下载仍用 `?` —— 网络失败会中断镜像回退"
    );
    assert!(
        n.contains("SHASUMS 下载失败") && n.contains("SHASUMS256.txt 中未找到条目"),
        "G6-j FAIL SHASUMS 的两条失败路径都应 continue 到下一个源"
    );
    // 反向：确认「换下一个源」语义仍在（不能在失败处直接返回）
    assert!(
        n.contains("SHASUMS") && n.contains("last_err = Some("),
        "G6-j FAIL 失败时应记 last_err 并继续（而非 return）"
    );
    eprintln!("G6-j PASS SHASUMS 失败不中断镜像回退");
}

/// G6-k：**退出握手不得在 UI 线程执行**（两条路径都必须 offload）。
///
/// `guardctl::shutdown_all` 最坏约 70 秒（`/session/stop` 60s + 轮询）。
/// 在 UI 线程做会让窗口假死，用户强杀 → 跳过握手 → 留下未停的 DSH。
///
/// 原实现：托盘 quit 路径正确地 spawn 到线程，而 `closeAction=exit`
/// 的关窗路径**直接在回调里同步执行** —— 同一纪律只覆盖两条路径中的一条。
#[test]
fn g6k_exit_handshake_is_off_ui_thread() {
    let m = read("src/main.rs");
    // 关窗 exit 路径必须 prevent_close + spawn
    assert!(
        m.contains("api.prevent_close();"),
        "G6-k FAIL 关窗 exit 路径缺 prevent_close（最后一个窗口关闭可能让进程先退出、握手被截断）"
    );
    assert!(
        m.contains("let h = window.app_handle().clone();"),
        "G6-k FAIL 关窗 exit 路径未把退出握手 offload 到线程（UI 会假死 ~70s）"
    );
    // 反向：不得再有「在 on_window_event 里同步调 shutdown_all 后 exit」的形态
    assert!(
        !m.contains("domain::guardctl::shutdown_all(port);\n                    app.exit(0);"),
        "G6-k FAIL 仍存在 UI 线程同步退出握手"
    );
    // 托盘路径的既有纪律仍在
    assert!(
        m.contains("let h = app.clone();"),
        "G6-k FAIL 托盘 quit 路径的 offload 丢失"
    );
    eprintln!("G6-k PASS 两条退出路径都把握手 offload 到后台线程");
}