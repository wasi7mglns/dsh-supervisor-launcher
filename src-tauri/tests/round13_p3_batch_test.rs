//! 壳仓 P3 批：平台事实判定 / 声明与实测同源 / 前端如实告知 / 死广播清理（2026-09-13）
//!
//! ## 缺陷
//!
//! ① P3 \`--service-plan\` 在 Windows 恒报「现存 = 否」
//!    main.rs 原用 \`platform::service().definition_path().is_file()\`，而
//!    windows.rs 的 definition_path 是**标识串** \`schtasks://DSH-Supervisor\`（没有文件）
//!    → \`is_file()\` 恒 false → 自检无论计划任务是否存在/刚建立都报「否」，
//!    把排障者方向带偏（本自检正是「服务定义」能力的官方验证入口）。
//!
//! ② P3 Linux \`capabilities().privilege_channel\` 硬编码 true（失效模式 b）
//!    而 \`has_privilege_channel()\` 会真的探测 pkexec/sudo。无两者之一的机器上，
//!    \`--platform-matrix\` 报 true，而 \`--shell-update-plan\` 的 self_update_capable=false、
//!    Node 安装会失败 —— 同一事实两个相反答案。
//!
//! ③ P3 引导页谎报「内核已是最新」（失效模式 f）
//!    core.rs 的 build_plan 在远端版本查询失败时输出 action=unknown **且带 error 原因**，
//!    而前端在 installed 非空时不看 p.error，直接走「已是最新」分支 →
//!    离线时用户被告知「已是最新」，掩盖「这次没查成」。
//!
//! ④ P3 \`env_status\` 死广播（失效模式 c/f）
//!    全仓零监听，且其 latest 已由 node_status 轮询（单一事实源）提供 →
//!    再加监听会制造第二真源，故删除。
//!
//! ## 锁定不变量
//!   P-a  \`--service-plan\` 经 ServiceControl::is_defined()（平台事实判定），
//!        且 Windows 覆写为真实查询；trait 默认实现仍适用于文件型平台
//!   P-b  Linux 的 capabilities().privilege_channel 与 has_privilege_channel() 同源
//!   P-c  前端消费 core_plan 的 error 字段（不再谎报「已是最新」）
//!   P-d  env_status 死广播已删除；env_done 有接收方（不再是死广播）
//!   P-e  反向：判据能识别旧形态（门禁非空转）

use std::fs;
use std::path::PathBuf;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    fs::read_to_string(manifest_dir().join(rel)).unwrap_or_else(|e| panic!("读取 {} 失败: {}", rel, e))
}

/// 剥离 \`//\` 整行注释（本仓多次被自己的说明文字骗过）。
fn strip_comments(src: &str) -> String {
    src.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn p_a_service_plan_uses_fact_based_is_defined() {
    let main = strip_comments(&read("src/main.rs"));
    assert!(
        !main.contains("definition_path().is_file()"),
        "P-a FAIL --service-plan 仍用 definition_path().is_file()（Windows 上恒 false）"
    );
    assert!(
        main.contains("platform::service().is_defined()"),
        "P-a FAIL --service-plan 未改用 ServiceControl::is_defined()"
    );
    // trait 有默认实现（文件型平台适用）
    let svc = strip_comments(&read("src/platform/service.rs"));
    assert!(
        svc.contains("fn is_defined(&self) -> bool {"),
        "P-a FAIL ServiceControl 缺少 is_defined 的默认实现"
    );
    assert!(
        svc.contains("self.definition_path().is_file()"),
        "P-a FAIL 默认实现应基于文件存在性（Linux unit / macOS plist 适用）"
    );
    // Windows 覆写为真实查询
    let win = strip_comments(&read("src/platform/windows.rs"));
    let i = win.find("fn is_defined(&self) -> bool {").expect("P-a FAIL Windows 未覆写 is_defined");
    let block = &win[i..(i + 400).min(win.len())];
    assert!(
        block.contains("\"/Query\"") && block.contains("GUARD_TASK"),
        "P-a FAIL Windows 的 is_defined 未用 schtasks /Query 做真实判定"
    );
}

#[test]
fn p_b_linux_capability_matches_probe() {
    let lin = strip_comments(&read("src/platform/linux.rs"));
    assert!(
        lin.contains("privilege_channel: self.has_privilege_channel()"),
        "P-b FAIL Linux 的 privilege_channel 未与实际探测同源"
    );
    assert!(
        !lin.contains("privilege_channel: true"),
        "P-b FAIL Linux 的 privilege_channel 仍硬编码 true"
    );
}

#[test]
fn p_c_frontend_consumes_core_plan_error() {
    let js = strip_comments(&read("bootstrap/js/50-kernel.js"));
    assert!(
        js.contains("p.error"),
        "P-c FAIL 前端未消费 core_plan 的 error 字段（远端查询失败会被谎报为「已是最新」）"
    );
    // 反向：确认「已是最新」分支仍在（不能把正常路径也改坏）
    assert!(
        js.contains("内核已是最新"),
        "P-c FAIL 正常路径的「内核已是最新」文案丢失了"
    );
}

#[test]
fn p_d_dead_broadcast_removed_and_env_done_consumed() {
    let main = strip_comments(&read("src/main.rs"));
    let cmds = strip_comments(&read("src/commands/mod.rs"));
    let init = strip_comments(&read("bootstrap/js/80-init.js"));
    assert!(
        !main.contains("env_status"),
        "P-d FAIL env_status 死广播仍在（零监听，且与 node_status 构成第二真源）"
    );
    assert!(
        cmds.contains("env_done"),
        "P-d FAIL env_done 的发射点不应被删除（它是完成事件）"
    );
    assert!(
        init.contains("listen('env_done'"),
        "P-d FAIL env_done 仍无接收方（死广播）"
    );
}

#[test]
fn p_e_offender_detector_is_not_vacuous() {
    // 反向：判据能识别旧形态
    assert!(
        "let x = platform::service().definition_path().is_file();".contains("definition_path().is_file()"),
        "P-e FAIL 判据无法识别旧的 is_file 形态"
    );
    assert!(
        "privilege_channel: true,".contains("privilege_channel: true"),
        "P-e FAIL 判据无法识别硬编码 true"
    );
    // 且修复后的形态不被误报
    let lin = strip_comments(&read("src/platform/linux.rs"));
    assert!(
        !lin.contains("privilege_channel: true"),
        "P-e FAIL 已修复的形态仍被误报"
    );
}
