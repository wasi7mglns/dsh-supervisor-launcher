//! 未知平台实现（`platform/unsupported.rs`）的** trait 方法归属**门禁（P1，2026-09-12）。
//!
//! ## 修复的缺陷
//!
//! 该文件把 `Platform` 的 11 个必需方法**写进了 `impl ServiceControl for Impl` 块内**：
//!
//! ```text
//!   impl ServiceControl for Impl {
//!       fn kind/definition_path/ensure_defined/start/stop/spawn_daemon   // ← 这 6 个是对的
//!       fn node_artifact/node_candidate_paths/... (11 个)              // ← 这些属于 Platform
//!   }
//! ```
//!
//! 两个方向都是编译错误：`impl Platform` 缺 11 个必需方法（E0046），
//! 而 ServiceControl 里多出 11 个不属于它的方法（E0407）。
//!
//! **为何从未暴露**：`platform/mod.rs` 用
//! `#[cfg(not(any(target_os = "linux", "macos", "windows")))]` 把整个模块排除 ——
//! 三大平台构建根本不编译它。于是「未知平台显式 Unsupported、绝不静默成功」的承诺
//! 在源码层不成立：任何移植/新增 target 的那一刻就编译失败。
//!
//! ## 为什么用静态结构断言
//!
//! 本仓库的 CI 只有三大平台工具链，无法编译该 cfg 分支；
//! 而缺陷的本质正是「方法放错了 impl 块」——**结构断言恰好能精确锁定它**。
//! （与同仓 `update_guard_test.rs` 对 updater 的选择同理。）
//!
//! ## 锁定不变量
//!   U-a  `impl ServiceControl for Impl` 只含 ServiceControl 的 6 个方法
//!   U-b  `impl Platform for Impl`（可多块）含 Platform 的**全部** 14 个方法
//!   U-c  两个集合**不相交**（同一方法不得同时出现在两个 trait 的 impl 里）
//!   U-d  真实 trait 定义的方法集与预期一致（防 trait 演进后本门禁失效）

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    fs::read_to_string(manifest_dir().join(rel)).unwrap_or_else(|e| panic!("读取 {} 失败: {}", rel, e))
}

/// 从 `trait X {` 块中抽取全部 `fn name`。
fn trait_methods(src: &str, trait_decl: &str) -> BTreeSet<String> {
    let start = src.find(trait_decl).unwrap_or_else(|| panic!("未找到 {}", trait_decl));
    // 找到 trait 块的结束（行首 `}`）
    let rest = &src[start..];
    let end = rest.find("\n}").map(|i| start + i).unwrap_or(src.len());
    extract_fn_names(&src[start..end])
}

/// 从 `impl ... for Impl {` 开始的块中抽取全部 `fn name`（按行首 `}` 收束）。
fn impl_methods(src: &str, impl_decl: &str) -> BTreeSet<String> {
    // ⚠ 先剥离注释再查找 —— 否则**说明文字里提到的** `impl X for Impl` 也会命中。
    //   （我第一版就踩了这个：本文件在 impl 内有一段长注释，
    //     逐字写着 "impl ServiceControl for Impl"，于是扫描越界到了下一个 impl。）
    //   调用方传的 decl 一律带 `{`（与真实声明同形），进一步避免误命中。
    let src = &strip_comments(src);
    let mut out = BTreeSet::new();
    let mut from = 0usize;
    while let Some(rel) = src[from..].find(impl_decl) {
        let start = from + rel;
        let rest = &src[start..];
        let end = rest.find("\n}").map(|i| start + i + 1).unwrap_or(src.len());
        out.extend(extract_fn_names(&src[start..end]));
        from = end;
    }
    out
}

/// 抽取 `fn <name>`（**剥离行注释与块注释**，避免文档里提到的名字被算进来）。
///
/// ⚠ 必须真正剥离注释，而不只是「跳过以 // 开头的行」：
///   本文件在 impl 块**内部**有一段长注释，逐行列举了方法名（说明归属问题），
///   那些名字会被 `strip_prefix("fn ")` 之外的形式带入吗？——不会；
///   但 _被注释掉的_ `fn xxx(...)` 变体会。故此处按块注释/行注释双剥离。
fn extract_fn_names(block: &str) -> BTreeSet<String> {
    let code = strip_comments(block);
    let mut out = BTreeSet::new();
    for line in code.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("fn ") {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                out.insert(name);
            }
        }
    }
    out
}

/// 剥离 Rust 的 `//` 行注释与 `/* */` 块注释（保留其它内容与行数结构）。
fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let b: Vec<char> = src.chars().collect();
    let mut i = 0usize;
    let mut in_line = false;
    let mut in_block = false;
    while i < b.len() {
        let c = b[i];
        let n = if i + 1 < b.len() { b[i + 1] } else { '\0' };
        if in_line {
            if c == '\n' { in_line = false; out.push(c); }
            i += 1;
            continue;
        }
        if in_block {
            if c == '*' && n == '/' { in_block = false; i += 2; continue; }
            if c == '\n' { out.push(c); }
            i += 1;
            continue;
        }
        if c == '/' && n == '/' { in_line = true; i += 2; continue; }
        if c == '/' && n == '*' { in_block = true; i += 2; continue; }
        out.push(c);
        i += 1;
    }
    out
}

const PLATFORM_METHODS: &[&str] = &[
    "name", "service", "capabilities",
    "node_artifact", "node_candidate_paths", "node_bin_after_install",
    "is_usable_executable", "core_extra_candidates", "is_local_fixed_dir",
    "install_node", "has_privilege_channel",
    "node_exe_name", "npm_exe_name", "core_exe_names",
];
const SERVICE_METHODS: &[&str] = &[
    "kind", "definition_path", "ensure_defined", "start", "stop", "spawn_daemon",
    // 2026-09-13 新增：服务定义**存在性**的平台事实判定。
    //   背景（P3）：--service-plan 原用 definition_path().is_file()，而 Windows 的
    //   「路径」是标识串 schtasks://DSH-Supervisor（没有文件）→ 恒 false →
    //   自检无论计划任务是否存在都报「现存 = 否」。故把判定收进 trait，由各平台实现。
    "is_defined",
];

#[test]
fn u_a_service_impl_has_only_service_methods() {
    let src = read("src/platform/unsupported.rs");
    let got = impl_methods(&src, "impl ServiceControl for Impl {");
    let want: BTreeSet<String> = SERVICE_METHODS.iter().map(|s| s.to_string()).collect();
    let extra: Vec<_> = got.difference(&want).cloned().collect();
    let missing: Vec<_> = want.difference(&got).cloned().collect();
    assert!(
        extra.is_empty() && missing.is_empty(),
        "U-a FAIL ServiceControl impl 方法集不符：多出 {:?}，缺少 {:?}",
        extra, missing
    );
    eprintln!("U-a PASS ServiceControl impl 只含 6 个应有方法");
}

#[test]
fn u_b_platform_impl_has_all_platform_methods() {
    let src = read("src/platform/unsupported.rs");
    let got = impl_methods(&src, "impl Platform for Impl {");
    let want: BTreeSet<String> = PLATFORM_METHODS.iter().map(|s| s.to_string()).collect();
    let missing: Vec<_> = want.difference(&got).cloned().collect();
    assert!(
        missing.is_empty(),
        "U-b FAIL Platform impl 缺少必需方法 {:?} —— 未知平台 target 会编译失败",
        missing
    );
    eprintln!("U-b PASS Platform impl 含全部 14 个方法");
}

#[test]
fn u_c_method_sets_are_disjoint() {
    let src = read("src/platform/unsupported.rs");
    let s = impl_methods(&src, "impl ServiceControl for Impl {");
    let p = impl_methods(&src, "impl Platform for Impl {");
    let both: Vec<_> = s.intersection(&p).cloned().collect();
    assert!(both.is_empty(), "U-c FAIL 方法同时出现在两个 impl 中: {:?}", both);
    eprintln!("U-c PASS 两个 impl 的方法集不相交");
}

#[test]
fn u_d_trait_definitions_match_expectation() {
    // 防「trait 演进后本门禁的常量集过期」——直接从真实 trait 定义读方法集并比对。
    let mod_rs = read("src/platform/mod.rs");
    let svc_rs = read("src/platform/service.rs");
    let plat = trait_methods(&mod_rs, "pub trait Platform");
    let svc = trait_methods(&svc_rs, "pub trait ServiceControl");
    let want_p: BTreeSet<String> = PLATFORM_METHODS.iter().map(|s| s.to_string()).collect();
    let want_s: BTreeSet<String> = SERVICE_METHODS.iter().map(|s| s.to_string()).collect();
    assert!(
        plat == want_p,
        "U-d FAIL Platform trait 的方法集已变化（本门禁常量需同步）：trait={:?}",
        plat
    );
    assert!(
        svc == want_s,
        "U-d FAIL ServiceControl trait 的方法集已变化（本门禁常量需同步）：trait={:?}",
        svc
    );
    eprintln!("U-d PASS trait 定义与门禁常量一致（Platform {} / ServiceControl {}）", plat.len(), svc.len());
}