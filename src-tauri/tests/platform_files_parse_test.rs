//! 平台文件**可解析性**门禁（2026-09-13）。
//!
//! ## 修复的缺陷
//!
//! \`src/platform/macos.rs\` 第 108 行的 AppleScript 字符串里**嵌套了未转义的双引号**：
//! \`\`\`text
//!   "do shell script "installer -pkg '{}' -target /" with administrator privileges",
//! \`\`\`
//! 这是**纯语法错误**（\`character literal may only contain one codepoint\` /
//! \`expected \`,\`, found \`-\`\`）→ **macOS target 根本无法编译**。
//!
//! 为什么 Linux 上看不见：该文件被 \`platform/mod.rs\` 的 \`#[cfg(target_os = "macos")]\`
//! **整体条件编译**，cfg 为假时文件**连解析都不做** ——
//! 与同文件先前那处 \`SVC_QUICK\` 未导入（E0425）是同一盲区的**两个独立缺陷**。
//!
//! 发现方式（值得记录）：新增的 CI \`platform-check\`（macos-latest 上
//! \`cargo check --all-targets\`）**第一次运行就 failure**；
//! 而本机用「临时把 macos 实现切到 Linux 上编译」的手法复现出了同一错误。
//!
//! ## 本门禁的作用
//!
//! \`rustfmt\` 是**独立文件解析器**，不受 \`#[cfg]\` 影响 ——
//! 于是可以在 **Linux** 上把「cfg 屏蔽文件」全部解析一遍，**在任何 CI runner 之前**
//! 拦住语法错误这一整类缺陷（比等 mac runner 快得多、且不消耗任何额度）。
//!
//! 边界（诚实说明）：本门禁只覆盖**语法**。**名字解析**错误（E0425 等）仍需
//! \`platform-check\` 在真实 mac/win runner 上编译 —— 两者互补，不可互相替代。
//!
//! ## 锁定不变量
//!   PF-a  \`src/platform/*.rs\` 中**每一个**文件都必须能被解析（含 cfg 屏蔽的）
//!   PF-b  反向：判据必须能识别语法错误（否则门禁空转），且不误报合法源码

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn platform_dir() -> PathBuf {
    manifest_dir().join("src").join("platform")
}

/// 用 rustfmt（独立文件解析器，不受 #[cfg] 影响）解析一段源码。
///
/// 返回 \`None\` = rustfmt 不可用（调用方应跳过，而不是失败）；
/// \`Some(Ok(()))\` = 可解析；\`Some(Err(诊断))\` = 语法错误。
fn rustfmt_parse(src: &str) -> Option<Result<(), String>> {
    let mut child = Command::new("rustfmt")
        .args(["--edition", "2021", "--emit", "stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    if let Some(mut si) = child.stdin.take() {
        let _ = si.write_all(src.as_bytes());
    }
    let out = child.wait_with_output().ok()?;
    if out.status.success() {
        Some(Ok(()))
    } else {
        let diag = String::from_utf8_lossy(&out.stderr)
            .lines()
            .take(6)
            .collect::<Vec<_>>()
            .join("\n");
        Some(Err(diag))
    }
}

fn rs_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("读取 {:?} 失败: {}", dir, e))
        .filter_map(|e| {
            let p = e.ok()?.path();
            if p.extension().and_then(|s| s.to_str()) == Some("rs") { Some(p) } else { None }
        })
        .collect();
    v.sort();
    v
}

#[test]
fn pf_a_every_platform_file_parses() {
    let dir = platform_dir();
    let files = rs_files(&dir);
    assert!(!files.is_empty(), "PF-a FAIL 未找到任何平台文件");
    // 至少应覆盖三平台 + 未知平台 + 共享文件（防目录结构变化后门禁空转）
    assert!(
        files.len() >= 5,
        "PF-a FAIL 平台文件数量异常（{}）—— 门禁可能已空转",
        files.len()
    );

    // rustfmt 不可用则跳过（与本仓「node 不可用时跳过语法检查」同一策略）
    match rustfmt_parse("fn main() {}\n") {
        None => {
            eprintln!("PF-a 跳过：rustfmt 不可用");
            return;
        }
        Some(Ok(())) => {}
        Some(Err(e)) => panic!("PF-a FAIL rustfmt 对平凡源码报错，环境异常：{}", e),
    }

    let mut bad: Vec<String> = Vec::new();
    for f in &files {
        let src = fs::read_to_string(f).unwrap_or_else(|e| panic!("读取 {:?} 失败: {}", f, e));
        match rustfmt_parse(&src) {
            Some(Ok(())) => {}
            Some(Err(diag)) => bad.push(format!(
                "{} —— **语法错误**（Linux 上因 #[cfg(target_os)] 不编译，故必须单独解析）:\n{}",
                f.file_name().unwrap_or_default().to_string_lossy(),
                diag
            )),
            None => panic!("PF-a FAIL rustfmt 中途不可用"),
        }
    }
    assert!(bad.is_empty(), "PF-a FAIL 以下平台文件无法解析：\n\n{}", bad.join("\n\n---\n\n"));
    eprintln!("PF-a PASS {} 个平台文件全部可解析（含 cfg 屏蔽的 macos/windows/unsupported）", files.len());
}

#[test]
fn pf_b_detector_is_not_vacuous() {
    // 反向：判据必须能识别语法错误（即本轮真缺陷的形态）
    let bad_src = "fn main() { let s = \"a \"b\" c\"; }\n";
    match rustfmt_parse(bad_src) {
        None => {
            eprintln!("PF-b 跳过：rustfmt 不可用");
        }
        Some(Ok(())) => panic!("PF-b FAIL 判据无法识别语法错误 —— 门禁空转"),
        Some(Err(_)) => {
            // 合法源码不得被误报
            assert!(
                matches!(rustfmt_parse("fn main() {}\n"), Some(Ok(()))),
                "PF-b FAIL 合法源码被误报"
            );
        }
    }
}
