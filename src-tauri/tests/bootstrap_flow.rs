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

/// B8：内核步骤同样受网络支配，必须也有界。
/// 与壳更新同类：registry 查询与 npm install 都可能因网络停滞而永久阻塞。
#[test]
fn b8_core_steps_are_bounded() {
    let html = bootstrap_html();
    assert!(html.contains("CORE_PLAN_BUDGET_MS"), "B8 FAIL 内核版本检查缺前端超时预算");
    assert!(html.contains("CORE_APPLY_BUDGET_MS"), "B8 FAIL 内核安装缺前端超时预算");
    assert!(
        html.contains("withTimeout(core.invoke('core_plan')"),
        "B8 FAIL core_plan 未用 withTimeout 包裹"
    );
    assert!(
        html.contains("withTimeout(core.invoke('core_apply'"),
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
        let mut idx = 0;
        while let Some(p) = html[idx..].find(bad) {
            let abs = idx + p;
            let line_start = html[..abs].rfind(char::is_whitespace).map(|x| x + 1).unwrap_or(0);
            let line_end = html[abs..].find(char::is_whitespace).map(|x| abs + x).unwrap_or(html.len());
            panic!("B12 FAIL 仍含旧措辞「{}」：{}", bad, html[line_start..line_end].trim());
        }
        idx += bad.len();
    }
    eprintln!("B12 PASS no stale desktop-update wording");
}

/// B13：服务定义必须由壳建立（P0 死锁修复）。
/// 首启时壳是唯一在场组件；若壳只 start 不 create，全新机器上守卫永远起不来。
#[test]
fn b13_shell_owns_service_definition() {
    let svc = fs::read_to_string(manifest_dir().join("src").join("service.rs"))
        .expect("B13 FAIL 缺少 src/service.rs（服务定义模块）");
    for needle in [
        "dsh-supervisor.service",            // Linux systemd unit
        "com.dsh.supervisor.plist",          // macOS LaunchAgent
        "DSH-Supervisor",                    // Windows 计划任务
        "pub fn ensure_defined",             // 建立入口
        "pub fn spawn_daemon",               // spawn 兜底
    ] {
        assert!(svc.contains(needle), "B13 FAIL service.rs 缺少关键项: {}", needle);
    }
    // 模板必须**内嵌**（外部文件在 npm 包内不存在，正是旧实现静默跳过的根因）
    assert!(svc.contains("WantedBy=default.target"), "B13 FAIL systemd 模板未内嵌");
    assert!(svc.contains("RunAtLoad"), "B13 FAIL LaunchAgent plist 未内嵌");

    let m = main_rs();
    assert!(m.contains("mod service;"), "B13 FAIL main.rs 未引入 service 模块");
    assert!(m.contains("service::ensure_defined"), "B13 FAIL ensure_guard 未建立服务定义");
    assert!(m.contains("service::spawn_daemon"), "B13 FAIL 缺少 spawn 兜底调用");
    eprintln!("B13 PASS shell owns 3-platform service definition + spawn fallback");
}

/// B14：macOS Node 安装格式必须与安装命令匹配（.pkg）。
/// 旧实现下载 .tar.gz 却交给 `installer -pkg` —— 格式不匹配，必然失败。
#[test]
fn b14_macos_node_installer_format_matches() {
    let n = fs::read_to_string(manifest_dir().join("src").join("node.rs")).expect("read node.rs");
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
    let m = main_rs();
    assert!(m.contains("minRequired"), "B16 FAIL 后端未回传 minRequired");
    eprintln!("B16 PASS min-node gate enforced");
}

/// B17：npm 形态的包根解析必须正确（launcher 后 __dirname 是包根，
/// 旧实现 path.join(__dirname, ..) 指向包外，导致模板与 bin 路径错位）。
#[test]
fn b17_package_root_resolution_is_form_agnostic() {
    let bin = fs::read_to_string(manifest_dir().join("..").join("..").join("bin").join("dsh-supervisor"))
        .expect("read bin/dsh-supervisor");
    assert!(bin.contains("findPackageRoot"), "B17 FAIL 未按 package.json 定位包根");
    assert!(
        !bin.contains("const ROOT = path.join(__dirname, '..')"),
        "B17 FAIL 仍用 __dirname/.. 硬推包根（发行态会指向包外）"
    );
    assert!(bin.contains("path.join(ROOT, 'bin', 'dsh-supervisor')"), "B17 FAIL bin 目标未按包根解析");
    eprintln!("B17 PASS package root resolution form-agnostic");
}

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
    assert!(m.contains("CACHE_TTL_SECS"), "B18 FAIL 缺测速缓存");
    // 壳自持配置 + 导出给内核
    assert!(m.contains("mirrors.json"), "B18 FAIL 缺壳自持配置文件");
    assert!(m.contains("export_to_kernel"), "B18 FAIL 未导出偏好给内核（内核会重新盲选）");
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

/// B23：三处预设集合必须一致（壳 mirror.rs / 壳 core.rs / 内核 config）。
#[test]
fn b23_preset_sets_are_consistent() {
    let core = fs::read_to_string(manifest_dir().join("src").join("core.rs")).expect("core.rs");
    assert!(
        core.contains("const DEFAULT_ORIGINS: [&str; 6]"),
        "B23 FAIL 壳 core.rs 的 DEFAULT_ORIGINS 未同步到 6 个"
    );
    // 内核 config.js 也应含新增的两个源
    let cfg_path = manifest_dir().join("..").join("..").join("src").join("platform").join("config.js");
    let cfg = fs::read_to_string(&cfg_path).expect("config.js");
    for needle in ["npmreg.proxy.ustclug.org", "r.cnpmjs.org"] {
        assert!(cfg.contains(needle), "B23 FAIL 内核 config.registries 缺 {}", needle);
    }
    eprintln!("B23 PASS preset sets consistent");
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
    let m = main_rs();
    assert!(
        m.contains("fn locate_core_candidates(resource_dir: Option<PathBuf>)"),
        "B25 FAIL 候选定位仍要求 AppHandle（CLI 无法复用）"
    );
    let c = fs::read_to_string(manifest_dir().join("src").join("core.rs")).expect("core.rs");
    assert!(c.contains("locate_core_for_cli"), "B25 FAIL 缺 CLI 专用定位函数");
    eprintln!("B25 PASS candidates usable without AppHandle");
}
