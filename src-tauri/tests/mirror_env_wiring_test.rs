//! 壳仓两处「写入/闸门不生效」回归（2026-09-13）。
//!
//! ## 缺陷 ①（失效模式 f + i，跨仓契约）：registry.json 的 selected 永远是 null
//!
//! \`mirror.rs::warmup_async\` 算出了 npm 最快源与延迟，却只放进内存 \`ProbeSnapshot\`，
//! **从不写入 \`m.selected_npm\`** —— 全仓没有任何地方把它设成一个被选中的源
//! （只在 load 时从磁盘读、在 mirror_set 时置 None）。
//! 于是 \`export_to_kernel_with\` 里 \`m.selected_npm.as_ref().map(...)\` 恒为 None →
//! 写进 registry.json 的 \`selected\` **永远是 null** →
//! 内核 \`domains/dist/index.js\` 的「优先采用壳投放的 selected……两侧必然同源」分支
//! **永不执行**（内核每次仍自测选源），跨仓同源承诺与 selected.checkedAt 的 TTL 路径全部失效。
//!
//! 附带第二处缺陷（潜伏）：唯一带延迟的调用点 \`node.rs\` 传的是 **Node 源**延迟，
//! 却与 npm 语义的 \`selected\` 配对 —— 「拿 Node 的延迟去描述 npm 的选择」。
//! 只因 origin 恒为空（selected_npm 恒 None）才未显形。
//!
//! ## 缺陷 ②（失效模式 e：闸门恒不可达）：Node 安装全程零进度/零状态反馈
//!
//! 前端 \`env_progress\` 监听器原为 \`if (p.busy) { ... }`，而该条件**永远为假**：
//! 带 busy 的唯一 emit 是 \`commands/mod.rs\` 的 \`crate::log(&s)\`，
//! 而它在**前一行刚把 busy 置回 false**；安装过程的进度事件来自 \`main.rs::push_status\`，
//! 其 payload **根本不含 busy**。
//! 后果：装 Node（30~90MB、慢网数分钟）期间引导页停在静态文案，
//! Rust 侧 0.1/0.2/0.3/0.8 进度与状态**全被丢弃**。
//!
//! ## 锁定不变量
//!   M-a  warmup_async 把选中 npm 源**落盘**（含延迟）
//!   M-b  export 的延迟来自**随选择落盘**的同源延迟，而非调用方传入的别源延迟
//!   M-c  node.rs 不再把 Node 侧延迟传给 npm 语义的 selected 导出
//!   E-a  env_progress 监听器不再以 busy 为闸（只要 status 就展示）
//!   E-b  progress 为数字时驱动进度条
//!   E-c  push_status 的 payload 带 busy（使「安装中」可辨识）
//!   E-d  反向：判据能识别「以 busy 为闸」的旧形态（门禁非空转）

use std::fs;
use std::path::PathBuf;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    fs::read_to_string(manifest_dir().join(rel)).unwrap_or_else(|e| panic!("读取 {} 失败: {}", rel, e))
}

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

#[test]
fn m_a_warmup_persists_selected_npm() {
    let raw = read("src/mirror.rs");
    let code = strip_comments(&raw);
    assert!(
        code.contains("m.selected_npm = Some("),
        "M-a FAIL warmup 未把选中的 npm 源落盘 —— registry.json 的 selected 将永远是 null"
    );
    assert!(
        code.contains("m.selected_npm_latency_ms = Some("),
        "M-a FAIL 未把选中 npm 源的延迟一起落盘（延迟与选择须同源）"
    );
    // 反向：确认落盘发生在 **warmup_async 内**（而非只命中 load() 里的读取赋值）。
    //   ⚠ 不能用 find(整个文件) —— 那会命中 load() 中的 \`m.selected_npm = Some(s2...)\`
    //     （位置在 warmup_async 之前）→ 假红。必须把搜索**限定在 warmup_async 的函数体**内。
    let i_warmup = code.find("pub fn warmup_async").expect("M-a FAIL 未找到 warmup_async");
    let body = &code[i_warmup..(i_warmup + 4000).min(code.len())];
    assert!(
        body.contains("m.selected_npm = Some("),
        "M-a FAIL warmup_async 函数体内未把选中的 npm 源落盘"
    );
    assert!(
        body.contains("save(&m)"),
        "M-a FAIL warmup_async 落盘后未调用 save"
    );
}

#[test]
fn m_b_export_latency_is_same_source() {
    let code = strip_comments(&read("src/mirror.rs"));
    assert!(
        code.contains("selected_npm_latency_ms"),
        "M-b FAIL 导出未使用随选择落盘的同源延迟"
    );
    // 反向：selected 的 latencyMs 不得直接取自调用方参数（旧形态）
    let i = code.find("let selected = m.selected_npm.as_ref().map").expect("M-b FAIL 未找到 selected 构造");
    let block = &code[i..(i + 400).min(code.len())];
    assert!(
        !block.contains("\"latencyMs\": latency_ms"),
        "M-b FAIL selected.latencyMs 仍直接取自调用方参数（可能与 npm 选择不同源）"
    );
}

#[test]
fn m_c_node_export_does_not_pass_node_latency_for_npm() {
    let code = strip_comments(&read("src/node.rs"));
    assert!(
        !code.contains("export_to_kernel_with"),
        "M-c FAIL node.rs 仍把 Node 侧延迟传给 npm 语义的 selected 导出"
    );
    assert!(
        code.contains("export_to_kernel(&m)"),
        "M-c FAIL node.rs 未改为无延迟导出"
    );
}

#[test]
fn e_a_env_progress_listener_not_gated_on_busy() {
    let raw = read("bootstrap/js/80-init.js");
    let code = strip_comments(&raw);
    // 定位 env_progress 监听块
    let i = code.find("listen('env_progress'").expect("E-a FAIL 未找到 env_progress 监听");
    let block = &code[i..(i + 900).min(code.len())];
    assert!(
        !block.contains("if (p.busy)"),
        "E-a FAIL 监听器仍以 busy 为闸（该条件恒为假 → 安装全程零反馈）"
    );
    assert!(
        block.contains("if (p.status)"),
        "E-a FAIL 未改为「只要 status 就展示」"
    );
    assert!(
        block.contains("p.progress"),
        "E-b FAIL 未消费 progress（进度条不动）"
    );
}

#[test]
fn e_c_push_status_payload_carries_busy() {
    let code = strip_comments(&read("src/main.rs"));
    let i = code.find("env_progress").expect("E-c FAIL 未找到 env_progress 发射点");
    let block = &code[i..(i + 300).min(code.len())];
    assert!(
        block.contains("\"busy\": true"),
        "E-c FAIL push_status 的 payload 未带 busy（安装中不可辨识）"
    );
}

#[test]
fn e_d_offender_detector_is_not_vacuous() {
    // 反向：判据必须能识别「以 busy 为闸」的旧形态
    let old_form = "NS.evt.listen('env_progress', function (e) {\n\
         var p = e.payload || {};\n\
         if (p.busy) { NS.setStep(1); NS.status(p.status || 'x'); }\n\
       });";
    let i = old_form.find("listen('env_progress'").unwrap();
    let block = &old_form[i..];
    assert!(
        block.contains("if (p.busy)"),
        "E-d FAIL 判据无法识别旧形态 —— 门禁空转"
    );
}
