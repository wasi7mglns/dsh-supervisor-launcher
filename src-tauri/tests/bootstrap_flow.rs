//! 引导流程回归（2026-09-11）。
//!
//! 背景：引导页曾把「桌面壳自更新」当作**第一步**（旧称「门 0」），理由是「新壳才可能带有
//! 新的 Node/内核安装要求」。该前提不成立（环境检测与 Node 探测都是本地判定，与壳版本无关），
//! 且把最慢最不可靠的**网络**步骤放在首位 —— Windows 真机实测直接卡死在「正在检查桌面更新」，
//! 用户无法进入产品。
//!
//! 本测试锁定修复后的不变量，防止静默回归：
//!   B1 步骤顺序：检测环境 → 运行环境 → 桌面更新 → 内核版本 → 守卫就绪 → 进入面板
//!   B2 HTML 步骤条顺序与 JS stepNames 数组一致（两处不同步会导致高亮错位）
//!   B3 引导从「检测环境」启动，不再从桌面更新启动
//!   B4 网络步骤（检查/下载）必须有超时兜底与可跳过出口 —— 否则底层挂起即永久卡死
//!   B5 下载必须有进度反馈（此前回调体为空，用户无法区分「在下载」与「卡死」）
//!   B6 Rust 侧：check 有超时、mark_pending 在 install 之前（Windows 上 install 不返回）
//!   B7 面向用户的文案不得再出现「门 0」这一内部概念

use std::fs;
use std::path::PathBuf;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn bootstrap_html() -> String {
    fs::read_to_string(manifest_dir().join("bootstrap").join("bootstrap.html"))
        .expect("read bootstrap.html")
}

fn main_rs() -> String {
    fs::read_to_string(manifest_dir().join("src").join("main.rs")).expect("read main.rs")
}

/// 按出现位置给步骤 id 排序（取 HTML 中 id="st-xxx" 的声明顺序）。
fn html_step_order(html: &str) -> Vec<String> {
    let mut found: Vec<(usize, String)> = Vec::new();
    for id in ["st-env", "st-node", "st-shell", "st-core", "st-guard", "st-panel"] {
        let needle = format!("id=\"{}\"", id);
        if let Some(pos) = html.find(&needle) {
            found.push((pos, id.to_string()));
        }
    }
    found.sort_by_key(|(p, _)| *p);
    found.into_iter().map(|(_, s)| s).collect()
}

const EXPECTED: [&str; 6] = ["st-env", "st-node", "st-shell", "st-core", "st-guard", "st-panel"];

#[test]
fn b1_step_order_env_first_shell_after_env() {
    let html = bootstrap_html();
    let order = html_step_order(&html);
    assert_eq!(order, EXPECTED, "B1 FAIL 步骤顺序不符（实测 {:?}）", order);
    eprintln!("B1 PASS step order: {:?}", order);
}

#[test]
fn b2_js_stepnames_matches_html_order() {
    let html = bootstrap_html();
    let order = html_step_order(&html);
    // 源码用单引号；两种引号都接受，避免因风格调整误报。
    let single = format!(
        "var stepNames = [{}];",
        EXPECTED.iter().map(|s| format!("\x27{}\x27", s)).collect::<Vec<_>>().join(", ")
    );
    let double = format!(
        "var stepNames = [{}];",
        EXPECTED.iter().map(|s| format!("\"{}\"", s)).collect::<Vec<_>>().join(", ")
    );
    assert!(
        html.contains(&single) || html.contains(&double),
        "B2 FAIL JS stepNames 与 HTML 顺序不一致；期望 {} 或 {}",
        single,
        double
    );
    assert_eq!(order.len(), 6, "B2 FAIL HTML 未声明全部 6 个步骤");
    eprintln!("B2 PASS stepNames matches HTML order");
}

#[test]
fn b3_boot_starts_from_env_not_shell_update() {
    let html = bootstrap_html();
    assert!(html.contains("stepEnv().catch"), "B3 FAIL boot() 未从 stepEnv 启动");
    assert!(
        !html.contains("stepShellUpdate().catch"),
        "B3 FAIL boot() 仍从桌面更新启动（旧缺陷：把网络步骤放在首位）"
    );
    // 环境就绪后才进入桌面更新
    assert!(
        html.contains("then(stepShellUpdate)"),
        "B3 FAIL 未看到「环境 → 桌面更新」的衔接"
    );
    eprintln!("B3 PASS boot starts from env, shell update after env");
}

#[test]
fn b4_network_steps_have_timeout_and_skip() {
    let html = bootstrap_html();
    assert!(html.contains("function withTimeout"), "B4 FAIL 缺少前端超时兜底（withTimeout）");
    assert!(html.contains("SHELL_CHECK_BUDGET_MS"), "B4 FAIL 缺少检查超时预算");
    assert!(html.contains("SHELL_DOWNLOAD_BUDGET_MS"), "B4 FAIL 缺少下载超时预算");
    assert!(html.contains("btnSkipShell"), "B4 FAIL 缺少「跳过桌面更新」出口按钮");
    assert!(html.contains("showSkip"), "B4 FAIL 跳过出口未接线");
    eprintln!("B4 PASS timeout + skip exit present");
}

#[test]
fn b5_download_progress_is_wired() {
    let html = bootstrap_html();
    assert!(
        html.contains("shell_update_progress"),
        "B5 FAIL 前端未监听下载进度事件（下载阶段将零反馈）"
    );
    assert!(html.contains("progBar"), "B5 FAIL 缺少进度条元素");
    let m = main_rs();
    assert!(
        m.contains("shell_update_progress"),
        "B5 FAIL Rust 侧未上报下载进度"
    );
    eprintln!("B5 PASS download progress wired");
}

#[test]
fn b6_rust_check_timeout_and_pending_before_install() {
    let m = main_rs();
    assert!(m.contains("SHELL_CHECK_TIMEOUT"), "B6 FAIL 缺少检查超时常量");
    assert!(m.contains("SHELL_DOWNLOAD_TIMEOUT"), "B6 FAIL 缺少下载超时常量");
    assert!(m.contains("tokio::time::timeout"), "B6 FAIL 未使用 tokio 超时包裹网络调用");
    // Windows 上 install 会结束本进程，故 mark_pending 必须在其之前
    let mp = m.find("update::mark_pending").expect("B6 FAIL 未找到 mark_pending");
    let inst = m.find("u.install(").expect("B6 FAIL 未找到 u.install");
    assert!(mp < inst, "B6 FAIL mark_pending 必须在 install 之前（Windows 上 install 不返回）");
    eprintln!("B6 PASS timeout present, mark_pending before install");
}

#[test]
fn b7_no_stale_door0_concept_in_user_facing_text() {
    let html = bootstrap_html();
    // 允许出现在「纠正说明」注释里，但不得出现在面向用户的元素文本中。
    for bad in ["门 0", "门0"] {
        let mut idx = 0;
        while let Some(p) = html[idx..].find(bad) {
            let abs = idx + p;
            let line_start = html[..abs].rfind(char::is_whitespace).map(|x| x + 1).unwrap_or(0);
            let line = &html[line_start..html[abs..].find(char::is_whitespace).map(|x| abs + x).unwrap_or(html.len())];
            let is_comment = line.trim_start().starts_with("<!--") || line.trim_start().starts_with("//") || line.contains("旧称");
            assert!(is_comment, "B7 FAIL 用户可见文本仍含内部概念「{}」：{}", bad, line.trim());
            idx = abs + bad.len();
        }
    }
    eprintln!("B7 PASS no stale door-0 concept in user-facing text");
}
