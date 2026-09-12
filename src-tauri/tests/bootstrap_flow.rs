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

/// 前端全部源码 = HTML 内联脚本 + 它 src 引用的每个 js/*.js。
///
/// ⚠ 为什么不是只读 bootstrap.html：2026-09-11 起引导脚本已按职责拆成 9 个外部模块
///   （原 802 行单块，一处语法错即全页不执行）。断言若只读 HTML，会**静默看不到**
///   任何前端逻辑 —— 门禁变成空转。此函数保证「测的是真正会执行的代码」。
fn bootstrap_html() -> String {
    let root = manifest_dir().join("bootstrap");
    let html = fs::read_to_string(root.join("bootstrap.html")).expect("read bootstrap.html");
    let mut all = html.clone();
    // 按 HTML 中的出现顺序拼接外部模块，便于「先加载」类的断言仍然成立
    let mut rest = html.as_str();
    while let Some(i) = rest.find("<script") {
        let after = &rest[i..];
        let open_end = match after.find('>') { Some(v) => v, None => break };
        let tag = &after[..open_end];
        if let Some(s) = tag.find("src=") {
            let tail = &tag[s + 4..];
            let tail = tail.trim_start_matches(|c| c == ' ' || c == '\t' || c == '"' || c == '\'');
            let end = tail.find(|c| c == '"' || c == '\'').unwrap_or(tail.len());
            let rel = &tail[..end];
            if let Ok(js) = fs::read_to_string(root.join(rel)) {
                all.push('\n');
                all.push_str(&js);
            }
        }
        let body_start = i + open_end + 1;
        let close_rel = match rest[body_start..].find("</script>") { Some(v) => v, None => break };
        rest = &rest[body_start + close_rel + "</script>".len()..];
    }
    all
}

/// 取某个函数的**函数体**（从 sig 到下一个顶层 function 或文件末尾）。
///
/// ⚠ 为什么不按固定字节长度截取：本仓含大量中文注释，1 汉字 = **3 字节**，
///   故 `&h[i..i+900]` 实际只覆盖约 300 字符 —— 断言会因此误判（曾真实发生）。
/// 把源码里的 `NS.` 前缀去掉，便于断言与「是否命名空间化」解耦。
///
/// 背景：2026-09-11 前端拆分为 9 个外部模块，共享符号统一走 `window.__BOOT_NS`，
/// 于是 `withTimeout(...)` 变成 `NS.withTimeout(...)`。断言若写死带前缀的文本，
/// 就会在「纯重命名」时误报 —— 而重命名并不改变行为。
fn ns_stripped(src: &str) -> String {
    src.replace("NS.", "")
}
fn function_body(src: &str, sig: &str) -> String {
    let start = match src.find(sig) { Some(v) => v, None => return String::new() };
    let rest = &src[start..];
    let next = rest[1..]
        .find("\n  function ")
        .map(|i| i + 1)
        .unwrap_or(rest.len());
    rest[..next].to_string()
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
        "stepNames = [{}];",
        EXPECTED.iter().map(|s| format!("\x27{}\x27", s)).collect::<Vec<_>>().join(", ")
    );
    let double = format!(
        "stepNames = [{}];",
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
        ns_stripped(&html).contains("then(stepShellUpdate)"),
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
    let m = crate_sources();
    assert!(
        m.contains("shell_update_progress"),
        "B5 FAIL Rust 侧未上报下载进度"
    );
    eprintln!("B5 PASS download progress wired");
}

#[test]
fn b6_rust_check_timeout_and_pending_before_install() {
    let m = crate_sources();
    assert!(m.contains("SHELL_CHECK_TIMEOUT"), "B6 FAIL 缺少检查超时常量");
    assert!(m.contains("SHELL_DOWNLOAD_TIMEOUT"), "B6 FAIL 缺少下载超时常量");
    assert!(m.contains("tokio::time::timeout"), "B6 FAIL 未使用 tokio 超时包裹网络调用");
    // Windows 上 install 会结束本进程，故 mark_pending 必须在其之前
    let mp = m.find("update::mark_pending").expect("B6 FAIL 未找到 mark_pending");
    let inst = m.find("u.install(").expect("B6 FAIL 未找到 u.install");
    assert!(mp < inst, "B6 FAIL mark_pending 必须在 install 之前（Windows 上 install 不返回）");
    eprintln!("B6 PASS timeout present, mark_pending before install");
}

/// B8：内核步骤同样受网络支配，必须也有界。
/// 与壳更新同类：registry 查询与 npm install 都可能因网络停滞而永久阻塞。
#[test]
fn b8_core_steps_are_bounded() {
    let html = bootstrap_html();
    assert!(html.contains("CORE_PLAN_BUDGET_MS"), "B8 FAIL 内核版本检查缺前端超时预算");
    assert!(html.contains("CORE_APPLY_BUDGET_MS"), "B8 FAIL 内核安装缺前端超时预算");
    assert!(
        ns_stripped(&html).contains("withTimeout(core.invoke('core_plan')"),
        "B8 FAIL core_plan 未用 withTimeout 包裹"
    );
    assert!(
        ns_stripped(&html).contains("withTimeout(core.invoke('core_apply'"),
        "B8 FAIL core_apply 未用 withTimeout 包裹"
    );
    // Rust 侧：npm install 不得无限阻塞
    let core = fs::read_to_string(manifest_dir().join("src").join("core.rs")).expect("read core.rs");
    assert!(core.contains("NPM_INSTALL_TIMEOUT"), "B8 FAIL npm install 无超时常量");
    assert!(core.contains("run_command_bounded"), "B8 FAIL npm install 未走有界执行");
    // 只检查**代码行**，注释里对旧实现的说明不算违规
    let code_only: String = core
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            !(t.starts_with("//") || t.starts_with("///") || t.starts_with("*"))
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !code_only.contains("cmd.output()"),
        "B8 FAIL 仍存在无限阻塞的 cmd.output()（npm 挂起即永久卡住）"
    );
    eprintln!("B8 PASS core steps bounded (frontend + rust)");
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

/// B9：步骤标签必须与产品语义一致（用户 2026-09-11 明确要求）。
/// 这六个词是**面向用户的概念**，不是实现细节 —— 用户特别指出「桌面更新」是错误表达，
/// 应为「桌面版本」，且末步应为「进入控制面板」。此断言防止文案被改回。
#[test]
fn b9_step_labels_match_required_semantics() {
    let html = bootstrap_html();
    let expected = [
        ("st-env", "检测环境"),
        ("st-node", "运行环境"),
        ("st-shell", "桌面版本"),
        ("st-core", "内核版本"),
        ("st-guard", "守卫就绪"),
        ("st-panel", "进入控制面板"),
    ];
    for (id, label) in expected {
        let needle = format!("id=\"{}\"", id);
        let pos = html.find(&needle).unwrap_or_else(|| panic!("B9 FAIL 找不到步骤 {}", id));
        // 该步骤声明之后紧接着的文本节点就是标签
        let tail = &html[pos..(pos + 200).min(html.len())];
        assert!(
            tail.contains(label),
            "B9 FAIL 步骤 {} 的标签不是「{}」（片段：{}）",
            id, label, tail
        );
    }
    eprintln!("B9 PASS step labels match required semantics");
}

/// B10：托盘必须区分左右键（用户报告「Windows 右键不好用」）。
#[test]
fn b10_tray_click_is_button_aware() {
    let m = main_rs();
    assert!(
        m.contains("show_menu_on_left_click(false)"),
        "B10 FAIL 未关闭「左键弹菜单」（左键应显示窗口，右键才弹菜单）"
    );
    assert!(
        m.contains("MouseButton::Left"),
        "B10 FAIL 托盘点击未区分左键 —— 右键也会 show_main，会把右键菜单顶掉"
    );
    assert!(
        m.contains("MouseButtonState::Up"),
        "B10 FAIL 未限定抬起状态（应在按键抬起时响应）"
    );
    // 不得再出现「匹配任意 Click」的写法
    assert!(
        !m.contains("TrayIconEvent::Click { .. } = event"),
        "B10 FAIL 仍存在不区分按键的托盘 Click 匹配"
    );
    eprintln!("B10 PASS tray click is button-aware");
}

/// B11：Windows 平台配置必须关闭 shadow（用户报告「四周有隐形框框」）。
/// 官方文档：Windows 上 `shadow: true`（默认）会让无边框窗口多出 1px 白色边框。
/// 本机平台配置须与基础配置**除 shadow 外完全一致**，避免两处漂移。
#[test]
fn b11_windows_config_disables_shadow_and_stays_in_sync() {
    let dir = manifest_dir();
    let base: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.join("tauri.conf.json")).expect("base cfg"))
            .expect("parse base cfg");
    let win_path = dir.join("tauri.windows.conf.json");
    assert!(win_path.exists(), "B11 FAIL 缺少 tauri.windows.conf.json（Windows 平台覆盖）");
    let win: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&win_path).expect("win cfg")).expect("parse win cfg");

    let b = &base["app"]["windows"][0];
    let w = &win["app"]["windows"][0];
    assert_eq!(w["shadow"], serde_json::json!(false), "B11 FAIL Windows shadow 必须为 false（否则无边框窗口有 1px 白边）");

    // 除 shadow 外逐字段比对
    let bo = b.as_object().expect("base window object");
    let wo = w.as_object().expect("win window object");
    for (k, v) in bo {
        if k == "shadow" { continue; }
        assert_eq!(wo.get(k), Some(v), "B11 FAIL Windows 覆盖配置字段 {} 与基础配置不一致", k);
    }
    eprintln!("B11 PASS windows shadow disabled, config in sync");
}

/// B12：用户可见文案不得再出现「桌面更新」（应为「桌面版本」）。
#[test]
fn b12_no_stale_desktop_update_wording() {
    let html = bootstrap_html();
    for bad in ["桌面更新"] {
        let idx = 0;
        while let Some(p) = html[idx..].find(bad) {
            let abs = idx + p;
            let line_start = html[..abs].rfind(char::is_whitespace).map(|x| x + 1).unwrap_or(0);
            let line_end = html[abs..].find(char::is_whitespace).map(|x| abs + x).unwrap_or(html.len());
            panic!("B12 FAIL 仍含旧措辞「{}」：{}", bad, html[line_start..line_end].trim());
        }
    }
    eprintln!("B12 PASS no stale desktop-update wording");
}

/// B13：服务定义必须由壳建立（P0 死锁修复）。
/// 首启时壳是唯一在场组件；若壳只 start 不 create，全新机器上守卫永远起不来。
#[test]
fn b13_shell_owns_service_definition() {
    // ⚠ 2026-09-11：服务定义已从 `src/service.rs` 迁入 **platform 适配层**
    //   （每个平台一个文件；`platform/service.rs` 是 trait 契约）。
    //   断言随产权迁移 —— 否则测试会盯着一个已不存在的文件而**误报通过/失败**。
    let dir = manifest_dir().join("src").join("platform");
    assert!(dir.is_dir(), "B13 FAIL 缺少 src/platform/（平台适配层）");
    let mut all = String::new();
    for e in fs::read_dir(&dir).expect("B13 platform 目录").flatten() {
        if e.path().extension().and_then(|x| x.to_str()) == Some("rs") {
            all.push_str(&fs::read_to_string(e.path()).unwrap_or_default());
        }
    }
    // ⚠ 断言须是**语义等价**而非字面量：macOS 的 plist 路径由常量拼接
    //   （`format!("{}.plist", GUARD_LABEL)`），字面量 "com.dsh.supervisor.plist"
    //   在源码里**并不存在**。照旧断言字面量会把正确的实现判为缺失。
    for needle in [
        "dsh-supervisor.service",            // Linux systemd unit
        "com.dsh.supervisor",                // macOS LaunchAgent 标签
        ".plist",                            // macOS plist 扩展
        "DSH-Supervisor",                    // Windows 计划任务
        "fn ensure_defined",                 // 建立入口（trait 方法）
        "fn spawn_daemon",                   // spawn 兜底（trait 方法）
    ] {
        assert!(all.contains(needle), "B13 FAIL platform 层缺少关键项: {}", needle);
    }
    // 模板必须**内嵌**（外部文件在 npm 包内不存在，正是旧实现静默跳过的根因）
    assert!(all.contains("WantedBy=default.target"), "B13 FAIL systemd 模板未内嵌");
    assert!(all.contains("RunAtLoad"), "B13 FAIL LaunchAgent plist 未内嵌");

    let m = main_rs();
    assert!(m.contains("mod platform;"), "B13 FAIL main.rs 未引入 platform 模块");
    assert!(
        m.contains("platform::service().ensure_defined"),
        "B13 FAIL ensure_guard 未建立服务定义"
    );
    // ⚠ 2026-09-11：`ensure_guard` 已迁入 domain/guardctl.rs（分层），
    //   故「是否调用 spawn 兜底」须连同 domain 层一起查 ——
    //   而「main.rs 是否引入 platform」仍只查 main.rs（那是**分层断言**）。
    assert!(
        crate_sources().contains("platform::service().spawn_daemon"),
        "B13 FAIL 缺少 spawn 兜底调用"
    );
    eprintln!("B13 PASS shell owns 3-platform service definition + spawn fallback");
}

/// B14：macOS Node 安装格式必须与安装命令匹配（.pkg）。
/// 旧实现下载 .tar.gz 却交给 `installer -pkg` —— 格式不匹配，必然失败。
#[test]
fn b14_macos_node_installer_format_matches() {
    // 2026-09-11：macOS 安装逻辑已下沉到 platform/macos.rs（门禁 G1）——
    //   断言随产权迁移，否则测试盯着一个已不含该逻辑的文件而误报。
    let n = platform_sources();
    assert!(n.contains("node-v{}.pkg"), "B14 FAIL macOS 未改用官方 .pkg（安装命令要求 .pkg）");
    assert!(
        !n.contains("node-v{}-darwin-{}.tar.gz"),
        "B14 FAIL 仍下载 .tar.gz 却交给 installer -pkg（格式不匹配）"
    );
    assert!(n.contains("installer -pkg"), "B14 FAIL 未找到 installer -pkg 调用");
    eprintln!("B14 PASS macOS uses .pkg consistent with installer -pkg");
}

/// B15：Node 安装包下载超时必须足够大（产物 30-90MB；ureq 的 timeout 覆盖整次调用）。
#[test]
fn b15_download_timeout_is_generous_enough() {
    let n = fs::read_to_string(manifest_dir().join("src").join("node.rs")).expect("read node.rs");
    assert!(n.contains("HTTP_TOTAL_TIMEOUT"), "B15 FAIL 缺少下载超时常量");
    assert!(
        !n.contains("from_secs(60)"),
        "B15 FAIL 仍存在 60 秒总超时（30-90MB 安装包必然超时）"
    );
    assert!(n.contains("from_secs(15 * 60)"), "B15 FAIL 超时未放宽到 15 分钟");
    eprintln!("B15 PASS download timeout generous");
}

/// B16：Node 最低门槛必须被前端消费（后端算了但前端忽略 = 静默放行旧版 Node）。
#[test]
fn b16_min_node_gate_is_enforced_in_frontend() {
    let html = bootstrap_html();
    assert!(html.contains("st.minOk === false"), "B16 FAIL 前端未校验 Node 最低门槛");
    assert!(html.contains("minRequired"), "B16 FAIL 前端未使用后端回传的最低要求");
    let m = crate_sources();
    assert!(m.contains("minRequired"), "B16 FAIL 后端未回传 minRequired");
    eprintln!("B16 PASS min-node gate enforced");
}

// B17 已移除（2026-09-11 双仓隔离收尾）：它断言的是**内核仓**的 `bin/dsh-supervisor`
//（经 `../../` 跨仓读取），壳仓独立后该路径不存在。
// 等价覆盖已迁至内核仓 `test/package-root-test.js`（P1 系列）——
// 断言应与它验证的代码同仓，而不是靠目录布局巧合成立。

/// B18：镜像适配必须由壳自持（装机时无内核，面板不可用）。
#[test]
fn b18_shell_owns_mirror_adaptation() {
    let m = fs::read_to_string(manifest_dir().join("src").join("mirror.rs"))
        .expect("B18 FAIL 缺少 src/mirror.rs（壳自持镜像模块）");
    // 三类下载源预设齐备
    assert!(m.contains("NODE_PRESETS"), "B18 FAIL 缺 Node 镜像预设");
    assert!(m.contains("NPM_PRESETS"), "B18 FAIL 缺 npm 镜像预设");
    assert!(m.contains("SHELL_PRESETS"), "B18 FAIL 缺壳自更新端点预设");
    // 并行探测 + 缓存
    assert!(m.contains("pub fn probe_all"), "B18 FAIL 缺并行探测");
    assert!(m.contains("thread::scope"), "B18 FAIL 探测未并行（串行会被慢源拖死）");
    // 2026-09-11 校准：原断言查 `CACHE_TTL_SECS`，但该常量与其配套的 `cache_fresh()`
    // 是**死代码**（无人调用）——已删除。真正生效的缓存机制是：
    //   · `warmup_async()` 在独立线程预热（`WARMING` 防重复）；
    //   · `cached()` 纯读快照（无 I/O，可高频轮询）；
    //   · 快照的 `at` 时间戳下发给前端（新鲜度**可见**，由用户判断）。
    // 断言应指向**活的**机制，否则门禁会「因为死代码被删而红」。
    // 壳自持配置 + 导出给内核
    assert!(m.contains("mirrors.json"), "B18 FAIL 缺壳自持配置文件");
    assert!(m.contains("export_to_kernel"), "B18 FAIL 未导出偏好给内核（内核会重新盲选）");
    assert!(m.contains("pub fn warmup_async"), "B18 FAIL 缺后台预热（前端不应阻塞等测速）");
    assert!(m.contains("pub fn cached"), "B18 FAIL 缺纯读缓存读取口（轮询会变成网络请求）");
    assert!(!m.contains("CACHE_TTL_SECS"), "B18 FAIL CACHE_TTL_SECS 已确认是死代码，不应回潮");
    assert!(m.contains("manual"), "B18 FAIL 未保护内核 manual 选择不被覆盖");
    eprintln!("B18 PASS shell owns mirror adaptation");
}

/// B19：三条下载链路都必须接入镜像适配（不能只做一处）。
#[test]
fn b19_all_three_download_paths_use_mirrors() {
    let node = fs::read_to_string(manifest_dir().join("src").join("node.rs")).expect("node.rs");
    let core = fs::read_to_string(manifest_dir().join("src").join("core.rs")).expect("core.rs");
    let main = main_rs();
    // ① Node：并行探测 + 跨源取最高版本（镜像会滞后一版）
    assert!(node.contains("crate::mirror::probe_all"), "B19 FAIL Node 未用并行探测");
    assert!(node.contains("best_from_index"), "B19 FAIL Node 未跨源解析");
    assert!(node.contains("preferred"), "B19 FAIL Node 下载未使用选中的最快源");
    // ② 内核 npm：并行 + 壳配置优先
    assert!(core.contains("crate::mirror::probe_all"), "B19 FAIL 内核 npm 未用并行探测");
    assert!(core.contains("crate::mirror::load"), "B19 FAIL 内核 npm 未读壳镜像配置");
    // ③ 壳自更新：运行时端点覆盖
    assert!(main.contains(".endpoints(endpoints)"), "B19 FAIL 壳自更新端点未运行时覆盖");
    assert!(main.contains("crate::mirror::load()"), "B19 FAIL 壳自更新未读镜像配置");
    eprintln!("B19 PASS all three download paths use mirrors");
}

/// B20：引导页必须在网络失败时提供镜像自助入口（否则用户无任何出口）。
#[test]
fn b20_bootstrap_offers_mirror_fallback() {
    let html = bootstrap_html();
    assert!(html.contains("mirrorBox"), "B20 FAIL 引导页缺镜像设置容器");
    assert!(html.contains("btnMirror"), "B20 FAIL 缺镜像入口按钮");
    assert!(html.contains("mirrorNode"), "B20 FAIL 缺 Node 镜像输入");
    assert!(html.contains("mirrorNpm"), "B20 FAIL 缺内核镜像输入");
    assert!(html.contains("mirror_status"), "B20 FAIL 未接探测命令");
    assert!(html.contains("mirror_set"), "B20 FAIL 未接保存命令");
    // **仅失败时**出现（不干扰普通用户）：默认 display:none
    assert!(
        html.contains("id=\"mirrorBox\" style=\"display:none\""),
        "B20 FAIL 镜像设置未默认隐藏（会干扰普通用户）"
    );
    // 失败态自动展开
    assert!(html.contains("showMirror()"), "B20 FAIL 失败态未自动展开镜像设置");
    eprintln!("B20 PASS bootstrap offers mirror fallback");
}

/// B21：不得再出现「串行先到」的注释与实现不一致。
#[test]
fn b21_no_serial_first_hit_comment() {
    let node = fs::read_to_string(manifest_dir().join("src").join("node.rs")).expect("node.rs");
    assert!(
        !node.contains("并发尝试官方/镜像"),
        "B21 FAIL 仍有「并发尝试」的失真注释（旧实现实为串行）"
    );
    assert!(
        !node.contains("fn sources("),
        "B21 FAIL 旧的串行 sources() 仍在"
    );
    eprintln!("B21 PASS no stale serial comment");
}

/// B22：镜像预设必须全部经真实验证（不得收录未验证的源）。
#[test]
fn b22_mirror_presets_are_verified() {
    let m = fs::read_to_string(manifest_dir().join("src").join("mirror.rs")).expect("mirror.rs");
    // Node：10 个（实测下载+SHA256 通过）
    assert!(m.contains("pub const NODE_PRESETS: [&str; 10]"), "B22 FAIL Node 预设数量不符（应为已验证的 10 个）");
    // npm：6 个（实测 tarball 下载通过）
    assert!(m.contains("pub const NPM_PRESETS: [&str; 6]"), "B22 FAIL npm 预设数量不符（应为已验证的 6 个）");
    // 壳自更新：仅 2 个（静态文件直链，实测只有 unpkg/jsdelivr 可用）
    assert!(m.contains("pub const SHELL_PRESETS: [&str; 2]"), "B22 FAIL 壳端点数量不符");
    // 被排除的源不得出现在预设里
    for bad in ["mirrors.ustc.edu.cn/node\"", "mirrors.aliyun.com/npm\"", "mirrors.tuna.tsinghua.edu.cn/npm\""] {
        assert!(!m.contains(bad), "B22 FAIL 收录了实测不可用的镜像: {}", bad);
    }
    // 必须记录验证方法与排除原因（防止后人盲目加源）
    assert!(m.contains("SHA256 校验"), "B22 FAIL 未记录 Node 镜像的验证方法");
    assert!(m.contains("已排除"), "B22 FAIL 未记录被排除的镜像");
    eprintln!("B22 PASS mirror presets are verified");
}

/// B23：**壳侧**预设集合必须自洽（mirror.rs 主路径与 core.rs 兜底路径一致）。
///
/// 注（2026-09-11 双仓隔离收尾）：原断言还跨仓读取**内核** `src/platform/config.js`，
/// 壳仓独立后该路径不存在。内核侧集合由其自身 `test/package-root-test.js`（P2 系列）保证；
/// 本测试只负责**壳仓内部**一致性（两处不一致会导致主路径与兜底路径选出不同源）。
#[test]
fn b23_shell_preset_sets_are_consistent() {
    let core = fs::read_to_string(manifest_dir().join("src").join("core.rs")).expect("core.rs");
    assert!(
        core.contains("const DEFAULT_ORIGINS: [&str; 6]"),
        "B23 FAIL 壳 core.rs 的 DEFAULT_ORIGINS 未同步到 6 个"
    );
    let m = fs::read_to_string(manifest_dir().join("src").join("mirror.rs")).expect("mirror.rs");
    assert!(
        m.contains("pub const NPM_PRESETS: [&str; 6]"),
        "B23 FAIL 壳 mirror.rs 的 NPM_PRESETS 未同步到 6 个"
    );
    for needle in ["npmreg.proxy.ustclug.org", "r.cnpmjs.org"] {
        assert!(core.contains(needle), "B23 FAIL core.rs DEFAULT_ORIGINS 缺 {}", needle);
        assert!(m.contains(needle), "B23 FAIL mirror.rs NPM_PRESETS 缺 {}", needle);
    }
    eprintln!("B23 PASS shell preset sets consistent");
}

/// B24：服务定义自检入口必须存在（P0 修复的可诊断性）。
/// 若服务定义建立失败，用户会卡在「守卫就绪」却无从自查；本入口提供无 GUI 的自检与建立。
#[test]
fn b24_service_plan_cli_exists() {
    let m = main_rs();
    assert!(m.contains("--service-plan"), "B24 FAIL 缺 --service-plan 自检入口");
    assert!(m.contains("--service-apply"), "B24 FAIL 缺 --service-apply 实际建立开关");
    assert!(m.contains("cli_service_plan"), "B24 FAIL 缺 cli_service_plan 实现");
    assert!(m.contains("DSH_GUARD_BIN"), "B24 FAIL 缺守卫路径覆盖（诊断/测试隔离用）");
    assert!(m.contains("definition_path"), "B24 FAIL 未报告服务定义路径");
    eprintln!("B24 PASS service-plan CLI present");
}

/// B25：无 GUI 定位守卫时不得要求 AppHandle（CLI 路径必须可独立工作）。
#[test]
fn b25_candidates_work_without_apphandle() {
    let m = crate_sources();
    assert!(
        m.contains("fn locate_core_candidates(resource_dir: Option<PathBuf>)"),
        "B25 FAIL 候选定位仍要求 AppHandle（CLI 无法复用）"
    );
    let c = fs::read_to_string(manifest_dir().join("src").join("core.rs")).expect("core.rs");
    assert!(c.contains("locate_core_for_cli"), "B25 FAIL 缺 CLI 专用定位函数");
    eprintln!("B25 PASS candidates usable without AppHandle");
}

// ═══════════════════════════════════════════════════════════════════
// 架构级回归（2026-09-11 二次修复）：环境探测不得被无界系统调用卡死
//
// 背景：1.0.3/1.0.4 都卡在「检测环境」，说明首轮修复（加超时）没触及根因。
//   根因是「探测内含无界阻塞系统调用」+「命令 await 该探测」，
//   于是「探测挂起」等价于「命令永不返回」，前端超时只是停止等待、并不解除挂起。
// ═══════════════════════════════════════════════════════════════════

/// B26：必须有独立的有界探测运行时（分离线程 + 有界等待 + 缓存 + 追踪）。
/// 这是唯一能给 CreateProcessW / GetFileAttributesW 加界的手段。
#[test]
fn b26_bounded_probe_runtime_exists() {
    let p = fs::read_to_string(manifest_dir().join("src").join("nodeprobe.rs"))
        .expect("B26 FAIL 缺少 src/nodeprobe.rs（有界探测运行时）");
    assert!(p.contains("recv_timeout"), "B26 FAIL 未用 recv_timeout（无法给阻塞调用加界）");
    assert!(p.contains("spawn_worker"), "B26 FAIL 探测未在分离线程中执行");
    assert!(p.contains("STALE_AFTER"), "B26 FAIL 缺陈旧判定（阻塞永久挂起会永久剥夺探测能力）");
    assert!(p.contains("pub fn status"), "B26 FAIL 缺有界查询入口");
    assert!(p.contains("pub fn invalidate"), "B26 FAIL 缺缓存失效（装完 Node 后必须能重新发现）");
    assert!(p.contains("pub fn current_stuck"), "B26 FAIL 缺「卡在谁」诊断");
    assert!(p.contains("trace"), "B26 FAIL 缺逐候选追踪");
    eprintln!("B26 PASS bounded probe runtime present");
}

/// B27：node_status 必须**立即返回**并支持轮询 —— 不得 await 探测到底。
#[test]
fn b27_node_status_is_pollable_not_blocking() {
    let m = crate_sources();
    // 必须暴露轮询所需字段
    assert!(m.contains("\"probing\""), "B27 FAIL node_status 未回传 probing（前端无法轮询）");
    assert!(m.contains("\"stuck\""), "B27 FAIL node_status 未回传 stuck（无法显示卡在哪）");
    assert!(m.contains("\"trace\""), "B27 FAIL node_status 未回传 trace");
    // 探测预算必须很短（命令本身不得长时间占用）
    assert!(
        m.contains("from_millis(900)"),
        "B27 FAIL node_status 的等待预算过长（应短到可轮询）"
    );
    // 网络侧必须与本地判定分离
    assert!(m.contains("async fn node_latest"), "B27 FAIL 缺独立 node_latest（网络与本地判定必须解耦）");
    eprintln!("B27 PASS node_status pollable and local-only");
}

/// B28：PATH 扫描必须有界，且过滤可能阻塞的非固定盘/UNC。
#[test]
fn b28_path_scan_bounded_and_local_only() {
    let e = fs::read_to_string(manifest_dir().join("src").join("env.rs")).expect("env.rs");
    assert!(e.contains("PATH_SCAN_BUDGET"), "B28 FAIL PATH 扫描无预算");
    assert!(e.contains("pub fn path_dirs_local_only"), "B28 FAIL 缺本地盘过滤");
    assert!(e.contains("GetDriveTypeW"), "B28 FAIL 未做磁盘类型判定（网络盘会阻塞）");
    assert!(e.contains("pub fn recorded_node_path"), "B28 FAIL 未回读 runtime.json（最廉价的探测来源）");
    eprintln!("B28 PASS path scan bounded and local-only");
}

/// B29：引导页必须**轮询**环境状态，且探测超时后给出可操作出口。
#[test]
fn b29_bootstrap_polls_and_offers_escape() {
    let h = bootstrap_html();
    assert!(h.contains("st.probing"), "B29 FAIL 引导页未处理 probing（无法轮询）");
    assert!(h.contains("failEnvTimeout"), "B29 FAIL 缺环境超时的专门处理");
    // 关键可用性：装 Node 不需要已有 Node → 检测失败不得是死胡同
    assert!(h.contains("btnForceNode"), "B29 FAIL 缺「跳过检测直接安装」出口");
    assert!(h.contains("probeMirrorThen"), "B29 FAIL 镜像选择未显式可见化");
    eprintln!("B29 PASS bootstrap polls and offers escape");
}

/// B30：启动路径**不得同步**调用探测（否则窗口创建会被推迟）。
#[test]
fn b30_setup_must_not_block_on_probe() {
    let m = main_rs();
    // 定位 setup 段，确认其中无同步探测调用
    let setup_start = m.find(".setup(|app| {").expect("B30 FAIL 未找到 setup");
    let tail = &m[setup_start..];
    let setup_end = tail.find("\n        })").map(|i| setup_start + i).unwrap_or(m.len());
    let setup = &m[setup_start..setup_end];
    assert!(
        !setup.contains("let have = env::probe_system_node()"),
        "B30 FAIL setup 仍同步调用探测（会推迟窗口创建）"
    );
    assert!(setup.contains("nodeprobe::start()"), "B30 FAIL setup 未改为仅触发探测");
    eprintln!("B30 PASS setup does not block on probe");
}

/// B31：诊断串必须携带环境探测追踪与镜像选择结果。
#[test]
fn b31_diagnostics_include_probe_trace_and_mirror() {
    let h = bootstrap_html();
    assert!(h.contains("env_trace="), "B31 FAIL 诊断缺 env_trace");
    assert!(h.contains("env_stuck="), "B31 FAIL 诊断缺 env_stuck");
    assert!(h.contains("env_candidates="), "B31 FAIL 诊断缺 env_candidates");
    assert!(h.contains("mirror_probes="), "B31 FAIL 诊断缺镜像逐源延迟");
    // 环境超时不得被误判为网络问题（曾据此展开镜像设置，误导用户）
    assert!(
        !h.contains("/网络|镜像|超时|下载|不可达|timeout|network|mirror/i"),
        "B31 FAIL fail() 仍把「超时」当网络问题（环境超时会误导性展开镜像设置）"
    );
    eprintln!("B31 PASS diagnostics include trace and mirror");
}

// ═══════════════════════════════════════════════════════════════════
// 架构门禁（2026-09-11 全壳审计）：杜绝「无界阻塞 + 被 await」再次散布
//
// 背景：环境探测卡死的根因是「无界阻塞调用 + 被命令 await」。
//   审计发现**同一模式散布在 6 处**（服务管理器命令、guard_start、core_status、
//   托盘菜单、安装命令、内核候选定位）。逐个修完后必须有**系统性门禁**，
//   否则新增代码仍会重犯 —— 缺陷之所以能分散潜伏，正是因为缺少这类检查。
// ═══════════════════════════════════════════════════════════════════

/// 读取 src 下全部 Rust 源码（供全量源码扫描式断言使用）。
/// 平台适配层的全部源码拼接（供「逻辑已下沉」的断言使用）。
///
/// 2026-09-11：43 处平台分支收拢到 src/platform/ 后，
/// 原先针对 node.rs / service.rs 的断言必须改为针对平台层 ——
/// 否则测试会盯着不再含该逻辑的文件，产生假绿或假红。
fn platform_sources() -> String {
    let dir = manifest_dir().join("src").join("platform");
    let mut out = String::new();
    if let Ok(rd) = fs::read_dir(&dir) {
        for e in rd.flatten() {
            if e.path().extension().and_then(|x| x.to_str()) == Some("rs") {
                out.push_str(&fs::read_to_string(e.path()).unwrap_or_default());
            }
        }
    }
    out
}

/// 业务层的全部源码（`main.rs` + `domain/`）。
///
/// 2026-09-11 分层后，原先针对 `main.rs` 的断言必须覆盖 domain 层 ——
/// 否则测试会盯着一个已不含该逻辑的文件，产生**假红**（如 B25/B37/B44）。
///
/// ⚠ 但**分层断言本身**仍必须针对 `main.rs`（如 B13 要求 main.rs 引入 platform），
///   故本助手只用于「逻辑存在性」，不用于「位于哪一层」。
fn crate_sources() -> String {
    let mut out = main_rs();
    // 命令已迁入 src/commands/mod.rs（2026-09-11 分层），一并纳入。
    for sub in ["domain", "commands"] {
        let dir = manifest_dir().join("src").join(sub);
        if let Ok(rd) = fs::read_dir(&dir) {
            for e in rd.flatten() {
                if e.path().extension().and_then(|x| x.to_str()) == Some("rs") {
                    out.push_str(&fs::read_to_string(e.path()).unwrap_or_default());
                }
            }
        }
    }
    out
}

fn all_rust_sources() -> Vec<(String, String)> {
    // ⚠ **必须递归**（2026-09-11 修复门禁盲区）：
    //   原实现只读顶层 `src/`，而 `platform/` 是**子目录** ——
    //   于是 B32「禁止无界外部命令」等按本函数遍历的门禁
    //   对新建的平台适配层**完全不可见**（门禁看起来在跑，实际有盲区）。
    //   路径以 `platform/xxx.rs` 形式给出（保留层级信息，便于报错定位）。
    let dir = manifest_dir().join("src");
    let mut out = Vec::new();
    fn walk(dir: &std::path::Path, prefix: &str, out: &mut Vec<(String, String)>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                // 只下潜一层就够了（当前结构：src/platform/）；仍用递归以便将来扩展。
                walk(&p, &format!("{}{}/", prefix, name), out);
                continue;
            }
            if p.extension().and_then(|x| x.to_str()) != Some("rs") {
                continue;
            }
            if let Ok(s) = fs::read_to_string(&p) {
                out.push((format!("{}{}", prefix, name), s));
            }
        }
    }
    walk(&dir, "", &mut out);
    out
}

/// B32：**任何**外部命令都不得用裸 `.output()` / `.status()`（无界阻塞）。
///
/// 这是本次审计最重要的系统性防线：外部命令必须经 `bounded::run` 执行。
/// 否则在服务管理器无响应、网络盘断开、杀软拦截等环境下会无界挂起 ——
/// 这正是「卡在检测环境」「卡在守卫就绪」的共同根因。
#[test]
fn b32_no_bare_blocking_command_calls() {
    let mut offenders: Vec<String> = Vec::new();
    for (name, src) in all_rust_sources() {
        // bounded.rs 是执行器自身实现（它当然要 spawn），豁免。
        if name == "bounded.rs" {
            continue;
        }
        for (i, line) in src.split('\n').enumerate() {
            let t = line.trim();
            if t.starts_with("//") || t.starts_with("*") {
                continue;
            }
            if t.contains(".output()") || t.contains(".status()") {
                offenders.push(format!("{}:{} {}", name, i + 1, t));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "B32 FAIL 存在无界外部命令调用（应改用 crate::bounded::run）：\n{}",
        offenders.join("\n")
    );
    eprintln!("B32 PASS no bare blocking command calls");
}

/// B33：`guard_start` 必须两侧都有超时（服务管理器挂起 → 引导页永久停住）。
#[test]
fn b33_guard_start_has_timeout() {
    let m = crate_sources();
    assert!(m.contains("GUARD_TOTAL_BUDGET"), "B33 FAIL guard_start 缺总预算");
    assert!(
        m.contains("tokio::time::timeout(GUARD_TOTAL_BUDGET"),
        "B33 FAIL guard_start 未用 tokio 超时包裹"
    );
    let h = bootstrap_html();
    assert!(h.contains("GUARD_START_BUDGET_MS"), "B33 FAIL 前端缺守卫启动预算");
    assert!(
        ns_stripped(&h).contains("withTimeout(core.invoke('guard_start')"),
        "B33 FAIL 前端 guard_start 未包超时（裸 invoke）"
    );
    eprintln!("B33 PASS guard_start bounded on both sides");
}

/// B34：壳必须上报守卫启动阶段进度（静默等待与卡死无法区分）。
#[test]
fn b34_guard_progress_is_reported() {
    assert!(crate_sources().contains("guard_progress"), "B34 FAIL Rust 侧未上报 guard_progress");
    let h = bootstrap_html();
    assert!(h.contains("evt.listen('guard_progress'"), "B34 FAIL 前端未监听 guard_progress");
    eprintln!("B34 PASS guard progress reported");
}

/// B35：阻塞工作不得留在主线程（同步命令执行二进制会占住 UI 数十秒）。
#[test]
fn b35_blocking_work_not_on_main_thread() {
    let m = crate_sources();
    assert!(
        m.contains("async fn core_status"),
        "B35 FAIL core_status 仍是同步命令（会占主线程执行二进制）"
    );
    assert!(
        m.contains("locate_core_with_version"),
        "B35 FAIL core_status 未把定位工作放进阻塞线程池 / 未复用版本"
    );
    eprintln!("B35 PASS blocking work off main thread");
}

/// B36：托盘/退出回调不得在 UI 线程做网络 I/O。
#[test]
fn b36_tray_io_off_ui_thread() {
    // 派发函数已迁 domain/localhttp.rs；「托盘回调是否用它」仍看 main.rs。
    let m = crate_sources();
    assert!(crate_sources().contains("spawn_local_post"), "B36 FAIL 缺 off-thread 派发函数");
    assert!(
        m.contains(r#""start" => domain::localhttp::spawn_local_post("#),
        "B36 FAIL 托盘 start 仍在 UI 线程调用 post_local"
    );
    assert!(
        !m.contains(r#""start" => post_local("#),
        "B36 FAIL 托盘 start 仍直接调用 post_local（会冻结界面）"
    );
    eprintln!("B36 PASS tray IO off UI thread");
}

/// B37：内核候选定位必须过滤可能阻塞的非本地盘（与 PATH 探测同一防护）。
#[test]
fn b37_core_candidates_filter_network_paths() {
    assert!(
        // 候选定位已迁 domain/coreloc.rs（分层）；过滤器经 platform 层。
        crate_sources().contains("is_local_fixed_dir(dir)"),
        "B37 FAIL 内核候选定位未过滤非本地盘（网络盘 is_file 会阻塞）"
    );
    eprintln!("B37 PASS core candidates filter network paths");
}

/// B38：时间戳不得依赖外部进程，且三平台行为一致。
#[test]
fn b38_now_iso_is_pure_std() {
    let n = fs::read_to_string(manifest_dir().join("src").join("node.rs")).expect("node.rs");
    assert!(!n.contains(r#"Command::new("date")"#), "B38 FAIL now_iso 仍执行外部 date");
    assert!(n.contains("civil_from_days"), "B38 FAIL 未用纯 std 日期换算");
    eprintln!("B38 PASS now_iso pure std");
}

/// B39：互斥锁不得用裸 unwrap（中毒后所有命令永久 panic）。
#[test]
fn b39_mutex_poisoning_recovered() {
    let m = main_rs();
    assert!(
        !m.contains(".lock().unwrap()"),
        "B39 FAIL 仍有裸 .lock().unwrap()（锁中毒后全部命令 panic）"
    );
    assert!(
        m.contains(".lock().unwrap_or_else(|e| e.into_inner())"),
        "B39 FAIL 未使用中毒恢复式加锁"
    );
    eprintln!("B39 PASS mutex poisoning recovered");
}

/// B40：**命令路径上不得做 I/O** —— 这是我上一轮修复时自己引入的缺陷。
///
/// 背景（真实事故）：为让诊断串显示候选数量，`node_status` 里调了
/// `candidate_summary()`，而它内部会枚举候选 —— 那要做 `read_dir`（版本管理器目录）
/// 并对每个 PATH 条目调 `GetDriveTypeW`。
/// **这正是我声称已经消除的那类无界阻塞 I/O**，等于把刚搬走的石头又搬回来；
/// 而前端轮询当时没有独立心跳，于是 invoke 不返回 = 永久静默、不报错。
///
/// 断言：摘要必须来自缓存（探测线程写入），命令路径只读字符串。
#[test]
fn b40_command_path_must_not_enumerate() {
    let p = fs::read_to_string(manifest_dir().join("src").join("nodeprobe.rs")).expect("nodeprobe.rs");
    // candidate_summary 不得直接调用 candidates()
    let start = p.find("pub fn candidate_summary()").expect("B40 FAIL 缺 candidate_summary");
    let rest = &p[start..];
    let body_end = rest.find("\n}").unwrap_or(rest.len());
    let body = &rest[..body_end];
    assert!(
        !body.contains("candidates()"),
        "B40 FAIL candidate_summary 仍在现算候选（命令路径上做 I/O）"
    );
    assert!(body.contains("summary"), "B40 FAIL candidate_summary 未读缓存");
    // 摘要在探测线程里写入缓存（经 set_summary）
    assert!(p.contains("fn set_summary"), "B40 FAIL 缺 set_summary 写入入口");
    assert!(p.contains("l.summary = s"), "B40 FAIL set_summary 未真正写缓存");
    assert!(p.contains("set_summary(summarize("), "B40 FAIL 枚举完成后未写摘要缓存");
    eprintln!("B40 PASS command path does not enumerate");
}

/// B47：**任何可能阻塞的调用之前都必须先 stage** —— 「规则一」的机械化检查。
///
/// 为什么需要：线上故障正是「枚举阶段落在进度上报之外」导致 summary/stuck/trace 三项全空，
/// 用户看到「卡住且不报错、也没有任何线索」。
/// 把 I/O 搬进线程只解决「命令不阻塞」，**不解决「卡住时看不见线索」** —— 故需机械化断言。
#[test]
fn b47_every_blocking_phase_is_staged() {
    let p = fs::read_to_string(manifest_dir().join("src").join("nodeprobe.rs")).expect("nodeprobe.rs");
    let d_start = p.find("fn detect()").expect("B47 FAIL 缺 detect");
    let d_end = p.find("/// 探测单个候选").expect("B47 FAIL 缺 try_probe");
    let body = &p[d_start..d_end];

    // 三处可能阻塞的调用，各自之前必须有 stage()
    for needle in ["recorded_node_path", "known_locations()", "path_dirs_staged()"] {
        let at = body.find(needle).unwrap_or_else(|| panic!("B47 FAIL 未找到 {}", needle));
        assert!(
            body[..at].rfind("stage(").is_some(),
            "B47 FAIL {} 之前没有 stage() —— 卡住时会没有任何线索",
            needle
        );
    }
    assert!(p.contains("HARD_DEADLINE_MS"), "B47 FAIL 缺硬上限（规则二）");
    assert!(p.contains("环境探测超过"), "B47 FAIL 硬上限未产出可读原因");
    eprintln!("B47 PASS every blocking phase is staged");
}

/// B48：**交错顺序** —— PATH 过滤（唯一需逐盘符系统调用的阶段）必须排在最后。
///
/// 为什么关键：若先枚举全部候选再探测，则「PATH 过滤慢/卡」会导致
/// **连已枚举好的廉价候选都永远试不到** —— 用户本可瞬间命中已知安装落点却完全失败。
/// 交错后，绝大多数用户（Node 在标准位置）根本不会走到 PATH 过滤。
#[test]
fn b48_path_scan_comes_last() {
    let p = fs::read_to_string(manifest_dir().join("src").join("nodeprobe.rs")).expect("nodeprobe.rs");
    let d_start = p.find("fn detect()").expect("B48 FAIL 缺 detect");
    let d_end = p.find("/// 探测单个候选").expect("B48 FAIL 缺 try_probe");
    let body = &p[d_start..d_end];
    let i_recorded = body.find("recorded_node_path").expect("B48 FAIL 缺记录路径阶段");
    let i_known = body.find("known_locations()").expect("B48 FAIL 缺已知落点阶段");
    let i_path = body.find("path_dirs_staged()").expect("B48 FAIL 缺 PATH 阶段");
    assert!(i_recorded < i_known, "B48 FAIL 记录路径应在已知落点之前");
    assert!(i_known < i_path, "B48 FAIL 已知落点应在 PATH 过滤之前（否则会被 PATH 拖累）");
    // 每个阶段都必须在探测后就地返回（交错，而非先收全再试）
    let seg_known = &body[i_known..i_path];
    assert!(
        seg_known.contains("try_probe"),
        "B48 FAIL 已知落点阶段没有就地探测（仍是先收全再试）"
    );
    eprintln!("B48 PASS path scan comes last");
}

/// B41：前端**轮询循环**必须有独立于被调方的心跳。
///
/// 轮询与单次调用不同：若 `invoke` 永不 settle 而轮询只在其 `.then` 里再调度，
/// 整个循环就**静默停摆** —— 不报错、不推进。用户实测的「卡住且不报错」正是如此。
#[test]
fn b41_polling_loops_have_independent_heartbeat() {
    let h = bootstrap_html();
    // 环境探测轮询：每次查询都必须包超时
    assert!(ns_stripped(&h).contains("withTimeout(core.invoke('node_status'), 15000"), "B41 FAIL 环境轮询未包超时");
    assert!(h.contains("查询无响应，重试中"), "B41 FAIL 环境轮询无超时分支（无法自愈）");
    // 守卫就绪轮询
    assert!(ns_stripped(&h).contains("withTimeout(core.invoke('guard_ready'), 10000"), "B41 FAIL 守卫就绪轮询未包超时");
    eprintln!("B41 PASS polling loops have heartbeat");
}

/// B42：macOS 的平台标签必须与 `platform_artifact()` 选定的产物**语义一致**。
///
/// 原实现 arm64 用 `osx-arm64-tar` 判定、却下载 `.pkg` —— 靠两者恰好都存在而侥幸可用。
/// 实测（逐版本核对官方 index.json）：`osx-x64-pkg` 所有 LTS 都存在，
/// 而 `osx-arm64-pkg` 从不存在；通用 pkg 的标签就是 `osx-x64-pkg`。
#[test]
fn b42_macos_tag_matches_pkg_artifact() {
    // 2026-09-11：制品映射已下沉到 platform/macos.rs；标签与文件名现在同源
    //   （同一个 NodeArtifact 结构一次给出），不再有判定用 tar、下载用 pkg 的错位。
    let n = platform_sources();
    assert!(n.contains("node-v{}.pkg"), "B42 FAIL macOS 未选 .pkg");
    assert!(n.contains("osx-x64-pkg"), "B42 FAIL macOS 标签未与 .pkg 对齐");
    assert!(
        !n.contains("osx-arm64-tar"),
        "B42 FAIL macOS 仍在用 tar 标签判定 pkg 产物"
    );
    eprintln!("B42 PASS macOS tag matches artifact");
}

/// B43：Windows 经 `cmd /C` 启动时，引号必须能承受**含空格的路径**。
///
/// `%APPDATA%` 含 Windows 用户名，而用户名可以含空格（如 "John Smith"）。
/// 旧写法 `.args(["/C", path, "daemon"])` 会让 cmd 拆错 → 守卫启动失败且错误难解读。
#[test]
fn b43_windows_cmd_quoting_handles_spaces() {
    // 迁至平台层（2026-09-11）：Windows 的全部平台知识在 platform/windows.rs。
    let s = fs::read_to_string(manifest_dir().join("src").join("platform").join("windows.rs"))
        .expect("platform/windows.rs");
    assert!(s.contains("raw_arg(line)"), "B43 FAIL Windows 未用 raw_arg 精确控制引号");
    assert!(
        !s.contains(".args([\"/C\", &guard.display().to_string(), \"daemon\"])"),
        "B43 FAIL Windows 仍用会拆错的 args 形式"
    );
    eprintln!("B43 PASS windows cmd quoting handles spaces");
}

/// B44：本地 TCP 连接必须有**连接**超时，且守卫探针不得占用主线程。
///
/// `TcpStream::connect` **没有超时**：端口被防火墙 DROP（而非 REJECT）时会等到 OS
/// SYN 重试耗尽（Windows 默认 20+ 秒）。而 `guard_ready` 被引导页每 500ms 轮询、最多 40 次。
/// 原则：**不能依赖「回环地址正常时很快」来省略上限。**
#[test]
fn b44_local_connect_bounded_and_async() {
    // 连接助手已迁 domain/localhttp.rs（2026-09-11）。
    let m = crate_sources();
    assert!(m.contains("connect_local"), "B44 FAIL 缺统一的带超时连接助手");
    assert!(m.contains("connect_timeout"), "B44 FAIL 未用 connect_timeout");
    assert!(
        !m.contains("TcpStream::connect(addr)"),
        "B44 FAIL 仍有裸 TcpStream::connect（无连接超时）"
    );
    assert!(
        m.contains("async fn guard_ready"),
        "B44 FAIL guard_ready 仍是同步命令（主线程被轮询探针占住）"
    );
    eprintln!("B44 PASS local connect bounded and guard_ready async");
}

/// B45：**判定标签必须按架构给出**（与产物语义一致），不得硬编码 x64。
///
/// 与 B42（macOS）同类：arch 相关的产物，其判定标签也必须 arch 相关。
/// Linux arm64 上若用 `linux-x64` 判定、却下载 `linux-arm64` 文件，
/// 就是「靠恰好在同一个 files[] 里」而侥幸通过。
#[test]
fn b45_platform_tag_is_arch_aware() {
    let n = fs::read_to_string(manifest_dir().join("src").join("node.rs")).expect("node.rs");
    assert!(n.contains("linux-arm64"), "B45 FAIL Linux 标签未按架构区分");
    // 不得对 linux 直接 return 硬编码字面量
    assert!(
        !n.contains("#[cfg(target_os = \"linux\")]\n    { return \"linux-x64\"; }"),
        "B45 FAIL Linux 标签仍硬编码 x64"
    );
    // Windows arm64 限制必须被如实记录（诚实性断言）
    assert!(n.contains("win-arm64-msi"), "B45 FAIL 未记录 Windows arm64 的 msi 缺失限制");
    eprintln!("B45 PASS platform tag arch-aware");
}

/// B46：Windows 路径不得硬编码（系统盘符/语言/Program Files(x86) 都会变化）。
#[test]
fn b46_windows_paths_from_env_not_hardcoded() {
    let e = fs::read_to_string(manifest_dir().join("src").join("env.rs")).expect("env.rs");
    assert!(
        !e.contains(r#"PathBuf::from(r"C:\Program Files\nodejs\node.exe")"#),
        "B46 FAIL env.rs 仍硬编码 C:\\Program Files"
    );
    assert!(e.contains("ProgramFiles(x86)"), "B46 FAIL 未覆盖 Program Files (x86)");
    eprintln!("B46 PASS windows paths from env");
}

// ═══════════════════════════════════════════════════════════════════
// 数据流门禁（2026-09-11 用户质疑驱动）
//
// 用户指出：「镜像源都看不到…出现了很多东西丢失的状态」。
// 查证后确认**不是能力丢失，是可见性丢失** —— 外壳做了工作，用户什么都看不到：
//   · afterEnv 只在「未装/过低 Node」时才测镜像 → Node 达标的**主力用户**永远看不到；
//   · core_plan 早已回传 registry（命中的镜像），但前端**从未使用** —— 数据链路断了。
// 经全字段审计，共 **9 个字段**后端产出而前端未用。
//
// 故加此门禁：**后端回传的关键字段必须在前端被消费**，否则信息等于不存在。
// ═══════════════════════════════════════════════════════════════════

/// B49：镜像信息必须「全程可见」——不得只在需要下载 Node 时才产生。
#[test]
fn b49_mirror_visible_regardless_of_node_state() {
    let h = bootstrap_html();
    // 必须与引导并行预热（而非在下载分支里才测速）
    assert!(h.contains("startMirrorWarmup"), "B49 FAIL 缺镜像预热入口");
    assert!(h.contains("mirror_warmup"), "B49 FAIL 未调用 mirror_warmup");
    assert!(h.contains("mirror_cached"), "B49 FAIL 未读取镜像缓存");
    // 预热必须在 boot 中启动（与步骤无关）。
    // ⚠ 用函数边界取体，不要用固定字节长度（中文注释 3 字节/字，会误判）。
    let boot_body = function_body(&h, "function boot()");
    assert!(boot_body.contains("startMirrorWarmup()"), "B49 FAIL boot 未启动镜像预热");
    // 诊断串必须始终带镜像（含「预热中」这种明确状态，而非 none）
    assert!(h.contains("mirror_npm_best="), "B49 FAIL 诊断缺 mirror_npm_best");
    assert!(h.contains("warming（预热中）"), "B49 FAIL 诊断未区分「预热中」与「不可达」");
    eprintln!("B49 PASS mirror visible regardless of node state");
}

/// B50：`core_plan` 已回传的 `registry`（命中镜像）必须在前端被展示。
#[test]
fn b50_registry_field_is_consumed() {
    let c = fs::read_to_string(manifest_dir().join("src").join("core.rs")).expect("core.rs");
    assert!(c.contains("\"registry\""), "B50 FAIL core.rs 未回传 registry");
    let h = bootstrap_html();
    assert!(h.contains("p.registry"), "B50 FAIL 前端未消费 core_plan 的 registry（数据链路断裂）");
    eprintln!("B50 PASS registry field consumed by frontend");
}

/// B51：内核安装步骤必须显示当前镜像（该步依赖镜像却曾无任何可见性）。
#[test]
fn b51_kernel_steps_show_mirror() {
    let h = bootstrap_html();
    assert!(h.contains("function mirrorText"), "B51 FAIL 缺统一的镜像文案函数");
    let plan = h.find("function stepCorePlan()").expect("B51 FAIL 缺 stepCorePlan");
    let plan_body = &h[plan..(plan + 2600).min(h.len())];
    assert!(plan_body.contains("mirrorText()") || plan_body.contains("p.registry"), "B51 FAIL 内核步骤未展示镜像");
    eprintln!("B51 PASS kernel steps show mirror");
}

/// B52：npm 镜像探测必须用**真实包名**，不能用根路径。
///
/// 根路径在多数 registry 上返回 404 → **健康的源被判「不可达」**。
/// 实测：腾讯云 npm 连测 3 次均 HTTP 200、能正确返回我们的包，
/// 却因根路径 404 而在测速中显示不可达，进而被排除在选择之外 ——
/// **探测方法错误让壳无谓地少一个可用镜像**。
#[test]
fn b52_npm_probe_uses_real_package() {
    let m = fs::read_to_string(manifest_dir().join("src").join("mirror.rs")).expect("mirror.rs");
    assert!(m.contains("fn npm_probe_path"), "B52 FAIL 缺 npm 探测路径函数");
    assert!(m.contains("package_name()"), "B52 FAIL npm 探测未用真实包名");
    // probe_all 必须把空 path 转成真实包名
    assert!(m.contains("if path.is_empty()"), "B52 FAIL probe_all 未处理空 path");
    eprintln!("B52 PASS npm probe uses real package");
}

// ═══════════════════════════════════════════════════════════════════
// B53：**引导页 JS 必须语法正确** —— 本条来自一次真实事故（我造成的）。
//
// 事故：在 diagText() 里加字段时漏了一个逗号，导致整个 `<script>` 块语法错误。
// 后果：**所有 JS 都不执行** → boot() 从不运行 → 页面永远停在 HTML 静态文案
//   「正在检测系统环境…」→ 不报错、不推进、诊断三项全 none。
// 排查代价极高：现象看起来像「探测卡住」，于是反复在 Rust 侧找根因（三轮都找错方向），
//   而真正的问题是**前端一行 JS 没执行**。
//
// 更严重的是：我在提交前**确实跑了** `node --check`，但它失败了，
//   而我没检查命令输出（`&&` 短路导致成功提示未打印）就继续往下走 —— 于是带着错误发布。
//
// 故加此门禁：**把 JS 语法检查变成自动化测试**，不依赖人的注意力。
// ═══════════════════════════════════════════════════════════════════

/// 提取 HTML 中全部 `<script>` 块的内联内容（跳过带 src 的外部引用）。
fn inline_scripts(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find("<script") {
        let after = &rest[i..];
        let open_end = match after.find('>') { Some(v) => v, None => break };
        let tag = &after[..open_end];
        let body_start = i + open_end + 1;
        let close_rel = match rest[body_start..].find("</script>") { Some(v) => v, None => break };
        if !tag.contains("src=") {
            out.push(rest[body_start..body_start + close_rel].to_string());
        }
        rest = &rest[body_start + close_rel + "</script>".len()..];
    }
    out
}

/// 用 node --check 验证 JS 语法；node 不可用时明确 SKIP（不静默通过）。
fn check_js_syntax(label: &str, js: &str) -> Result<(), String> {
    let node = if cfg!(windows) { "node.exe" } else { "node" };
    let dir = std::env::temp_dir().join(format!("dsh-js-check-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    let file = dir.join(format!("{}.js", label.replace(|c: char| !c.is_alphanumeric(), "_")));
    fs::write(&file, js).map_err(|e| format!("写入临时文件失败: {}", e))?;
    let out = std::process::Command::new(node)
        .arg("--check")
        .arg(&file)
        .output()
        .map_err(|e| format!("无法运行 node（{}）: {}", node, e))?;
    let _ = fs::remove_file(&file);
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).to_string())
    }
}

#[test]
fn b53_frontend_js_must_be_syntactically_valid() {
    let root = manifest_dir().join("bootstrap");
    let mut checked = 0;
    // ① 外部模块：js/ 下每个 .js 都必须独立语法正确
    let js_dir = root.join("js");
    if js_dir.is_dir() {
        let mut mods: Vec<_> = fs::read_dir(&js_dir)
            .expect("read js dir")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "js").unwrap_or(false))
            .collect();
        mods.sort();
        assert!(!mods.is_empty(), "B53 FAIL js/ 目录存在但没有任何 .js 模块");
        let boot = fs::read_to_string(root.join("bootstrap.html")).expect("read bootstrap.html");
        for p in &mods {
            let label = p.file_name().unwrap().to_string_lossy().to_string();
            let js = fs::read_to_string(p).unwrap_or_else(|e| panic!("B53 FAIL 读取 {} 失败: {}", label, e));
            match check_js_syntax(&format!("mod-{}", label), &js) {
                Ok(()) => checked += 1,
                Err(e) => {
                    if e.contains("无法运行 node") {
                        eprintln!("B53 SKIP（本机无 node，无法校验 JS 语法）: {}", e.trim());
                        return;
                    }
                    panic!("B53 FAIL 模块 {} 语法错误:\n{}", label, e);
                }
            }
            // ② 反向：每个模块都必须被 HTML 引用 —— 否则是「写了不加载」的死文件
            assert!(boot.contains(&format!("js/{}", label)),
                "B53 FAIL 模块 {} 存在但 bootstrap.html 未引用（死文件）", label);
        }
        eprintln!("B53 外部模块 {} 个，全部被引用且语法正确", mods.len());
    }
    for name in ["bootstrap.html", "shell.html"] {
        let html = fs::read_to_string(root.join(name)).unwrap_or_else(|e| panic!("B53 FAIL 读取 {} 失败: {}", name, e));
        let scripts = inline_scripts(&html);
        if scripts.is_empty() && name == "bootstrap.html" {
            // 已拆为外部模块（上面已逐文件校验，更强）——但不允许「既无内联也无外部引用」
            assert!(html.contains("js/00-runtime.js"), "B53 FAIL bootstrap.html 既无内联脚本也无外部模块引用");
            continue;
        }
        assert!(!scripts.is_empty(), "B53 FAIL {} 中未找到内联 <script>", name);
        for (i, js) in scripts.iter().enumerate() {
            match check_js_syntax(&format!("{}-{}", name, i), js) {
                Ok(()) => checked += 1,
                Err(e) => {
                    // node 缺失时明确 SKIP（而非静默通过）——否则门禁形同虚设。
                    if e.contains("无法运行 node") {
                        eprintln!("B53 SKIP（本机无 node，无法校验 JS 语法）: {}", e.trim());
                        return;
                    }
                    panic!("B53 FAIL {} 第 {} 个 <script> 语法错误:\n{}", name, i + 1, e);
                }
            }
        }
    }
    eprintln!("B53 PASS frontend JS valid（校验 {} 个 script 块）", checked);
}

// ═══════════════════════════════════════════════════════════════════
// B54：**需要 IPC 的页面必须是主帧**（2026-09-11 架构级修复）
//
// 真实根因（用户 Windows 真机：所有 invoke 都不返回）：
//   Tauri 把 IPC 初始化脚本标记为 for_main_frame_only ——
//   （tauri/src/manager/webview.rs: main_frame_script() 内 for_main_frame_only: true）
//   其中包括 window.__TAURI_INTERNALS__（invoke / ipc 的实现）。
//   而 window.__TAURI__.core.invoke 内部是 window.__TAURI_INTERNALS__.invoke(...)，
//   故在 **iframe** 中 invoke 立即抛错、永无结果。
//
// 症状：shell_identity / node_status / mirror_cached 全部挂起，
//   诊断串每一项都是 none —— 「卡在检测环境、不报错、无任何线索」。
//
// 修复：引导页**移出 iframe**（窗口直接加载 bootstrap.html），
//   引导完成后再导航到 shell.html（壳框架；其 iframe 只放守卫托管的面板，不需 IPC）。
// ═══════════════════════════════════════════════════════════════════

#[test]
fn b54_bootstrap_must_be_main_frame() {
    // 1) 窗口 URL 必须是引导页（主帧）
    let conf: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(manifest_dir().join("tauri.conf.json")).expect("tauri.conf.json"),
    )
    .expect("B54 FAIL tauri.conf.json 解析失败");
    let url = conf["app"]["windows"][0]["url"].as_str().unwrap_or("");
    assert_eq!(url, "bootstrap.html", "B54 FAIL 窗口 URL 必须是 bootstrap.html（引导页需主帧 IPC）");

    // 2) shell.html 不得再把引导页放进 iframe
    let shell = fs::read_to_string(manifest_dir().join("bootstrap").join("shell.html")).expect("shell.html");
    assert!(
        !shell.contains("src=\"bootstrap.html\""),
        "B54 FAIL shell.html 仍把 bootstrap.html 放进 iframe（IPC 将不可用）"
    );

    // 3) 引导页不得依赖 iframe 的 IPC 回退路径（那条路径在本平台不可用）
    let boot = bootstrap_html();
    assert!(
        !boot.contains("window.parent.__TAURI__"),
        "B54 FAIL 引导页仍依赖 iframe 的 IPC 回退（该路径在 Tauri 中不可用）"
    );

    // 4) 壳框架必须主动索取面板 URL（引导页导航过来后，事件可能已错过）
    assert!(shell.contains("shell_panel_url"), "B54 FAIL shell.html 未主动索取面板 URL");
    eprintln!("B54 PASS bootstrap is main frame");
}

// B55：IPC 不可用时必须**明确报错**，不得静默返回。
//
// 旧实现 `if (!core) return;` 会让页面停在静态文案「正在检测系统环境…」，
// 既不报错也不推进 —— 这正是用户看到的现象之一，使排障无从下手。
#[test]
fn b55_missing_ipc_must_fail_loudly() {
    let boot = bootstrap_html();
    // ⚠ 只看 boot() 函数体：别处（如 wctl 的按钮守卫）出现 `if (!core) return;` 是合法的，
    //   不应误判（首版断言即因此误报）。
    let body = function_body(&boot, "function boot()");
    // ⚠ 必须先剔除注释行：boot() 的注释里**引用了**旧写法（`if (!core) return;`）作说明，
    //   不过滤会把说明文字误判为实际代码（首版断言即因此误报）。
    let code: String = body
        .split('\n')
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !code.contains("if (!core) return;"),
        "B55 FAIL boot() 在 core 缺失时仍静默返回（应明确报错）"
    );
    assert!(
        code.contains("Tauri IPC 不可用"),
        "B55 FAIL boot() 缺 IPC 缺失时的明确错误信息"
    );
    eprintln!("B55 PASS missing IPC fails loudly");
}

// ═══════════════════════════════════════════════════════════════════════════
// G1：平台分支只允许出现在 `platform/` 内（2026-09-11）
//
// == 背景（用户指正：「壳是乱的，没有架构」）==
//
// 改造前实测：平台分支 **43 处散落在 8 个文件**
//   node.rs 11 / main.rs 10 / service.rs 9 / env.rs 6 / bounded.rs 2 /
//   nodeprobe.rs 2 / update.rs 2 / core.rs 1
//
// 后果：
//   · 加一个平台要翻 8 个文件（且容易漏掉某处）；
//   · 查一个平台 bug 要先猜它在哪一层；
//   · per-OS 知识互相耦合（main.rs 里同时有服务启停的 4 份 #[cfg]）。
//
// 对照：内核（JS）同口径 67 处平台判断，其中 **60 处在 platform/os/（90%）** ——
// 即内核有这个层、而壳没有。本门禁保证壳保持这个形态。
//
// == 断言 ==
//   G1-a 平台分支只出现在 platform/ 内（白名单：已声明并说明理由的例外）
//   G1-b platform/ 必须存在，且每个平台一个文件
//   G1-c `main.rs` 不得再定义平台相关的服务控制函数
//
// == 白名单政策 ==
//
// 例外必须**在下方显式登记并给出理由**，不允许悄悄加 `#[allow]` 绕过。
// 目前唯一例外是 `bounded.rs`（infra 层）：它的 `CREATE_NO_WINDOW` 是
// 「进程创建」这一**原语**的平台差异，属于 infra 而非业务平台知识 ——
// 且平台层自身依赖它，若下沉会形成循环依赖。
// ═══════════════════════════════════════════════════════════════════════════
#[test]
fn g1_platform_branches_only_in_platform_layer() {
    // 例外：文件相对路径 → 理由
    let allowed: &[(&str, &str)] = &[(
        "bounded.rs",
        "infra 原语：CREATE_NO_WINDOW 是进程创建的平台差异；平台层依赖它，下沉会循环",
    )];

    let mut offenders: Vec<String> = Vec::new();
    for (name, src) in all_rust_sources() {
        // platform/ 目录内是**合法**的平台分支所在地
        if name.starts_with("platform/") {
            continue;
        }
        if allowed.iter().any(|(f, _)| *f == name) {
            continue;
        }
        for (i, line) in src.lines().enumerate() {
            let t = line.trim();
            // 只看**属性位置**的 cfg（注释里提到 cfg 不算）
            if !t.starts_with("#[cfg(") {
                continue;
            }
            if t.contains("cfg(test)") {
                continue; // 测试门控不是平台分支
            }
            if t.contains("target_os") || t.contains("windows") || t.contains("unix") {
                offenders.push(format!("{}:{} {}", name, i + 1, t));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "G1 FAIL 平台分支出现在 platform/ 之外（应迁入 platform/）：\n{}",
        offenders.join("\n")
    );

    // G1-b：platform 层必须存在且结构完整
    let pdir = manifest_dir().join("src").join("platform");
    assert!(pdir.is_dir(), "G1 FAIL 缺少 src/platform/");
    for f in [
        "mod.rs",
        "service.rs",
        "linux.rs",
        "macos.rs",
        "windows.rs",
        "unsupported.rs",
    ] {
        assert!(
            pdir.join(f).is_file(),
            "G1 FAIL platform/ 缺少 {}（每平台一文件：加平台=加文件，不改既有代码）",
            f
        );
    }

    // G1-c：main.rs 不得再定义跨平台的服务控制函数（原分层违规）
    let m = main_rs();
    for f in ["fn start_guard_service", "fn stop_guard_service"] {
        assert!(
            !m.contains(f),
            "G1 FAIL main.rs 仍定义 {}（应与服务定义同属 platform::ServiceControl）",
            f
        );
    }
    eprintln!("G1 PASS platform branches confined to platform/");
}

// ═══════════════════════════════════════════════════════════════════════════
// G1': `all_rust_sources` 必须**递归**（否则门禁对 platform/ 有盲区）
//
// 这是修门禁时发现的真实缺陷：原实现只读顶层 `src/`，
// 于是 B32 / G1 等按它遍历的门禁**看不到子目录** ——
// 门禁看起来在跑，实际有盲区。
// ═══════════════════════════════════════════════════════════════════════════
#[test]
fn g1_all_rust_sources_is_recursive() {
    let srcs = all_rust_sources();
    assert!(
        srcs.iter().any(|(n, _)| n.starts_with("platform/")),
        "G1' FAIL all_rust_sources 未递归到 platform/（门禁盲区！）"
    );
    assert!(
        srcs.iter().any(|(n, _)| n == "platform/linux.rs"),
        "G1' FAIL 未枚举到 platform/linux.rs"
    );
    assert!(
        srcs.iter().any(|(n, _)| n == "main.rs"),
        "G1' FAIL 未枚举到顶层 main.rs"
    );
    eprintln!("G1' PASS all_rust_sources recursive ({} files)", srcs.len());
}

// ═══════════════════════════════════════════════════════════════════════════
// G2：命令层只做校验与委托（不得直接执行外部命令 / 不得有平台分支）
//
// 背景：`main.rs` 改造前 1420 行，混装 IPC 命令 + 平台逻辑 + 业务逻辑。
// 拆出 `commands/` 后，必须**防回潮** —— 否则第一万次「就加一行」会把它变回去。
//
// G1 已单独禁止 `commands/` 出现平台分支；此处补「不得直接 spawn」。
// ═══════════════════════════════════════════════════════════════════════════
#[test]
fn g2_commands_layer_only_delegates() {
    let dir = manifest_dir().join("src").join("commands");
    let mut offenders: Vec<String> = Vec::new();
    if let Ok(rd) = fs::read_dir(&dir) {
        for e in rd.flatten() {
            if e.path().extension().and_then(|x| x.to_str()) != Some("rs") {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            let src = fs::read_to_string(e.path()).unwrap_or_default();
            for (i, line) in src.lines().enumerate() {
                let t = line.trim();
                if t.starts_with("//") {
                    continue;
                }
                // 直接构造外部命令 = 业务/平台逻辑泄漏
                if t.contains("std::process::Command::new") || t.contains("process::Command::new") {
                    offenders.push(format!("{}:{} {}", name, i + 1, t));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "G2 FAIL 命令层出现外部命令调用（应经 domain/platform）：\n{}",
        offenders.join("\n")
    );
    eprintln!("G2 PASS commands layer only delegates");
}

// ═══════════════════════════════════════════════════════════════════════════
// G3：`main.rs` 必须保持「只做组装」（防单体回潮）
//
// 改造前 1420 行；拆出 domain/ + commands/ 后 469 行。
// 上限设为 **550** 行：给出合理余量（新增一个命令约 10 行），
// 但一旦有人把业务写回 main.rs 就会触发。
//
// ⚠ 为什么不设 150（目标值）：目标值需要把 `setup()` 内的窗口/托盘装配也拆出，
//   那属于后续工作；**门禁应当锁定当前已达成的水平并能防止退化**，
//   而不是设一个当下即红的数字（红了就会被 `#[ignore]` 掉，门禁形同虚设）。
// ═══════════════════════════════════════════════════════════════════════════
#[test]
fn g3_main_rs_stays_assembly_only() {
    let src = main_rs();
    let n = src.lines().count();
    assert!(
        n <= 550,
        "G3 FAIL main.rs 已 {} 行（上限 550）—— 业务应下沉到 domain/，命令应放入 commands/",
        n
    );
    // 强断言：main.rs 不得再定义 `#[tauri::command]`（命令必须全在 commands/）
    let cmd_count = src.matches("#[tauri::command]").count();
    assert_eq!(cmd_count, 0, "G3 FAIL main.rs 仍在定义 IPC 命令（{} 个）", cmd_count);
    // 强断言：命令层必须真的存在
    let cdir = manifest_dir().join("src").join("commands");
    assert!(cdir.join("mod.rs").is_file(), "G3 FAIL 缺少 src/commands/mod.rs");
    eprintln!("G3 PASS main.rs is assembly-only ({} lines)", n);
}

// ═══════════════════════════════════════════════════════════════════════════
// G5：前端脚本必须**逐个**语法正确（不变量 F1）
//
// == 背景（一次真实的静默死亡）==
//
// `bootstrap.html` 的单个 `<script>` 缺一个逗号 → **整块语法错误** →
// 该块内**所有** JS 都不执行 → 页面停在静态文案「正在检测系统环境…」，
// 不报错、不推进、诊断串每一项都是 none。
//
// 教训：**「整页一个语法检查」不够** —— 必须逐块检查，
// 才能定位到「哪一个块坏了」，也才能在拆分为多文件后继续有效。
//
// == 断言 ==
//   G5-a  每个内联 <script> 块独立通过 `node --check`
//   G5-b  F2：必须注册全局 onerror / unhandledrejection
//   G5-c  F3：IPC 不可用时必须**明确报错**（不得静默停住）
//   G5-d  需要 IPC 的页面存在可见的致命错误容器（错误能被人看见）
// ═══════════════════════════════════════════════════════════════════════════
#[test]
fn g5_frontend_scripts_syntax_and_error_handling() {
    use std::process::Command;

    let pages = ["bootstrap.html", "shell.html"];
    let mut checked = 0usize;
    let mut all_inline = String::new();

    for page in pages {
        let path = manifest_dir().join("bootstrap").join(page);
        let html = fs::read_to_string(&path).unwrap_or_else(|_| panic!("G5 FAIL 缺少 {}", page));

        // 提取内联 script 块（跳过带 src 的）
        let mut blocks: Vec<String> = Vec::new();
        let mut rest = html.as_str();
        while let Some(i) = rest.find("<script") {
            let after = &rest[i..];
            let Some(gt) = after.find('>') else { break };
            let attrs = &after[..gt];
            let body_start = i + gt + 1;
            let Some(close) = rest[body_start..].find("</script>") else { break };
            let body = &rest[body_start..body_start + close];
            if !attrs.contains("src") {
                blocks.push(body.to_string());
            } else if let Some(sq) = attrs.find("src=") {
                let tail = &attrs[sq + 4..];
                let tail = tail.trim_start_matches(|c: char| c == ' ' || c == '\t' || c == '"' || c == '\'');
                let end = tail.find(|c: char| c == '"' || c == '\'').unwrap_or(tail.len());
                let rel = &tail[..end];
                // 拆分后的外部模块同样是「会执行的 JS」，必须一并检查（否则 G5 空转）
                if let Ok(js) = fs::read_to_string(manifest_dir().join("bootstrap").join(rel)) {
                    blocks.push(js);
                }
            }
            rest = &rest[body_start + close + "</script>".len()..];
        }

        for (n, b) in blocks.iter().enumerate() {
            all_inline.push_str(b);
            // 写临时文件做 node --check（无 node 时跳过，不让门禁在无 Node 环境假红）
            let tmp = std::env::temp_dir().join(format!("g5-{}-{}.js", page, n));
            if fs::write(&tmp, b).is_err() {
                continue;
            }
            match Command::new("node").arg("--check").arg(&tmp).output() {
                Ok(o) => {
                    assert!(
                        o.status.success(),
                        "G5 FAIL {} 的内联 script #{} 语法错误（该块内所有 JS 都不会执行）：\n{}",
                        page,
                        n,
                        String::from_utf8_lossy(&o.stderr)
                    );
                    checked += 1;
                }
                Err(_) => {
                    // node 不可用：跳过语法检查，但**其余断言仍然执行**
                    eprintln!("G5 SKIP {} #{} 语法检查（无 node）", page, n);
                }
            }
            let _ = fs::remove_file(&tmp);
        }
    }

    assert!(checked > 0, "G5 FAIL 未检查到任何内联 script 块（提取逻辑可能失效）");

    // G5-b F2：全局错误上报
    assert!(
        all_inline.contains("addEventListener(\"error\"")
            || all_inline.contains("addEventListener('error'")
            || all_inline.contains("window.onerror"),
        "G5 FAIL 前端未注册全局 error 处理器（不变量 F2：静默死亡必须变成可见）"
    );
    assert!(
        all_inline.contains("unhandledrejection"),
        "G5 FAIL 前端未注册 unhandledrejection 处理器（不变量 F2）"
    );

    // G5-c F3：IPC 不可用时明确报错
    assert!(
        all_inline.contains("IPC 不可用") || all_inline.contains("__TAURI_INTERNALS__"),
        "G5 FAIL 前端无 IPC 可用性自检（不变量 F3：不得静默停住）"
    );

    // G5-d 可见的错误容器
    let bootstrap = fs::read_to_string(manifest_dir().join("bootstrap").join("bootstrap.html"))
        .unwrap_or_default();
    assert!(
        bootstrap.contains("id=\"fatal\""),
        "G5 FAIL bootstrap.html 缺少可见的致命错误容器（#fatal）—— 错误必须能被人看见"
    );

    eprintln!("G5 PASS {} inline script blocks checked, F2/F3 present", checked);
}
