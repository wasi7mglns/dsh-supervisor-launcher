//! CI 覆盖性门禁（2026-09-13）。
//!
//! ## 缺陷 ①（门禁漏跑）
//!
//! 壳仓 CI 的「门禁测试」步骤原先硬编码枚举 test target：
//!   cargo test --bins --test bootstrap_flow --test update_guard_test --test platform_unsupported_structure_test
//! 于是新增的 tests/*.rs 被静默排除在 CI 之外。实测漏了 4 个：
//!   · platform_shared_items_test —— 正是为「平台文件用了父模块项却没导入
//!     → macOS E0425」建立的那道门禁（建了却不在 CI 跑）；
//!   · mirror_env_wiring_test、round13_p3_batch_test（本轮新增）；
//!   · service_self_heal_test（既有的，一直没在 CI 跑）。
//! 该步骤自己的注释就写着「60+ 个门禁……此前从未在 CI 执行」——同类问题又复发了。
//!
//! ## 缺陷 ②（轻量替代 = 掩盖问题）
//!
//! 中途曾引入一个只做 cargo check 的轻量 job 并给 build 加 tag 守卫，
//! 让日常 push 走轻量路径。这是错的：仅编译校验发现不了打包/签名/产物装配阶段的问题
//! （glibc 基座、bundle 装配、验签清单、UPDATE 资产），而本仓恰在那些阶段踩过坑；
//! 轻量检查给出「绿了」的假象，反而掩盖问题。
//! 现按明确要求修订为：push main 直接跑完整构建矩阵（不设轻量替代、不设 tag 守卫）。
//!
//! ## 锁定不变量
//!   C-a  CI 门禁步骤必须自动枚举 tests/*.rs（不得硬编码 target 列表）
//!   C-b  唯一允许排除的 target 是 updater_artifacts（它需打包产物 SHELL_REHEARSAL_DIR）
//!   C-c  push 时必须跑完整构建矩阵：build job 不得被 if: 守卫限制；
//!        matrix 必须含 macOS 与 Windows；且 build 内要跑 cargo test
//!   C-d  反悔防护：不得存在「只做 cargo check、不做构建」的轻量 job 替代完整构建
//!   C-e  反向：判据能识别旧形态（门禁非空转）

use std::fs;
use std::path::PathBuf;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn ci() -> String {
    let p = manifest_dir().join("..").join(".github").join("workflows").join("build.yml");
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("读取 {:?} 失败: {}", p, e))
}

/// 去掉整行 # 注释（YAML 注释）—— 本仓多次被自己的说明文字骗过。
fn strip_yaml_comments(src: &str) -> String {
    src.lines().filter(|l| !l.trim_start().starts_with('#')).collect::<Vec<_>>().join("\n")
}

/// 是否存在「硬编码的 --test 目标名」（除 updater_artifacts 外）。
fn has_hardcoded_test_targets(yaml: &str) -> bool {
    yaml.lines()
        .filter(|l| l.contains("--test "))
        // 自动枚举那一行里也含 "--test "（在 sed 表达式内），但它同时含 "sed" 与 "TARGETS"
        .filter(|l| !(l.contains("sed") && l.contains("TARGETS")))
        .any(|l| !l.contains("updater_artifacts"))
}

/// 抽取某个顶层 job 的文本块（从两空格缩进的 name: 到下一个顶层 job 或文件尾）。
fn job_block(yaml: &str, name: &str) -> String {
    let marker = format!("\n  {}:\n", name);
    let start = match yaml.find(&marker) {
        Some(i) => i + 1,
        None => return String::new(),
    };
    let rest = &yaml[start..];
    let mut end = rest.len();
    for (idx, _line) in rest.match_indices("\n  ") {
        // 仅当该位置确为「一个新的顶层 job」：下一行形如  <name>: 且其后是行尾
        let after = &rest[idx + 1..];
        let first = after.lines().next().unwrap_or("");
        if first.starts_with("  ") && !first.starts_with("   ") && first.trim_end().ends_with(':') {
            end = idx;
            break;
        }
    }
    rest[..end].to_string()
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
    for l in y.lines().filter(|l| l.contains("grep -v")) {
        assert!(
            l.matches("grep -v").count() == 1,
            "C-b FAIL 出现了多个 grep -v 排除（排除列表被扩大）：{}",
            l.trim()
        );
    }
}

#[test]
fn c_c_full_build_runs_on_push_for_all_platforms() {
    let y = strip_yaml_comments(&ci());
    assert!(
        y.contains("branches: [main]" ) || y.contains("branches: ['main']"),
        "C-c FAIL push main 未触发 CI"
    );

    let build = job_block(&y, "build");
    assert!(!build.is_empty(), "C-c FAIL 未找到 build job");

    // ① 不得有 **job 级** if: 守卫（否则日常 push 会跳过完整构建）。
    //    ⚠ 只认**恰好 4 空格缩进**的 if: —— job 级键缩进 4，步骤级在 steps 下缩进 8。
    //      第一版用 trim_start() 判会命中步骤级 if（如「tag 才上传 Release 资产」），假红。
    let job_level_if: Vec<&str> = build
        .lines()
        .filter(|l| l.starts_with("    if:") && !l.starts_with("     "))
        .collect();
    assert!(
        job_level_if.is_empty(),
        "C-c FAIL build job 存在 job 级 if: 守卫（日常 push 会跳过完整构建，不得用轻量检查替代）：{:?}",
        job_level_if
    );

    // ② 完整矩阵必须含 macOS 与 Windows（否则 cfg 屏蔽的平台文件永不被编译）
    assert!(
        build.contains("macos-latest") && build.contains("windows-latest"),
        "C-c FAIL build 矩阵未覆盖 macOS/Windows —— 平台文件不会被编译，问题无法暴露"
    );

    // ③ 门禁测试必须随完整构建执行
    assert!(
        build.contains("cargo test"),
        "C-c FAIL build job 内未执行 cargo test —— 门禁不随构建跑"
    );
}

#[test]
fn c_d_no_lightweight_substitute_job() {
    let y = strip_yaml_comments(&ci());
    // 反悔防护：不得存在只做 cargo check、不做 build 的独立 job 来替代完整构建。
    for name in ["build", "platform-check", "check", "compile-check", "fast-check"] {
        let b = job_block(&y, name);
        if b.is_empty() {
            continue;
        }
        let has_check = b.contains("cargo check");
        let has_full = b.contains("cargo build") || b.contains("tauri build") || b.contains("tauri-action");
        assert!(
            !(has_check && !has_full),
            "C-d FAIL job {} 只做 cargo check 而不做完整构建 —— 这是轻量替代，会掩盖问题",
            name
        );
    }
    assert!(
        job_block(&y, "platform-check").is_empty(),
        "C-d FAIL 轻量 platform-check job 又回来了 —— 应以完整构建替代"
    );
}

#[test]
fn c_e_offender_detector_is_not_vacuous() {
    let old = "cargo test --bins --test bootstrap_flow --test update_guard_test --test platform_unsupported_structure_test 2>&1 | tail -80";
    assert!(
        has_hardcoded_test_targets(old),
        "C-e FAIL 判据无法识别硬编码 --test 列表 —— 门禁空转"
    );
    let new = "TARGETS=$(ls tests/*.rs | sed 's#tests/##' | grep -v '^updater_artifacts$' | sed 's/^/--test /')";
    assert!(
        !has_hardcoded_test_targets(new),
        "C-e FAIL 修复后的自动枚举行被误报为硬编码"
    );
    assert!(
        !has_hardcoded_test_targets("cargo test --test updater_artifacts -- --nocapture"),
        "C-e FAIL updater_artifacts 的单独步骤被误报"
    );
    // 反向：job_block 能正确定位（否则 C-c/C-d 空转）
    let sample = "jobs:\n  build:\n    runs-on: x\n    steps:\n      - run: cargo build\n\n  publish:\n    runs-on: y\n";
    let b = job_block(sample, "build");
    assert!(
        b.contains("cargo build") && !b.contains("runs-on: y"),
        "C-e FAIL job_block 定位错误，C-c/C-d 会空转：{:?}",
        b
    );
}
