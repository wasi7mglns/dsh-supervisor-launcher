//! CI 门禁**覆盖性**门禁（2026-09-13）。
//!
//! ## 修复的缺陷
//!
//! 壳仓 CI 的「门禁测试」步骤原先**硬编码**枚举 test target
//! （\`cargo test --bins --test bootstrap_flow --test update_guard_test --test platform_unsupported_structure_test\`），
//! 于是**新增的 \`tests/*.rs\` 被静默排除在 CI 之外**。实测漏了 4 个：
//!   · \`platform_shared_items_test\` —— 正是为「平台文件用了父模块项却没导入
//!     → macOS E0425」建立的那道门禁（本轮 P1 的主角）；
//!   · \`mirror_env_wiring_test\`、\`round13_p3_batch_test\`（本轮新增）；
//!   · \`service_self_heal_test\`（**既有的**，一直没在 CI 跑）。
//!
//! 讽刺的是，该步骤自己的注释就写着「本仓 60+ 个门禁……**此前从未在 CI 执行**」——
//! 同类问题以「新增文件被硬编码列表漏掉」的形式**又复发了**。
//!
//! ## 另一个相关缺口
//!
//! \`macos.rs\` / \`windows.rs\` 被 \`#[cfg(target_os)]\` 条件编译，**Linux 上从不参与编译**，
//! 而 CI 原先只在 tag 时跑 → 日常推送**没有任何 mac/win 编译校验**。
//!
//! ## 锁定不变量
//!   C-a  CI 门禁步骤必须**自动枚举** \`tests/*.rs\`（不得硬编码 target 列表）
//!   C-b  唯一允许排除的 target 是 \`updater_artifacts\`（它需打包产物 SHELL_REHEARSAL_DIR）
//!   C-c  CI 必须存在 **macOS + Windows 的编译检查**（\`cargo check --all-targets\`）
//!   C-d  反向：判据能识别「硬编码 --test 列表」的旧形态（门禁非空转）

use std::fs;
use std::path::PathBuf;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn ci() -> String {
    let p = manifest_dir().join("..").join(".github").join("workflows").join("build.yml");
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("读取 {:?} 失败: {}", p, e))
}

/// 去掉整行 \`#\` 注释（YAML 注释）—— 本仓多次被自己的说明文字骗过。
fn strip_yaml_comments(src: &str) -> String {
    src.lines().filter(|l| !l.trim_start().starts_with('#')).collect::<Vec<_>>().join("\n")
}

/// 是否存在「硬编码的 --test 目标名」（除 updater_artifacts 外）。抽出为函数以便反向断言复用。
fn has_hardcoded_test_targets(yaml: &str) -> bool {
    yaml.lines()
        .filter(|l| l.contains("--test "))
        // 自动枚举那一行里也含 "--test "（在 sed 表达式内），但它同时含 "sed" 与 "TARGETS"
        .filter(|l| !(l.contains("sed") && l.contains("TARGETS")))
        .any(|l| !l.contains("updater_artifacts"))
}

#[test]
fn c_a_ci_gate_step_auto_enumerates_test_targets() {
    let y = strip_yaml_comments(&ci());
    assert!(
        y.contains("ls tests/*.rs"),
        "C-a FAIL CI 门禁步骤未自动枚举 tests/*.rs —— 新增门禁文件会被静默排除在 CI 之外"
    );
    assert!(
        y.contains("TARGETS") && y.contains("cargo test --bins"),
        "C-a FAIL 未把枚举结果传给 cargo test"
    );
    assert!(
        !has_hardcoded_test_targets(&y),
        "C-a FAIL 仍存在硬编码 --test 枚举（会漏掉新增门禁）"
    );
}

#[test]
fn c_b_only_updater_artifacts_is_excluded() {
    let y = strip_yaml_comments(&ci());
    assert!(
        y.contains("grep -v '^updater_artifacts$'"),
        "C-b FAIL 未按预期排除 updater_artifacts（或排除写法变了，需同步本门禁）"
    );
    // 反向：不得出现第二个排除项（防止排除列表被扩大成新的漏网口）
    for l in y.lines().filter(|l| l.contains("grep -v")) {
        assert!(
            l.matches("grep -v").count() == 1,
            "C-b FAIL 出现了多个 grep -v 排除（排除列表被扩大）：{}",
            l.trim()
        );
    }
}

#[test]
fn c_c_ci_compiles_macos_and_windows() {
    let y = strip_yaml_comments(&ci());
    assert!(
        y.contains("macos-latest") && y.contains("windows-latest"),
        "C-c FAIL CI 未覆盖 macOS/Windows runner —— macos.rs/windows.rs 永不被编译"
    );
    assert!(
        y.contains("cargo check --all-targets"),
        "C-c FAIL CI 缺少 cargo check --all-targets（mac/win 平台分支的编译门禁）"
    );
    assert!(
        y.contains("platform-check"),
        "C-c FAIL 缺少轻量平台检查 job（日常 push 的 mac/win 编译门禁）"
    );
}

#[test]
fn c_d_offender_detector_is_not_vacuous() {
    // 反向：判据能识别「硬编码 --test 列表」的旧形态
    let old = "cargo test --bins --test bootstrap_flow --test update_guard_test --test platform_unsupported_structure_test 2>&1 | tail -80";
    assert!(
        has_hardcoded_test_targets(old),
        "C-d FAIL 判据无法识别硬编码 --test 列表 —— 门禁空转"
    );
    // 且修复后的形态不被误报
    let new = "TARGETS=$(ls tests/*.rs | sed 's#tests/##' | grep -v '^updater_artifacts$' | sed 's/^/--test /' | tr '\n' ' ')";
    assert!(
        !has_hardcoded_test_targets(new),
        "C-d FAIL 修复后的自动枚举行被误报为硬编码"
    );
    // 而「只写 updater_artifacts」的单独步骤不算硬编码（它是唯一允许的排除项）
    assert!(
        !has_hardcoded_test_targets("cargo test --test updater_artifacts -- --nocapture"),
        "C-d FAIL updater_artifacts 的单独步骤被误报"
    );
}
