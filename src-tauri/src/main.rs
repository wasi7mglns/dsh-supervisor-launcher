#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// dsh-supervisor-gui：桌面壳 + 环境引导器（开源：dsh-supervisor-launcher）。
// 职责：
//   1. 探测系统 Node.js；缺失/过旧 → 内嵌引导页 → 一键安装官方最新 LTS（下载/校验/授权）。
//   2. Node 就绪 → 定位已安装内核（dsh-supervisor SEA 二进制，npm 子包 / ~/.local/bin / PATH）
//      → 拉起守卫 daemon → 面板 127.0.0.1:3100。内核闭源（npm 安装），壳不内嵌任何内核资产。
// 托盘常驻：关窗 = 隐藏；菜单动作直发本地 API（裸 TCP，无额外依赖）。

// 有界子进程执行（公共设施）：所有外部命令一律经它，避免「无界阻塞分散潜伏」。
// 结构化错误模型：IPC 边界统一返回它（前端按 `kind` 分支、并显示后端给的 `hint`）。
mod error;
mod bounded;
mod core;
mod env;
// 镜像源适配（壳自持）：装机时无内核，三处下载都必须自带镜像能力。
mod mirror;
// 有界 Node 探测（架构层修复）：分离线程 + 有界等待 + 缓存 + 追踪。
// 根因：探测内含无界阻塞系统调用，且被命令 await —— 详见本文件根因说明。
mod nodeprobe;
mod node;
// ★ 平台适配层（2026-09-11）：**全仓唯一的平台分支所在地**。
// 它接管了原先分居两处的「服务定义」（service.rs）与「服务启停」（原本文件），
// 消除「同一概念分居两层」的分层违规 —— 加平台不再需要改两处不同层。
/// 业务层（平台无关）：从 main.rs 拆出的可独立测试的模块。
/// IPC 命令边界层（只做校验与委托）。
mod commands;
mod domain;
mod platform;
// 桌面壳自更新 + 落盘日志 + 身份上报（2026-09-11）
mod update;

use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{Emitter, Manager};
// app.updater()：Tauri 官方更新器入口（强制 minisign 验签）。
use tauri_plugin_updater::UpdaterExt;

pub(crate) struct RunState {
    busy: bool,
    installed: Option<String>, // 系统当前 node 版本
    latest: Option<String>,    // 官方最新 LTS
    progress: f32,
    status: String,
    logs: Vec<String>,
    error: Option<String>,
}

impl Default for RunState {
    fn default() -> Self {
        RunState { busy: false, installed: None, latest: None, progress: 0.0, status: "探测中…".into(), logs: vec![], error: None }
    }
}

pub(crate) fn log(state: &RunState) -> serde_json::Value {
    serde_json::json!({
        "busy": state.busy,
        "installed": state.installed,
        "latest": state.latest,
        "progress": state.progress,
        "status": state.status,
        "logs": state.logs,
        "error": state.error,
    })
}








pub(crate) fn push_status(app: &tauri::AppHandle, status: String, progress: f32) {
    {
        let state = app.state::<Mutex<RunState>>();
        let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
        s.status = status.clone();
        s.progress = progress;
        s.logs.push(status.clone());
    }
    let _ = app.emit("env_progress", serde_json::json!({ "status": status, "progress": progress }));
}

fn run_install(app: &tauri::AppHandle) -> Result<(String, String), String> {
    push_status(app, "获取官方最新 LTS 版本…".into(), 0.1);
    let choice = node::latest_lts()?;
    let version = choice.version.clone();
    let file = choice.file.clone();
    // 记录镜像选择（含延迟诊断），便于用户与排障
    mirror::save(&mirror::Mirrors {
        selected_node: Some(choice.source.clone()),
        checked_at: Some(mirror::now_secs()),
        ..mirror::load()
    })
    .ok();
    push_status(app, format!("选用镜像 {}（{}ms）", choice.source, choice.latency_ms), 0.15);
    push_status(app, format!("官方最新 LTS: {}", version), 0.2);
    let dl_dir = env::supervisor_dir().join("dl");
    push_status(app, format!("下载 {}（约 30~50MB）…", file), 0.3);
    let local = node::download_verified(&version, &file, &dl_dir, Some(choice.source.as_str()))?;
    push_status(app, "SHA256 校验通过，准备安装…".into(), 0.8);
    let node_path = node::install(&local)?;
    let node_path = node_path.display().to_string();
    match node::probe_after() {
        Some((_p, v)) if v == version => Ok((node_path, v)),
        Some((_p, v)) => Err(format!("安装后版本 {} 与目标 {} 不一致", v, version)),
        None => Err("安装后未能检测到 Node.js".into()),
    }
}

/// 内核可执行名候选（跨平台）：Windows 上 npm 可能生成 .cmd 垫片；Unix 无扩展名。







// ── 守卫服务的「启停」已迁入 platform 层（2026-09-11）──
//
// 此处原定义 start_guard_service / stop_guard_service（含 8 份 #[cfg]），
// 与**原** service.rs 的「服务定义」（另 4 份 #[cfg]，该文件已并入本层）分居两层 —— 同一概念的 per-OS
// 知识被切开，加一个平台要改两处**不同层**，且很容易只改一处。
//
// 现统一在 platform::service::ServiceControl：**定义与启停永远是同一个对象**。
//
// · 原 GUARD_CMD_TIMEOUT=20s 的有界性 → platform::SVC_NORMAL
// · 原「壳绝不直接 spawn 守卫」的政策 → platform::service::spawn_daemon 的文档
//   （该约束不凌驾于可用性：容器/无 user session/策略拦截等场景需 spawn 兜底）


/// 无头输出：壳自更新基线（供 CI 冒烟与人工诊断）。
/// 副作用：写 identity.json + shell.log（验证落盘链路）。
fn shell_update_plan_text() -> String {
    let v = env!("CARGO_PKG_VERSION").to_string();
    let id = update::init_identity(&v);
    let (should, reason) = update::should_check(&v);
    let mut out = String::new();
    out.push_str(&format!("shell_version={}", id.get("version").and_then(|x| x.as_str()).unwrap_or("?")));
    out.push_str(&format!(" platform={}", id.get("platform").and_then(|x| x.as_str()).unwrap_or("?")));
    out.push_str(&format!(" arch={}", id.get("arch").and_then(|x| x.as_str()).unwrap_or("?")));
    out.push_str(&format!(" install_kind={}", update::install_kind()));
    out.push_str(&format!(" self_update_capable={}", update::self_update_capable()));
    out.push_str(&format!(" attempt={}", id.get("attempt").and_then(|x| x.as_u64()).unwrap_or(0)));
    out.push_str(&format!(" should_check={}", should));
    if !reason.is_empty() { out.push_str(&format!(" reason={}", reason)); }
    out.push_str(&format!(" state_dir={}", update::state_dir().display()));
    out
}



// ── 超时预算（防「无超时网络请求 → 引导页永久卡住」）──
// 事故背景（2026-09-11 Windows 真机实测）：用户装完桌面壳后，引导第一步就是壳更新，
// 而 `check()` 在无超时的情况下遇到网络不可达**永不返回** → 前端 Promise 既不 resolve
// 也不 reject → `.catch` 不触发 → 永久卡在「正在检查桌面更新」，用户无法进入产品。
// 故：check 必须短超时（快速失败），下载必须长超时（大安装包 + 慢网），且外层再加 tokio
// 兜底（reqwest 的 request timeout 不保证覆盖 DNS 等阶段）。

/// 构造带超时的更新器。
/// ⚠ 该超时是 reqwest 的**整个请求**超时：check 用短超时；下载必须用长超时，
///   否则大安装包会在传输中途被切断。
fn shell_updater(
    app: &tauri::AppHandle,
    timeout: std::time::Duration,
) -> Result<tauri_plugin_updater::Updater, String> {
    let mut builder = app.updater_builder().timeout(timeout);
    // 端点运行时覆盖（2026-09-11）：Tauri 配置里写死的 endpoints 是编译期常量，
    // 而不同网络环境下 CDN 可达性差异很大。此处用壳自持的镜像配置覆盖，
    // 使用户（或壳自身测速结果）可以在**不重新编译**的前提下切换更新源。
    let endpoints: Vec<tauri::Url> = crate::mirror::load()
        .shell
        .iter()
        .filter_map(|s| tauri::Url::parse(s).ok())
        .collect();
    if !endpoints.is_empty() {
        builder = builder
            .endpoints(endpoints)
            .map_err(|e| format!("更新端点配置无效: {}", e))?;
    }
    builder
        .build()
        .map_err(|e| format!("更新器不可用: {}", e))
}

// ═══════════════════════════════════════════════════════════════════
// 镜像源适配命令（引导页在**网络失败时**提供手动入口）
//
// 设计意图（2026-09-11）：壳装机时没有内核，面板（RegistryCard）此时不可用，
// 用户若遇到镜像不可达将没有任何出口。故引导页必须在失败时给出可操作的输入框。
// 平时不显示（避免干扰普通用户），仅失败态出现。
// ═══════════════════════════════════════════════════════════════════






/// 无头自检：**守卫服务定义**（P0 关键修复的功能验证入口）。
///
/// 为什么需要它：服务定义由壳在首启时建立，若失败，用户会卡在「守卫就绪」而**无法自查**
/// （GUI 进不去、日志分散）。本入口让你在任何平台无 GUI 地确认：
///   · 服务定义将写到哪个路径；
///   · 当前是否存在；
///   · 守卫可执行文件是否已定位；
///   · `--service-apply` 时**实际建立**并报告结果。
///
/// 用法：
///   dsh-supervisor-gui --service-plan              # 只报告，不写盘
///   dsh-supervisor-gui --service-plan --service-apply   # 实际建立服务定义
fn cli_service_plan() -> i32 {
        println!("== 守卫服务定义自检 ==");
    println!("平台          = {}", std::env::consts::OS);
    println!("服务定义路径  = {}", platform::service().definition_path().display());
    println!("现存          = {}", if platform::service().definition_path().is_file() { "是" } else { "否" });
    println!("HOME          = {}", std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_else(|_| "(未设置)".into()));

    // 守卫可执行文件定位（与实际 ensure_guard 同一路径推导，避免「自检通过但运行时找不到」）。
    // DSH_GUARD_BIN 可显式覆盖：用于①自动定位失败的机器做诊断 ②测试隔离 HOME。
    let guard: Option<PathBuf> = std::env::var("DSH_GUARD_BIN")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(core::locate_core_for_cli);
    match &guard {
        Some(p) => {
            println!("守卫可执行    = {}", p.display());
            println!("守卫存在      = {}", if p.is_file() { "是" } else { "否" });
        }
        None => println!("守卫可执行    = （未定位到，请先安装内核）"),
    }

    let apply = std::env::args().any(|a| a == "--service-apply");
    if !apply {
        println!("");
        println!("（未写盘。加 --service-apply 实际建立服务定义）");
        return 0;
    }
    let Some(g) = guard else {
        eprintln!("无法建立：未定位到守卫可执行文件（先安装内核）");
        return 2;
    };
    match platform::service().ensure_defined(&g) {
        Ok(desc) => {
            println!("");
            println!("建立结果      = {}", desc);
            println!("建立后现存    = {}", if platform::service().definition_path().is_file() { "是" } else { "否" });
            0
        }
        Err(e) => {
            eprintln!("建立失败      = {}", e);
            1
        }
    }
}
fn main() {
    // 启动阶段诊断（**临时**，定位「卡在检测」用）：写入 stderr，任何平台可见。
    // ══════════════════════════════════════════════════════════════════
    // 启动里程碑日志（**常开**，落盘到 ~/.dsh/shell/shell.log）。
    //
    // 为什么必须是常开能力（真实教训）：引导页卡住时，`shell.log` 若只有
    //   「壳启动」一行，就**无法区分**这两种截然不同的病因：
    //     (a) Rust 侧 setup() 从未执行（插件/DBus 层失败）；
    //     (b) setup() 正常、但前端 JS 从未执行（语法错误 / IPC 失败）。
    //   两者的修复方向完全相反，而我为此**来回排查了三轮**。
    //   现在这条轨迹是 always-on 的：打开 shell.log 一眼就能定位到哪一层。
    //
    // 成本：每次启动 6 行；shell.log 超过 1MB 自动滚动（见 update::log）。
    // ══════════════════════════════════════════════════════════════════
    macro_rules! bt {
        ($($a:tt)*) => {
            crate::update::log(&format!("[boot] {}", format!($($a)*)));
        };
    }
    bt!("main enter");
    // 无头自检：镜像测速与选择（用户要求「镜像必须可见」的验证入口）。
    if std::env::args().any(|a| a == "--mirror-plan") {
        std::process::exit(domain::cli::cli_mirror_plan());
    }
    // 无头自检：环境探测（架构修复后的可诊断入口）。
    if std::env::args().any(|a| a == "--env-plan") {
        std::process::exit(domain::cli::cli_env_plan());
    }
    // 无头自检：守卫服务定义（P0 修复的功能验证入口，任何平台可用）。
    if std::env::args().any(|a| a == "--service-plan") {
        std::process::exit(cli_service_plan());
    }
    // 无头冒烟入口：--node-plan 仅打印环境探针 + 官方最新 LTS，不启动窗口。
    if std::env::args().any(|a| a == "--node-plan") {
        std::process::exit(domain::cli::cli_plan());
    }
    // 无头自检：**平台矩阵**（2026-09-11，门禁 A4/G4）。
    //
    // 目的：把「平台矩阵」从**文档承诺**变成**可执行断言** ——
    //   文档说支持某能力但代码没实现，这一输出会在三平台 CI 上暴露。
    //   同时它是「平台适配层真的被接上」的活体证据（否则全是 dead_code 警告）。
    if std::env::args().any(|a| a == "--platform-matrix") {
        println!("{}", platform::matrix_text());
        std::process::exit(0);
    }
    // 无头自检：内核版本治理（包名/镜像/最新版本）——发布后冒烟验证，无需 GUI。
    if std::env::args().any(|a| a == "--core-plan") {
        println!("{}", core::plan_text());
        std::process::exit(0);
    }
    // 无头自检：壳自更新能力基线——发布后冒烟验证 + CI 门禁，无需 GUI。
    // 输出身份/安装形态/是否可自更新/护栏状态；同时**实际写一次** identity.json 与 shell.log，
    // 以验证落盘链路可用（这是「壳零日志、无法诊断」问题的结构性修复）。
    if std::env::args().any(|a| a == "--shell-update-plan") {
        println!("{}", shell_update_plan_text());
        std::process::exit(0);
    }
    bt!("building app");
    tauri::Builder::default()
        // 单实例管控（2026-09）：同一 user 会话内只允许一个壳实例——重复启动第二实例时
        // 插件自动让新进程退出，回调里唤起既有主窗口（show+focus+导航面板），避免双壳/多壳并存。
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            domain::windowing::show_main(app);
        }))
        // 壳自更新插件：强制 minisign 验签；平台安装语义内部处理。
        // 未配置 pubkey 时插件仍可注册（check 会失败并返回错误，由引导页按「失败放行」处理）。
        .plugin(tauri_plugin_updater::Builder::new().build())
        // app.restart()：更新安装后重启进入新版本（旧进程装、新进程跑）。
        .plugin(tauri_plugin_process::init())
        .manage(Mutex::new(RunState::default()))
        .invoke_handler(tauri::generate_handler![commands::node_status, commands::core_status, commands::core_plan, commands::core_apply, commands::guard_start, commands::guard_ready, commands::start_node_install, commands::finish_boot, commands::win_ctl, commands::shell_identity, commands::shell_reset_update_guard, commands::shell_update_check, commands::shell_update_apply, commands::shell_restart, commands::shell_set_phase, commands::mirror_status, commands::mirror_set, commands::node_latest, commands::mirror_warmup, commands::mirror_cached, commands::shell_panel_url])
        .setup(|app| {
            bt!("setup enter");
            // 托盘直发本地 API 的端口：显式 DSH_SUPERVISOR_TRAY_PORT 优先，否则从用户 config.apiPort 解析
            let port: u16 = std::env::var("DSH_SUPERVISOR_TRAY_PORT")
                .ok().and_then(|p| p.parse().ok()).unwrap_or_else(|| env::api_port());
            let handle = app.handle().clone();

            // 壳身份初始化（2026-09-11）：写 ~/.dsh/shell/identity.json + shell.log，
            // 并推进「更新护栏」状态（pendingVersion 是否生效 / attempt 计数 / 拉黑）。
            // 必须尽量早执行：即使后续任一环节失败，也留下可诊断的落盘痕迹。
            bt!("init_identity...");
            let _ = update::init_identity(&app.package_info().version.to_string());
            bt!("init_identity done");

            // 镜像契约**无条件导出**（2026-09-11 架构修复）：
            //   旧实现只在 node::latest_lts() 内导出，而该函数在「离线」或
            //   「全部 Node 镜像不可达」时返回 Err → **契约完全不写**，
            //   内核便只能用它自己的硬编码副本（与壳的目录可能已分叉）。
            //   故在壳启动时无条件导出一份，保证内核永远有契约可读。
            //   ⚠ 不阻断引导：失败只记 shell.log。
            crate::mirror::export_on_boot();

            // 环境判定（2026-09 改）：Node 缺失或低于最低标准(>=22.12, DSH commander 硬门槛) → 引导页安装；
            // 达标（即使不是最新 LTS）→ 直接拉起守卫进入面板，不卡升级。初始 url 即 bootstrap.html。
            // ⚠ **不得在此阻塞**（架构修复 2026-09-11）：
            //   旧实现在这里同步调用探测。setup 在**窗口创建之前**运行，
            //   而探测内含无界阻塞系统调用（CreateProcessW / GetFileAttributesW
            //   在网络路径上无上限）—— 一旦挂起，连窗口都会被推迟出现，
            //   且失败现象是「启动慢/无窗口」，与真正的病因相距极远。
            //   现改为：仅**触发**探测（分离线程），结果由引导页异步等待。
            //   UI 立即可见是硬要求 —— 检测再慢也不能挡住界面。
            nodeprobe::start();
            {
                let h = handle.clone();
                std::thread::spawn(move || {
                    // 在飞探测最多等 45 秒（与引导页预算一致）；未完成则放弃本次回填，
                    // 下次 node_status 轮询仍会拿到结果。
                    let out = nodeprobe::status(std::time::Duration::from_secs(45));
                    if let Some(v) = out.version {
                        let st = h.state::<Mutex<RunState>>();
                        st.lock().unwrap_or_else(|e| e.into_inner()).installed = Some(v);
                    }
                });
            }
            // 单一引导流程（K3 修复）：壳启动只做「环境信息探测」，**不再并行拉起守卫**。
            // 守卫的安装/升级/启动全部由引导页显式驱动
            // （core_plan → core_apply → guard_start → guard_ready），杜绝两条互不知晓的流程竞争，
            // 以及「Rust 侧已尝试拉起但页面不知情」的假成功。
            std::thread::spawn(move || {
                if let Ok(c) = node::latest_lts() {
                    let v = c.version.clone();
                    let state = handle.state::<Mutex<RunState>>();
                    let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
                    s.latest = Some(v.clone());
                    drop(s);
                    let _ = handle.emit("env_status", serde_json::json!({ "latest": v }));
                }
            });

            bt!("setup: building tray");
            // ── 托盘 ──
            let show_m = tauri::menu::MenuItem::with_id(app, "show", "显示控制面板", true, None::<&str>)?;
            let start = tauri::menu::MenuItem::with_id(app, "start", "启动 DSH", true, None::<&str>)?;
            let stop = tauri::menu::MenuItem::with_id(app, "stop", "停止 DSH", true, None::<&str>)?;
            let restart = tauri::menu::MenuItem::with_id(app, "restart", "重启一次", true, None::<&str>)?;
            let quit = tauri::menu::MenuItem::with_id(app, "quit", "退出管家", true, None::<&str>)?;
            let menu = tauri::menu::Menu::with_items(app, &[&show_m, &start, &stop, &restart, &quit])?;

            tauri::tray::TrayIconBuilder::with_id("dsh-supervisor-tray")
                .icon(app.default_window_icon().expect("no default icon").clone())
                .tooltip("dsh-supervisor")
                .menu(&menu)
                // 左键=显示窗口 / 右键=弹出菜单（Windows·Linux 惯例）。
                // ⚠ 原为 true（左键也弹菜单），叠加下方 on_tray_icon_event 不区分按键，
                //   导致右键时既弹菜单又调用 domain::windowing::show_main() 抢焦点 → 菜单被顶掉，
                //   用户感知为「右键不好用」（2026-09-11 Windows 真机实测）。
                // 注：上游文档明确 Linux 不支持该开关（菜单由桌面环境决定）——
                //     故 Linux 上左键可能仍显示菜单，属平台限制，非本仓可控。
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| {
                    match event.id.as_ref() {
                        "show" => domain::windowing::show_main(app),
                        // ⚠ 网络 I/O **必须离开 UI 线程**（2026-09-11 架构修复）。
                        //   托盘菜单事件由 UI 线程派发，而 post_local 最多阻塞 60 秒
                        //   （TCP 连接 + 读写超时）。守卫挂起或端口无响应时，
                        //   点击「启动/停止/重启」会**把整个界面冻结 60 秒** ——
                        //   用户看到的是「点了没反应」，且期间窗口无法重绘。
                        //   改为派发到独立线程：菜单立即响应，结果异步生效。
                        "start" => domain::localhttp::spawn_local_post(port, "/lifecycle/dsh/start"),
                        "stop" => domain::localhttp::spawn_local_post(port, "/lifecycle/dsh/stop"),
                        "restart" => domain::localhttp::spawn_local_post(port, "/lifecycle/dsh/restart"),
                        // 退出管家 = 完全退出：通知守卫停止全部服务链，随后壳退出
                        "quit" => {
                            // 契约 §4.1：请求内核停被管对象（等回执）→ 由所有者停止守卫 → 壳退出。
                            // ⚠ 同样离开 UI 线程：退出握手最坏可耗时约 70 秒（/session/stop 60s
                            //   + 轮询 10s）。若在 UI 线程做，用户会看到窗口卡住不动，
                            //   误以为「程序关不掉」而强杀 —— 那会跳过退出握手，留下未停的 DSH。
                            let h = app.clone();
                            std::thread::spawn(move || {
                                domain::guardctl::shutdown_all(port);
                                h.exit(0);
                            });
                        }
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    // ⚠ 必须区分按键与状态（2026-09-11 修复）：
                    //   原实现匹配 `Click { .. }`（任意键、任意状态）→ **右键**也会 show_main，
                    //   把刚要弹出的右键菜单顶掉/抢走焦点。现只响应「左键 + 抬起」，
                    //   右键交由系统弹出 .menu() 设置的菜单。
                    if let tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        button_state: tauri::tray::MouseButtonState::Up,
                        ..
                    } = event
                    {
                        domain::windowing::show_main(tray.app_handle());
                    }
                })
                .build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // 关闭窗口行为（读守卫 config.closeAction，系统级开关 2026-09）：
                // 'exit' = 退出管家（通知守卫停止全部服务链 + 壳退出）；默认 'hide' = 隐藏至托盘常驻。
                if env::close_action() == "exit" {
                    let app = window.app_handle();
                    let port = env::api_port();
                    // 契约 §4.1：停被管对象（等回执）→ 由所有者停止守卫 → 壳退出
                    domain::guardctl::shutdown_all(port);
                    let _ = app.exit(0);
                    return;
                }
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running dsh-supervisor-gui");
    bt!("main exit (run returned)");
}
