//! 平台文件**共享项导入**门禁（P1，2026-09-13）。
//!
//! ## 修复的缺陷
//!
//! \`platform/macos.rs\` 在 :202 使用了 \`SVC_QUICK\`（launchctl bootout 的有界超时），
//! 但其 \`use super::{...}\` 列表**漏了它**：
//!
//! \`\`\`text
//!   macos.rs:18  use super::{home_dir, Capabilities, Platform, SVC_NORMAL};   // ← 缺 SVC_QUICK
//!   macos.rs:202 crate::bounded::run_lossy(Command::new("sh")..., SVC_QUICK); // ← 使用
//! \`\`\`
//!
//! Rust 子模块**不继承**父模块作用域，故 \`target_os = "macos"\` 构建时是未绑定标识符：
//!
//! \`\`\`text
//!   error[E0425]: cannot find value \`SVC_QUICK\` in this scope
//! \`\`\`
//!
//! （已用 rustc 最小复现确证该作用域规则。）
//!
//! **为何从未暴露**：\`platform/mod.rs\` 用 \`#[cfg(target_os = ...)]\` 条件编译，
//! Linux 上的 \`cargo test/check\` **根本不编译 macos.rs / windows.rs / unsupported.rs** ——
//! 100+ 项测试全绿也掩盖它。**macOS（arm64 + x64）构建直接失败、无法出包**。
//!
//! 对照：linux.rs 与 windows.rs 的 use 列表都含 SVC_QUICK —— 同一纪律只在一个文件上漏了。
//!
//! ## 为什么用静态结构断言
//!
//! 与同目录 \`platform_unsupported_structure_test.rs\` 同理：CI 只有三大平台工具链，
//! 无法编译「非本平台」的 cfg 分支；而缺陷的本质正是「用了父模块的项却没导入」——
//! 结构断言恰好能精确锁定它。
//!
//! ## 锁定不变量
//!   S-a  每个平台文件用到的 **父模块共享项**，要么经 \`use super::...\` 导入，
//!        要么以 \`super::X\` / \`crate::platform::X\` 限定路径访问
//!   S-b  \`SVC_*\` 常量在三平台文件里都已导入（本次缺陷的直接锁）
//!   S-c  反向：判据能识别「使用但未导入」的形态（门禁非空转）

use std::fs;
use std::path::PathBuf;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// 读取待检查文件，并**归一化换行为 LF**。
///
/// ⚠ 2026-09-13：这些门禁大量做**多行源码片段**的文本断言，而 Windows 检出
///   可能是 CRLF（core.autocrlf + 本仓原先无 .gitattributes）→ 内嵌换行的针脚永不匹配：
///     · 正向 find → 退化为 usize::MAX（失败）；
///     · 反向 !contains → 退化为恒真（假绿、门禁空转，比失败更糟）。
///   实测：把工作树整体转成 CRLF 后跑全套，update_guard_test 的 G6-g 立刻失败 ——
///   这正是 Windows leg 在 CI 上红掉的原因（macOS/Linux 是 LF 故全绿）。
///   修法：**在读取处归一化**，使断言在任何平台检查的是同一件事。
fn read(rel: &str) -> String {
    let s = fs::read_to_string(manifest_dir().join(rel))
        .unwrap_or_else(|e| panic!("读取 {} 失败: {}", rel, e));
    if s.contains('\r') { s.replace("\r\n", "\n") } else { s }
}

/// \`platform/mod.rs\` 里对外（子模块经 \`use super\`）共享的项。
/// 取值来自 mod.rs 的 \`pub\` 声明；新增共享项时应同步加入本表（S-d 会校验 SVC_*）。
const SHARED: &[&str] = &[
    "SVC_QUICK",
    "SVC_NORMAL",
    "home_dir",
    "user_name",
    "Capabilities",
    "NodeArtifact",
    "Platform",
    "latest_versioned_node",
];

/// 去掉整行注释后的代码（本仓多次被自己的说明文字骗过）。
fn strip_comments(src: &str) -> String {
    src.lines()
        .filter(|l| {
            let t = l.trim_start();
            !t.starts_with("//")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 该文件是否以「限定路径」访问超模块项（super::X 或 crate::platform::X）。
fn qualified(src: &str, item: &str) -> bool {
    let a = format!("super::{}", item);
    let b = format!("crate::platform::{}", item);
    src.contains(&a) || src.contains(&b)
}

/// 该文件是否在 \`use super::...\` 行里导入了该项。
fn imported(src: &str, item: &str) -> bool {
    src.lines().any(|l| {
        let t = l.trim_start();
        if !t.starts_with("use ") {
            return false;
        }
        if !(t.contains("super::") || t.contains("crate::platform::")) {
            return false;
        }
        // 词边界匹配，避免 SVC_NORMAL 命中 SVC_NORMAL_EXTRA 之类
        t.split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .any(|tok| tok == item)
    })
}

/// 该项是否在本文件内**定义**（如本地 const）—— 定义了就不需要导入。
///
/// ⚠ 判据必须要求「该项是**被声明的名字**」，不能只是「该行包含这个名字」：
///   第一版写成「以 fn/const 开头且 contains(item)」，于是
///   \`fn f() { SVC_QUICK }\`（**使用**该常量）也被当成「本地定义」→ 反向断言失效（门禁空转）。
fn defined_locally(src: &str, item: &str) -> bool {
    src.lines().any(|l| {
        let t = l.trim_start();
        let decl = t
            .strip_prefix("pub const ")
            .or_else(|| t.strip_prefix("const "))
            .or_else(|| t.strip_prefix("pub fn "))
            .or_else(|| t.strip_prefix("fn "))
            .or_else(|| t.strip_prefix("pub static "))
            .or_else(|| t.strip_prefix("static "));
        match decl {
            // 声明名 = 去掉前缀后到第一个分隔符为止的标识符
            Some(rest) => {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                name == item
            }
            None => false,
        }
    })
}

/// 判据：返回「用到但未导入/未限定/未本地定义」的项。
fn offenders(src: &str) -> Vec<&'static str> {
    let code = strip_comments(src);
    let mut out = Vec::new();
    for item in SHARED.iter().copied() {
        // 使用判定：词边界出现
        let used = code
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .any(|tok| tok == item);
        if !used {
            continue;
        }
        if imported(&code, item) || qualified(&code, item) || defined_locally(&code, item) {
            continue;
        }
        out.push(item);
    }
    out
}

#[test]
fn s_a_platform_files_import_shared_super_items() {
    let files = [
        "src/platform/linux.rs",
        "src/platform/macos.rs",
        "src/platform/windows.rs",
        "src/platform/unsupported.rs",
    ];
    for f in files {
        let src = read(f);
        let bad = offenders(&src);
        assert!(
            bad.is_empty(),
            "S-a FAIL {} 使用了父模块共享项却未导入：{:?}\n\
             （子模块不继承父模块作用域 → 该平台 target 编译失败 E0425；\n\
               而 Linux 上的 cargo check 因 #[cfg(target_os)] 不编译它，故长期不可见）",
            f,
            bad
        );
    }
}

#[test]
fn s_b_svc_constants_imported_in_all_platforms() {
    // SVC_* 的具体锁：三个平台文件都必须把用到的 SVC_* 导入
    let files = [
        ("src/platform/linux.rs", "src/platform/mod.rs"),
        ("src/platform/macos.rs", "src/platform/mod.rs"),
        ("src/platform/windows.rs", "src/platform/mod.rs"),
    ];
    // 先从 mod.rs 解析出实际存在的 SVC_* 常量（防硬编码过时）
    let modsrc = read("src/platform/mod.rs");
    let svc: Vec<String> = modsrc
        .lines()
        .filter(|l| l.trim_start().starts_with("pub const SVC_"))
        .filter_map(|l| {
            let rest = l.trim_start().trim_start_matches("pub const ");
            rest.split(|c: char| !(c.is_alphanumeric() || c == '_')).next().map(|s| s.to_string())
        })
        .collect();
    assert!(!svc.is_empty(), "S-b FAIL 未能从 mod.rs 解析出 SVC_* 常量");
    for (f, _) in files {
        let src = read(f);
        let code = strip_comments(&src);
        for item in &svc {
            let used = code
                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .any(|tok| tok == item.as_str());
            if !used {
                continue;
            }
            assert!(
                imported(&code, item) || qualified(&code, item),
                "S-b FAIL {} 使用了 {} 但未导入（macOS 构建会 E0425）",
                f,
                item
            );
        }
    }
    eprintln!("S-b PASS SVC_* 常量在三平台文件均已导入：{:?}", svc);
}

#[test]
fn s_c_offender_detector_is_not_vacuous() {
    // 反向：判据必须能识别「使用但未导入」的形态（=本次修复前的 macos.rs 片段）
    let broken = "use super::{home_dir, Capabilities, Platform, SVC_NORMAL};\n\
         fn f() -> std::time::Duration { SVC_QUICK }";
    let bad = offenders(broken);
    assert!(
        bad.contains(&"SVC_QUICK"),
        "S-c FAIL 判据无法识别「使用但未导入」—— 门禁空转。got={:?}",
        bad
    );
    // 并确认修好后的形态不再被报
    let fixed = "use super::{home_dir, Capabilities, Platform, SVC_NORMAL, SVC_QUICK};\n\
         fn f() -> std::time::Duration { SVC_QUICK }";
    assert!(
        offenders(fixed).is_empty(),
        "S-c FAIL 已导入的形态仍被误报：{:?}",
        offenders(fixed)
    );
}
