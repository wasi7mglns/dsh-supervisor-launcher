#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// dsh-supervisor-gui：桌面壳 + 环境引导器（开源：dsh-supervisor-launcher）。
// 职责：
//   1. 探测系统 Node.js；缺失/过旧 → 内嵌引导页 → 一键安装官方最新 LTS（下载/校验/授权）。
//   2. Node 就绪 → 定位已安装内核（dsh-supervisor SEA 二进制，npm 子包 / ~/.local/bin / PATH）
//      → 拉起守卫 daemon → 面板 127.0.0.1:3100。内核闭源（npm 安装），壳不内嵌任何内核资产。
// 托盘常驻：关窗 = 隐藏；菜单动作直发本地 API（裸 TCP，无额外依赖）。

mod env;
mod node;

use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{Emitter, Manager};

struct RunState {
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

fn log(state: &RunState) -> serde_json::Value {
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

#[tauri::command]
fn node_status(state: tauri::State<Mutex<RunState>>, app: tauri::AppHandle) -> serde_json::Value {
    // 每次查询都做一次真实探针（PATH 变化会即时反映）
    let sys = env::probe_system_node();
    let st = state.lock().unwrap();
    let installed = st.installed.clone().or_else(|| sys.as_ref().map(|x| x.1.clone()));
    // 后台未拉过最新版时，启动线程补一次
    if st.latest.is_none() && !st.busy {
        let handle = app.clone();
        std::thread::spawn(move || {
            if let Ok((v, _f)) = node::latest_lts() {
                let state = handle.state::<Mutex<RunState>>();
                let mut s = state.lock().unwrap();
                s.latest = Some(v.clone());
                let _ = handle.emit("env_status", serde_json::json!({ "latest": v }));
            }
        });
    }
    let mut o = log(&st);
    o["installed"] = serde_json::json!(installed);
    // DSH 最低门槛判定（>=22.12）：引导页据此决定是否需装 Node，outdated 仅展示不再阻塞
    o["minOk"] = serde_json::json!(node::meets_minimum(installed.as_deref()));
    if let Some(latest) = &st.latest {
        o["outdated"] = serde_json::json!(node::outdated(installed.as_deref(), latest));
    }
    o
}

#[tauri::command]
fn skip_env_upgrade(app: tauri::AppHandle) -> Result<(), String> {
    if env::probe_system_node().is_none() {
        return Err("缺少 Node.js，无法跳过（请先安装）".into());
    }
    // 用现有版本继续：拉起守卫并进入面板（api 基址从用户 config 解析，用户改 apiPort 后壳仍可达）
    ensure_guard(&app).map_err(|e| e.to_string())?;
    go_panel(&app);
    Ok(())
}

/// 引导页走完所有检测步骤后调用：进入面板（go_panel —— emit 守卫 URL，壳框架 iframe 切换）。
/// 由引导页 JS 在展示完整启动过程后触发，避免步骤一闪而过无感知。
#[tauri::command]
fn finish_boot(app: tauri::AppHandle) -> Result<(), String> {
    go_panel(&app);
    Ok(())
}

#[tauri::command]
fn start_node_install(state: tauri::State<Mutex<RunState>>, app: tauri::AppHandle) -> Result<(), String> {
    let mut st = state.lock().unwrap();
    if st.busy { return Ok(()); }
    st.busy = true;
    st.error = None;
    st.progress = 0.0;
    st.status = "准备安装 Node.js LTS…".into();
    st.logs.clear();
    let handle = app.clone();
    std::thread::spawn(move || {
        let out = run_install(&handle);
        let state = handle.state::<Mutex<RunState>>();
        let mut s = state.lock().unwrap();
        s.busy = false;
        match out {
            Ok((node_path, version)) => {
                s.installed = Some(version.clone());
                s.progress = 1.0;
                s.status = format!("Node.js {} 就绪，正在启动守卫…", version);
                s.logs.push(format!("安装完成: {} @ {}", version, node_path));
                node::record_runtime_meta(&node_path, &version);
                let _ = handle.emit("env_done", serde_json::json!({ "version": version }));
            }
            Err(e) => {
                s.error = Some(e.clone());
                s.status = "环境就绪前置失败".into();
                s.logs.push(format!("失败: {}", e));
                let _ = handle.emit("env_error", serde_json::json!({ "error": e }));
            }
        }
        let _ = handle.emit("env_progress", log(&s));
    });
    drop(st);
    Ok(())
}

fn push_status(app: &tauri::AppHandle, status: String, progress: f32) {
    {
        let state = app.state::<Mutex<RunState>>();
        let mut s = state.lock().unwrap();
        s.status = status.clone();
        s.progress = progress;
        s.logs.push(status.clone());
    }
    let _ = app.emit("env_progress", serde_json::json!({ "status": status, "progress": progress }));
}

fn run_install(app: &tauri::AppHandle) -> Result<(String, String), String> {
    push_status(app, "获取官方最新 LTS 版本…".into(), 0.1);
    let (version, file) = node::latest_lts()?;
    push_status(app, format!("官方最新 LTS: {}", version), 0.2);
    let dl_dir = env::supervisor_dir().join("dl");
    push_status(app, format!("下载 {}（约 30~50MB）…", file), 0.3);
    let local = node::download_verified(&version, &file, &dl_dir)?;
    push_status(app, "SHA256 校验通过，准备安装…".into(), 0.8);
    let node_path = node::install(&local)?;
    let node_path = node_path.display().to_string();
    match node::probe_after() {
        Some((_p, v)) if v == version => Ok((node_path, v)),
        Some((_p, v)) => Err(format!("安装后版本 {} 与目标 {} 不一致", v, version)),
        None => Err("安装后未能检测到 Node.js".into()),
    }
}

fn core_exe() -> &'static str {
    if cfg!(windows) { "dsh-supervisor.exe" } else { "dsh-supervisor" }
}

/// 定位已安装的内核（SEA 二进制，自足无需 Node）：PATH → ~/.local/bin → ~/.npm-global/bin → 旧内嵌兜底。
fn locate_core(app: &tauri::AppHandle) -> Option<PathBuf> {
    let name = core_exe();
    if let Some(p) = env::find_in_path(name) { return Some(p); }
    let home = env::home();
    let alts = [
        home.join(".local").join("bin").join(name),
        home.join(".npm-global").join("bin").join(name),
    ];
    for a in alts { if a.is_file() { return Some(a); } }
    if let Ok(res) = app.path().resource_dir() {
        let p = res.join("bin").join(name);
        if p.is_file() { return Some(p); } // 旧版内嵌资产升级过渡
    }
    None
}

/// 引导页查询用：内核是否已安装 + 安装提示。
#[tauri::command]
fn core_status(app: tauri::AppHandle) -> serde_json::Value {
    let installed = locate_core(&app).is_some();
    serde_json::json!({
        "installed": installed,
        "hint": "npm i -g @dsh-core/dsh-core-<platform>-<arch>"
    })
}

/// 桌面自定义窗口控制（Phase 3b：无边框窗口 + 自绘标题栏）。
/// 前端 WindowTitlebar 按钮 → invoke("win_ctl", {action})：
///   "minimize" / "toggle-maximize" / "hide"（关闭按钮 = 隐藏到托盘，与 CloseRequested 语义一致）。
#[tauri::command]
fn win_ctl(app: tauri::AppHandle, action: String) -> Result<(), String> {
    let win = app.get_webview_window("main").ok_or("主窗口不存在")?;
    match action.as_str() {
        "minimize" => win.minimize().map_err(|e| e.to_string()),
        "toggle-maximize" => {
            if win.is_maximized().unwrap_or(false) {
                win.unmaximize().map_err(|e| e.to_string())
            } else {
                win.maximize().map_err(|e| e.to_string())
            }
        }
        "maximize" => win.maximize().map_err(|e| e.to_string()),
        "unmaximize" => win.unmaximize().map_err(|e| e.to_string()),
        "hide" => {
            let _ = win.hide();
            Ok(())
        }
        "drag" => {
            // 显式窗口拖动（Linux WebKitGTK drag-region 属性常不生效的可靠替代）：
            // 前端标题栏拖动区 mousedown → invoke win_ctl drag → 走 Rust start_dragging
            win.start_dragging().map_err(|e| e.to_string())
        }
        _ => Err(format!("不支持的窗口动作: {}（minimize/toggle-maximize/maximize/unmaximize/hide/drag）", action)),
    }
}

/// 拉起守卫（定位已安装内核 dsh-supervisor daemon——SEA 自足，不依赖 Node），等面板就绪。
/// 端口从用户 config.apiPort 解析（非硬编码 3100——壳极少更新但内核配置可演进，2026-09 修复 F7）。
fn ensure_guard(app: &tauri::AppHandle) -> Result<(), String> {
    let port = env::api_port();
    if is_alive(port) { return Ok(()); }
    let bin = locate_core(app).ok_or_else(|| format!(
        "未检测到内核 ({})。请先安装：npm i -g @dsh-core/dsh-core-<platform>-<arch>", core_exe()
    ))?;
    let mut cmd = std::process::Command::new(&bin);
    cmd.arg("daemon");
    if let Ok(p) = std::env::var("PATH") {
        let ng = env::home().join(".npm-global").join("bin");
        cmd.env("PATH", format!("{}:{}", ng.display(), p));
    }
    let _child = cmd.spawn().map_err(|e| format!("拉起守卫失败: {}", e))?;
    // 等面板就绪（最多 30s）
    for _ in 0..60 {
        if is_alive(port) { return Ok(()); }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    Err("守卫启动超时（30s 内面板未就绪）".into())
}

fn is_alive(port: u16) -> bool {
    let addr = format!("127.0.0.1:{}", port);
    if let Ok(mut it) = addr.to_socket_addrs() {
        if let Some(sa) = it.next() {
            return TcpStream::connect_timeout(&sa, std::time::Duration::from_millis(400)).is_ok();
        }
    }
    false
}

fn cli_plan() -> i32 {
    let sys = env::probe_system_node();
    match &sys {
        Some((p, v)) => println!("node=present {} @ {}", v, p.display()),
        None => println!("node=missing"),
    }
    match node::latest_lts() {
        Ok((v, f)) => {
            println!("latest_lts={} file={}", v, f);
            println!("node_outdated={}", node::outdated(sys.as_ref().map(|x| x.1.as_str()), &v));
            0
        }
        Err(e) => { eprintln!("latest_lts_error={}", e); 1 }
    }
}

/// 打开面板（分体架构 2026-09-07 定稿）：
/// 壳 = 自绘窗口容器(shell.html 唯一窗口栏 + 内容 iframe)；面板由守卫内核 HTTP 托管（同源）。
/// 切面板 = emit 守卫实际 API 基址(读 config.apiPort, 动态端口不硬编码) → 壳 iframe 导航该 URL，
/// 页面与守卫 API 同源直连（无跨源/CORS 透传）。
fn go_panel(app: &tauri::AppHandle) {
    let url = env::api_base_url(); // http://127.0.0.1:<config.apiPort 或高位段 fallback>/
    // 壳框架(shell.html)的 evt listener 在首帧注册；setup 线程的 emit 可能早于注册被丢弃，
    // 故延时重发数次覆盖竞态（listener 就绪后任一次生效即切面板）。
    for (i, delay_ms) in [400u64, 1200, 2500].iter().enumerate() {
        let h = app.clone();
        let u = url.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(*delay_ms));
            let _ = h.emit("shell:goto-panel", serde_json::json!({ "url": u, "seq": i }));
        });
    }
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
        // 每次显示都重新导航到面板（壳内 supervisor.html）：WebView 不做陈旧缓存 —— 拿最新 UI
        go_panel(app);
    }
}

fn post_local(port: u16, path: &str) {
    let addr = format!("127.0.0.1:{}", port);
    if let Ok(mut stream) = TcpStream::connect(addr) {
        let req = format!(
            "POST {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            path, port
        );
        let _ = std::io::Write::write_all(&mut stream, req.as_bytes());
        let mut buf = Vec::new();
        let _ = std::io::Read::read_to_end(&mut stream, &mut buf);
    }
}

fn main() {
    // 无头冒烟入口：--node-plan 仅打印环境探针 + 官方最新 LTS，不启动窗口。
    if std::env::args().any(|a| a == "--node-plan") {
        std::process::exit(cli_plan());
    }
    tauri::Builder::default()
        .manage(Mutex::new(RunState::default()))
        .invoke_handler(tauri::generate_handler![node_status, core_status, start_node_install, skip_env_upgrade, finish_boot, win_ctl])
        .setup(|app| {
            // 托盘直发本地 API 的端口：显式 DSH_SUPERVISOR_TRAY_PORT 优先，否则从用户 config.apiPort 解析
            let port: u16 = std::env::var("DSH_SUPERVISOR_TRAY_PORT")
                .ok().and_then(|p| p.parse().ok()).unwrap_or_else(|| env::api_port());
            let handle = app.handle().clone();

            // 环境判定（2026-09 改）：Node 缺失或低于最低标准(>=22.12, DSH commander 硬门槛) → 引导页安装；
            // 达标（即使不是最新 LTS）→ 直接拉起守卫进入面板，不卡升级。初始 url 即 bootstrap.html。
            let have = env::probe_system_node();
            {
                let st = handle.state::<Mutex<RunState>>();
                if let Some((_p, v)) = have.as_ref() { st.lock().unwrap().installed = Some(v.clone()); }
            }
            std::thread::spawn(move || {
                let latest = node::latest_lts().map(|x| x.0).ok();
                if let Some(v) = &latest {
                    let state = handle.state::<Mutex<RunState>>();
                    let mut s = state.lock().unwrap();
                    s.latest = Some(v.clone());
                    drop(s);
                    let _ = handle.emit("env_status", serde_json::json!({ "latest": v }));
                }
                // 引导门槛（2026-09 改）：Node 达到 DSH 最低标准(>=22.12)即放行，不要求最新 LTS。
                // latest 仅作 node_status 信息展示，不再阻塞升级。
                let need = !node::meets_minimum(have.as_ref().map(|x| x.1.as_str()));
                if !need {
                    // Node 达标：拉起守卫（不自动跳面板——引导页须完整走完步骤并展示每步检测
                    // 反馈后才由页面请求进面板，保留启动感知过程）。
                    if locate_core(&handle).is_some() {
                        let guard_ok = ensure_guard(&handle).is_ok();
                        let _ = handle.emit("env_ready", serde_json::json!({
                            "core": guard_ok,
                            "installed": have.as_ref().map(|x| x.1.clone()),
                        }));
                    } else {
                        // 留在引导页；页面 core_status → 显示「内核未安装」
                        let _ = handle.emit("env_error", serde_json::json!({
                            "error": "未检测到内核 (dsh-supervisor)。请先安装：npm i -g @dsh-core/dsh-core-<platform>-<arch>"
                        }));
                    }
                }
                // need=true：留在引导页（页面 node_status 缺失 Node 时自动走安装流程）
            });

            // ── 托盘 ──
            let show_m = tauri::menu::MenuItem::with_id(app, "show", "显示面板", true, None::<&str>)?;
            let start = tauri::menu::MenuItem::with_id(app, "start", "启动 DSH", true, None::<&str>)?;
            let stop = tauri::menu::MenuItem::with_id(app, "stop", "停止 DSH", true, None::<&str>)?;
            let restart = tauri::menu::MenuItem::with_id(app, "restart", "重启一次", true, None::<&str>)?;
            let quit = tauri::menu::MenuItem::with_id(app, "quit", "退出托盘", true, None::<&str>)?;
            let menu = tauri::menu::Menu::with_items(app, &[&show_m, &start, &stop, &restart, &quit])?;

            tauri::tray::TrayIconBuilder::with_id("dsh-supervisor-tray")
                .icon(app.default_window_icon().expect("no default icon").clone())
                .tooltip("dsh-supervisor")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(move |app, event| {
                    match event.id.as_ref() {
                        "show" => show_main(app),
                        "start" => post_local(port, "/start"),
                        "stop" => post_local(port, "/stop"),
                        "restart" => post_local(port, "/restart"),
                        "quit" => app.exit(0),
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click { .. } = event {
                        show_main(tray.app_handle());
                    }
                })
                .build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running dsh-supervisor-gui");
}
