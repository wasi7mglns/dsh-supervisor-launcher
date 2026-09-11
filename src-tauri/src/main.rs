#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// dsh-supervisor-gui：桌面壳 + 环境引导器（开源：dsh-supervisor-launcher）。
// 职责：
//   1. 探测系统 Node.js；缺失/过旧 → 内嵌引导页 → 一键安装官方最新 LTS（下载/校验/授权）。
//   2. Node 就绪 → 定位已安装内核（dsh-supervisor SEA 二进制，npm 子包 / ~/.local/bin / PATH）
//      → 拉起守卫 daemon → 面板 127.0.0.1:3100。内核闭源（npm 安装），壳不内嵌任何内核资产。
// 托盘常驻：关窗 = 隐藏；菜单动作直发本地 API（裸 TCP，无额外依赖）。

mod core;
mod env;
mod node;
// 桌面壳自更新 + 落盘日志 + 身份上报（2026-09-11）
mod update;

use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{Emitter, Manager};
// app.updater()：Tauri 官方更新器入口（强制 minisign 验签）。
use tauri_plugin_updater::UpdaterExt;

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


/// 引导页走完所有检测步骤后调用：进入面板（go_panel —— emit 守卫 URL，壳框架 iframe 切换）。
/// 由引导页 JS 在展示完整启动过程后触发，避免步骤一闪而过无感知。
#[tauri::command]
fn finish_boot(app: tauri::AppHandle) -> Result<(), String> {
    go_panel(&app, false); // 引导完成：URL 变化即导航
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

/// 内核可执行名候选（跨平台）：Windows 上 npm 可能生成 .cmd 垫片；Unix 无扩展名。

fn core_exe_names() -> &'static [&'static str] {
    if cfg!(windows) { &["dsh-supervisor.exe", "dsh-supervisor.cmd", "dsh-supervisor"] }
    else { &["dsh-supervisor"] }
}

/// 收集全部内核候选（去重 + 解析符号链接），供「按版本最高仲裁」使用。
/// 跨平台路径规范：
///   - PATH（env::find_in_path，Windows 走 PATHEXT）
///   - Windows: %APPDATA%\\npm（npm 全局 bin 目录）
///   - macOS:   /opt/homebrew/bin（Apple Silicon）、/usr/local/bin（Intel）
///   - Unix:    ~/.npm-global/bin、~/.local/bin（内核 install 写入的软链）
///   - 资源目录内嵌兜底（旧版过渡）
fn locate_core_candidates(app: &tauri::AppHandle) -> Vec<PathBuf> {
    let home = env::home();
    let mut out: Vec<PathBuf> = Vec::new();
    let mut add = |p: PathBuf, out: &mut Vec<PathBuf>| {
        if !p.is_file() { return; }
        let real = std::fs::canonicalize(&p).unwrap_or(p); // 解析 ~/.local/bin 软链到包内真实路径
        if !out.contains(&real) { out.push(real); }
    };
    for name in core_exe_names().iter().copied() {
        if let Some(p) = env::find_in_path(name) { add(p, &mut out); }
    }
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            for name in core_exe_names().iter().copied() {
                add(PathBuf::from(&appdata).join("npm").join(name), &mut out);
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        for name in core_exe_names().iter().copied() {
            add(PathBuf::from("/opt/homebrew/bin").join(name), &mut out);
            add(PathBuf::from("/usr/local/bin").join(name), &mut out);
        }
    }
    for name in core_exe_names().iter().copied() {
        add(home.join(".npm-global").join("bin").join(name), &mut out);
        add(home.join(".local").join("bin").join(name), &mut out);
    }
    if let Ok(res) = app.path().resource_dir() {
        for name in core_exe_names().iter().copied() { add(res.join("bin").join(name), &mut out); }
    }
    out
}

/// 定位已安装内核：多候选**按版本最高**仲裁（K5 修复）——旧内核不得遮蔽新内核。
fn locate_core(app: &tauri::AppHandle) -> Option<PathBuf> {
    let cands = locate_core_candidates(app);
    if cands.is_empty() { return None; }
    let mut best: Option<(PathBuf, String)> = None;
    for c in &cands {
        let v = core::installed_version(c).unwrap_or_else(|| "0.0.0".into());
        let better = best.as_ref().map(|(_, bv)| core::semver_cmp(&v, bv) > 0).unwrap_or(true);
        if better { best = Some((c.clone(), v)); }
    }
    best.map(|(p, _)| p).or_else(|| cands.into_iter().next())
}

/// 引导页查询用：内核是否已安装 + 当前版本 + 真实包名提示（不再硬编码平台字符串）。
#[tauri::command]
fn core_status(app: tauri::AppHandle) -> serde_json::Value {
    let bin = locate_core(&app);
    let version = bin.as_ref().and_then(|b| core::installed_version(b));
    let pkg = core::package_name().unwrap_or_else(|_| "@dsh-sup/dsh-core-<platform>".into());
    serde_json::json!({
        "installed": bin.is_some(),
        "version": version,
        "path": bin.map(|p| p.display().to_string()),
        "package": pkg,
        "hint": format!("npm i -g {}", pkg),
    })
}

/// 内核版本规划（引导页决策输入）：本地已装版本 + 远端最高版本 + 动作(install/upgrade/none)。
/// 网络调用放线程池，不阻塞 UI 线程。
#[tauri::command]
async fn core_plan(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let installed = locate_core(&app).and_then(|b| core::installed_version(&b));
    let pkg = core::package_name()?;
    let latest = tauri::async_runtime::spawn_blocking(move || core::latest_version(&pkg))
        .await.map_err(|e| e.to_string())?;
    Ok(core::build_plan(installed, latest))
}

/// 安装/升级/回退内核到指定版本（version=None → 最新）。方案 B 的执行口：
///   - 显式 `--prefix`（从现有内核路径反推）确保装回同一前缀（跨平台布局差异见 core.rs）；
///   - 逐个镜像回退；
///   - 如实回传成败（含退出码/stderr）——供引导页做「升级失败→回退旧版」判断，绝不吞错。
#[tauri::command]
async fn core_apply(app: tauri::AppHandle, version: Option<String>) -> Result<serde_json::Value, String> {
    let pkg = core::package_name()?;
    let prefix = locate_core(&app).and_then(|b| core::global_prefix_for(&b));
    let origins = core::registry_origins();
    let target = match version {
        Some(v) if core::is_valid_version(&v) => v,
        _ => {
            let p = pkg.clone();
            let r = tauri::async_runtime::spawn_blocking(move || core::latest_version(&p))
                .await.map_err(|e| e.to_string())?;
            match r {
                Ok((v, _)) => v,
                Err(e) => return Ok(serde_json::json!({"ok": false, "stage": "resolve", "error": e})),
            }
        }
    };
    let pkg2 = pkg.clone();
    let target2 = target.clone();
    let pref = prefix.clone();
    let res = tauri::async_runtime::spawn_blocking(move || {
        let mut last = String::from("无可用镜像");
        for o in &origins {
            match core::install_version(&pkg2, &target2, pref.as_deref(), Some(o.as_str())) {
                Ok(out) => return Ok::<(String, String), String>((o.clone(), out)),
                Err(e) => last = e,
            }
        }
        Err(last)
    }).await.map_err(|e| e.to_string())?;
    Ok(match res {
        Ok((origin, out)) => serde_json::json!({
            "ok": true, "version": target, "origin": origin, "output": out,
            "prefix": prefix.map(|p| p.display().to_string()),
        }),
        Err(e) => serde_json::json!({"ok": false, "version": target, "error": e}),
    })
}

/// 引导页驱动：申请所有者启动守卫（唯一启停权威，见 start_guard_service）。阻塞放线程池。
#[tauri::command]
async fn guard_start(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let a = app.clone();
    let r = tauri::async_runtime::spawn_blocking(move || ensure_guard(&a)).await.map_err(|e| e.to_string())?;
    Ok(match r {
        Ok(()) => serde_json::json!({"ok": true}),
        Err(e) => serde_json::json!({"ok": false, "error": e}),
    })
}

/// 守卫就绪探针（TCP + HTTP /healthz 双确认）：引导页据此决定进面板，替代固定延时（K6）。
#[tauri::command]
fn guard_ready() -> serde_json::Value {
    let port = env::api_port();
    if !is_alive(port) { return serde_json::json!({"ready": false, "reason": "tcp", "port": port}); }
    match http_get_local(port, "/healthz", std::time::Duration::from_secs(3)) {
        Some((code, _)) if (200..300).contains(&code) => serde_json::json!({"ready": true, "port": port}),
        Some((code, _)) => serde_json::json!({"ready": false, "reason": "http", "status": code, "port": port}),
        None => serde_json::json!({"ready": false, "reason": "http", "port": port}),
    }
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

/// 请求守卫的「生命周期所有者」启动守卫（契约 ARCHITECTURE-CONTRACT-phase0 §2.3）：
///   Linux=systemd --user；macOS=launchctl kickstart；Windows=schtasks。
/// 壳**绝不直接 spawn 守卫进程**——那会产生游离于服务管理器的第二实例（身份漂移，
/// 且使「停止守卫」无唯一权威）。壳只请求所有者启动。
#[cfg(target_os = "linux")]
fn start_guard_service() -> Result<(), String> {
    let out = std::process::Command::new("systemctl")
        .args(["--user", "start", "dsh-supervisor"])
        .output()
        .map_err(|e| format!("无法调用 systemctl: {}", e))?;
    if out.status.success() { Ok(()) } else {
        Err(format!("systemctl --user start dsh-supervisor 失败: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}
#[cfg(target_os = "macos")]
fn start_guard_service() -> Result<(), String> {
    let out = std::process::Command::new("sh")
        .args(["-c", "launchctl kickstart -k gui/$(id -u)/com.dsh.supervisor"])
        .output()
        .map_err(|e| format!("无法调用 launchctl: {}", e))?;
    if out.status.success() { Ok(()) } else {
        Err(format!("launchctl kickstart 失败: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}
#[cfg(target_os = "windows")]
fn start_guard_service() -> Result<(), String> {
    let out = std::process::Command::new("schtasks")
        .args(["/Run", "/TN", "DSH-Supervisor"])
        .output()
        .map_err(|e| format!("无法调用 schtasks: {}", e))?;
    if out.status.success() { Ok(()) } else {
        Err(format!("schtasks /Run 失败: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn start_guard_service() -> Result<(), String> { Err("当前平台不支持守卫服务管理".into()) }

/// 请求守卫的所有者停止守卫（契约 §4.1 退出时序的最后一步——守卫自身不再停自己）。
#[cfg(target_os = "linux")]
fn stop_guard_service() -> Result<(), String> {
    let out = std::process::Command::new("systemctl")
        .args(["--user", "stop", "dsh-supervisor"])
        .output()
        .map_err(|e| format!("无法调用 systemctl: {}", e))?;
    if out.status.success() { Ok(()) } else {
        Err(format!("systemctl --user stop 失败: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}
#[cfg(target_os = "macos")]
fn stop_guard_service() -> Result<(), String> {
    let out = std::process::Command::new("sh")
        .args(["-c", "launchctl bootout gui/$(id -u)/com.dsh.supervisor"])
        .output()
        .map_err(|e| format!("无法调用 launchctl: {}", e))?;
    if out.status.success() { Ok(()) } else {
        Err(format!("launchctl bootout 失败: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}
#[cfg(target_os = "windows")]
fn stop_guard_service() -> Result<(), String> {
    // Windows：先停 watchdog 保活任务，再终止守卫进程（否则 watchdog 会立刻重新拉起）。
    let _ = std::process::Command::new("schtasks").args(["/End", "/TN", "DSH-Supervisor-Watchdog"]).output();
    let _ = std::process::Command::new("schtasks").args(["/End", "/TN", "DSH-Supervisor"]).output();
    let _ = std::process::Command::new("taskkill").args(["/F", "/IM", "dsh-supervisor.exe"]).output();
    Ok(())
}
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn stop_guard_service() -> Result<(), String> { Err("当前平台不支持守卫服务管理".into()) }

/// 退出管家（契约 §4.1 冻结时序）：请求内核停全部被管对象（同步等待回执）→ 由所有者停止守卫。
/// 内核在回执前**不会**停止自己（阶段 1 已移除内核自停）。
fn shutdown_all(port: u16) {
    // 契约 §4.1 退出握手（阶段 3 增强）：
    //   1) 带超时请求内核停全部被管对象，并等待回执（防止守卫挂起时壳无限阻塞）；
    //   2) 轮询 sessionState 直到 stopped（确认内核确实停好；守卫已不可达同样视为完成）；
    //   3) 由所有者停止守卫进程——守卫自身从不停止自己（阶段 1 所有权归一）。
    let _ = post_local_timeout(port, "/session/stop", std::time::Duration::from_secs(60));
    for _ in 0..40 {
        match get_session_state(port) {
            Some(s) if s == "stopped" => break, // 内核已确认停链完成
            None => break,                      // 守卫已不可达 = 已退出
            _ => std::thread::sleep(std::time::Duration::from_millis(250)),
        }
    }
    if let Err(e) = stop_guard_service() {
        eprintln!("[shell] 停止守卫失败: {}（可手动 systemctl --user stop dsh-supervisor）", e);
    }
}

/// 拉起守卫（定位已安装内核 + 请求所有者启动），等面板就绪。
/// 端口从用户 config.apiPort 解析（非硬编码 3100）。
fn ensure_guard(app: &tauri::AppHandle) -> Result<(), String> {
    let port = env::api_port();
    if is_alive(port) { return Ok(()); }
    let _ = locate_core(app).ok_or_else(|| match core::package_name() {
        Ok(p) => format!("未检测到内核。请先安装：npm i -g {}", p),
        Err(e) => format!("未检测到内核：{}", e),
    })?;
    start_guard_service()?;
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
fn go_panel(app: &tauri::AppHandle, force: bool) {
    let url = env::api_base_url(); // http://127.0.0.1:<config.apiPort 或高位段 fallback>/
    // 壳框架(shell.html)的 evt listener 在首帧注册；setup 线程的 emit 可能早于注册被丢弃，
    // 故延时重发数次覆盖竞态（listener 就绪后任一次生效即切面板）。
    // force=true（如用户重新显示窗口）→ 即使 URL 相同也强制重载，保证拿到最新 UI。
    for (i, delay_ms) in [400u64, 1200, 2500].iter().enumerate() {
        let h = app.clone();
        let u = url.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(*delay_ms));
            let _ = h.emit("shell:goto-panel", serde_json::json!({ "url": u, "seq": i, "force": force }));
        });
    }
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
        // 每次显示都强制重新导航到面板：WebView 不做陈旧缓存，保证拿最新 UI（force=true）
        go_panel(app, true);
    }
}

fn post_local(port: u16, path: &str) {
    let _ = post_local_timeout(port, path, std::time::Duration::from_secs(60));
}

/// 同 post_local，但带读写超时（防止守卫挂起时壳无限阻塞）。返回响应体（解码 utf8 尽力）。
fn post_local_timeout(port: u16, path: &str, timeout: std::time::Duration) -> Option<String> {
    let addr = format!("127.0.0.1:{}", port);
    let mut stream = TcpStream::connect(addr).ok()?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let req = format!(
        "POST {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        path, port
    );
    std::io::Write::write_all(&mut stream, req.as_bytes()).ok()?;
    let mut buf = Vec::new();
    let _ = std::io::Read::read_to_end(&mut stream, &mut buf);
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// 本地 HTTP GET（返回 (状态码, 全文)）——守卫就绪探针用。
fn http_get_local(port: u16, path: &str, timeout: std::time::Duration) -> Option<(u16, String)> {
    let addr = format!("127.0.0.1:{}", port);
    let mut stream = TcpStream::connect(addr).ok()?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let req = format!("GET {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n", path, port);
    std::io::Write::write_all(&mut stream, req.as_bytes()).ok()?;
    let mut buf = Vec::new();
    let _ = std::io::Read::read_to_end(&mut stream, &mut buf);
    let s = String::from_utf8_lossy(&buf).into_owned();
    let code = s.split_whitespace().nth(1).and_then(|c| c.parse::<u16>().ok()).unwrap_or(0);
    Some((code, s))
}

/// 读取会话态（GET /session/status 的最小解析：找 "sessionState":"xxx"）。
fn get_session_state(port: u16) -> Option<String> {
    let addr = format!("127.0.0.1:{}", port);
    let mut stream = TcpStream::connect(addr).ok()?;
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(3)));
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(3)));
    let req = format!("GET /session/status HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n", port);
    std::io::Write::write_all(&mut stream, req.as_bytes()).ok()?;
    let mut buf = Vec::new();
    let _ = std::io::Read::read_to_end(&mut stream, &mut buf);
    let s = String::from_utf8_lossy(&buf);
    let key = "\"sessionState\":\"";
    let i = s.find(key)? + key.len();
    let rest = &s[i..];
    let v: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    if v.is_empty() { None } else { Some(v) }
}

// ═══════════════════════════════════════════════════════════════════
// 桌面壳自更新命令（2026-09-11）
//
// 三平台**同一代码路径**：检查 → 下载 → minisign 验签 → 平台安装 → 重启。
// 平台差异（Linux pkexec dpkg -i / macOS .app 替换 / Windows NSIS passive）
// 全部由 tauri-plugin-updater 内部处理，壳侧无平台分支。
// ═══════════════════════════════════════════════════════════════════

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

/// 壳身份快照（版本/安装形态/自更新能力/attempt/pinned），供引导页与诊断。
#[tauri::command]
fn shell_identity(app: tauri::AppHandle) -> serde_json::Value {
    let v = app.package_info().version.to_string();
    let mut id = update::identity_snapshot();
    if id.get("version").is_none() {
        id = update::init_identity(&v);
    }
    id
}

/// 引导阶段上报（写入 identity.json + shell.log，便于问题定位）。
#[tauri::command]
fn shell_set_phase(phase: String) {
    update::set_phase(&phase);
}

// ── 超时预算（防「无超时网络请求 → 引导页永久卡住」）──
// 事故背景（2026-09-11 Windows 真机实测）：用户装完桌面壳后，引导第一步就是壳更新，
// 而 `check()` 在无超时的情况下遇到网络不可达**永不返回** → 前端 Promise 既不 resolve
// 也不 reject → `.catch` 不触发 → 永久卡在「正在检查桌面更新」，用户无法进入产品。
// 故：check 必须短超时（快速失败），下载必须长超时（大安装包 + 慢网），且外层再加 tokio
// 兜底（reqwest 的 request timeout 不保证覆盖 DNS 等阶段）。
const SHELL_CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
const SHELL_DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20 * 60);

/// 构造带超时的更新器。
/// ⚠ 该超时是 reqwest 的**整个请求**超时：check 用短超时；下载必须用长超时，
///   否则大安装包会在传输中途被切断。
fn shell_updater(
    app: &tauri::AppHandle,
    timeout: std::time::Duration,
) -> Result<tauri_plugin_updater::Updater, String> {
    app.updater_builder()
        .timeout(timeout)
        .build()
        .map_err(|e| format!("更新器不可用: {}", e))
}

/// 检查是否有壳更新。
/// 返回 { ok, available, current, latest, notes, skipped?, reason? }
/// 语义：跳过的原因一律**不阻断启动**（有界失败即放行）。
#[tauri::command]
async fn shell_update_check(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let cur = app.package_info().version.to_string();
    let (should, reason) = update::should_check(&cur);
    if !should {
        update::log(&format!("桌面更新跳过：{}", reason));
        return Ok(serde_json::json!({
            "ok": true, "available": false, "skipped": true, "reason": reason, "current": cur
        }));
    }
    update::set_phase("shell-update-check");
    let updater = match shell_updater(&app, SHELL_CHECK_TIMEOUT) {
        Ok(u) => u,
        Err(msg) => {
            update::log(&format!("桌面更新检查失败：{}", msg));
            return Ok(serde_json::json!({ "ok": false, "available": false, "error": msg, "current": cur }));
        }
    };
    // 双保险：reqwest 的 request timeout 不保证覆盖所有阶段（如 DNS），外层再包一层 tokio 超时。
    let checked = tokio::time::timeout(
        SHELL_CHECK_TIMEOUT + std::time::Duration::from_secs(5),
        updater.check(),
    )
    .await;
    match checked {
        Err(_) => {
            let msg = format!("检查超时（{} 秒无响应，可能网络不可达）", SHELL_CHECK_TIMEOUT.as_secs());
            update::log("桌面更新检查超时（网络不可达？）");
            Ok(serde_json::json!({ "ok": false, "available": false, "error": msg, "current": cur }))
        }
        Ok(Ok(Some(u))) => {
            let latest = u.version.clone();
            update::log(&format!("桌面更新发现新版本 {}（当前 {}）", latest, cur));
            Ok(serde_json::json!({
                "ok": true,
                "available": true,
                "current": cur,
                "latest": latest,
                "notes": u.body.clone().unwrap_or_default(),
                "date": u.date.map(|d| d.to_string()).unwrap_or_default(),
            }))
        }
        Ok(Ok(None)) => {
            update::log(&format!("桌面更新已是最新（{}）", cur));
            Ok(serde_json::json!({ "ok": true, "available": false, "current": cur }))
        }
        Ok(Err(e)) => {
            // 网络失败/清单不可达/验签失败 → 一律「失败放行」，由引导页决定是否重试
            let msg = format!("{}", e);
            update::log(&format!("桌面更新检查失败：{}", msg));
            Ok(serde_json::json!({ "ok": false, "available": false, "error": msg, "current": cur }))
        }
    }
}

/// 下载并安装更新（minisign 验签在插件内强制执行）。
/// 成功后**不自动重启**——由引导页统一调用 shell_restart（便于先告知用户）。
#[tauri::command]
async fn shell_update_apply(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    update::set_phase("shell-update-download");
    // 下载需要长超时；但 check 不能等那么久 → check 单独用 tokio 包短超时。
    let updater = shell_updater(&app, SHELL_DOWNLOAD_TIMEOUT)?;
    let found = match tokio::time::timeout(SHELL_CHECK_TIMEOUT, updater.check()).await {
        Err(_) => {
            return Ok(serde_json::json!({ "ok": false, "error": "检查超时（可能网络不可达）" }));
        }
        Ok(Err(e)) => {
            return Ok(serde_json::json!({ "ok": false, "error": format!("检查失败: {}", e) }));
        }
        Ok(Ok(f)) => f,
    };
    let Some(u) = found else {
        return Ok(serde_json::json!({ "ok": true, "upToDate": true }));
    };
    let target = u.version.clone();

    // ⚠ mark_pending 必须在 install **之前**（2026-09-11 修复）。
    //   Windows 上 install 会 ShellExecuteW 启动安装程序后立即 std::process::exit(0)，
    //   其后的任何代码都不会执行 —— 原先把它放在 download_and_install 之后，
    //   导致 Windows 的护栏账本**永远拿不到 pendingVersion**，
    //   「更新成功确认 / 连续失败拉黑」机制在 Windows 上完全失效。
    update::mark_pending(&target);
    update::log(&format!("桌面更新开始下载 {}", target));

    // 下载：用**进度事件**驱动前端进度条。
    // （此前进度回调体是空的 `let _ = (chunk, total);` → 用户无法区分「正在下载」与
    //   「卡死」；4MB+ 安装包在慢网下长时间零反馈，Windows 真机实测体验极差。）
    // 注意：插件的 on_chunk 回调给的是**本块大小**（增量），故此处自行累加。
    let mut got: u64 = 0;
    let dl = tokio::time::timeout(
        SHELL_DOWNLOAD_TIMEOUT,
        u.download(
            |chunk, total| {
                got += chunk as u64;
                let _ = app.emit(
                    "shell_update_progress",
                    serde_json::json!({ "downloaded": got, "total": total }),
                );
            },
            || {},
        ),
    )
    .await;
    let bytes = match dl {
        Err(_) => {
            let msg = format!("下载超时（{} 分钟未完成）", SHELL_DOWNLOAD_TIMEOUT.as_secs() / 60);
            update::log(&format!("桌面更新{}", msg));
            return Ok(serde_json::json!({ "ok": false, "error": msg }));
        }
        Ok(Err(e)) => {
            let msg = format!("下载失败: {}", e);
            update::log(&format!("桌面更新{}", msg));
            return Ok(serde_json::json!({ "ok": false, "error": msg }));
        }
        Ok(Ok(b)) => b,
    };

    let total = bytes.len() as u64;
    let _ = app.emit(
        "shell_update_progress",
        serde_json::json!({ "downloaded": total, "total": total, "installing": true }),
    );
    update::log(&format!("桌面更新下载完成（{} 字节），开始安装 {}", total, target));

    // ⚠ Windows：install 启动安装程序后 std::process::exit(0)，**不会返回**；
    //   Linux/macOS：返回后由引导页调用 shell_restart 重启进入新版本。
    if let Err(e) = u.install(bytes) {
        let msg = format!("安装失败: {}", e);
        update::log(&format!("桌面更新{}", msg));
        return Ok(serde_json::json!({ "ok": false, "error": msg }));
    }
    update::log(&format!("桌面更新安装完成 {}", target));
    Ok(serde_json::json!({ "ok": true, "installed": target }))
}

/// 重启进入新版本（旧进程装、新进程跑）。
#[tauri::command]
fn shell_restart(app: tauri::AppHandle) {
    update::set_phase("restarting");
    update::log("桌面更新重启以应用新版本");
    app.restart();
}

fn main() {
    // 无头冒烟入口：--node-plan 仅打印环境探针 + 官方最新 LTS，不启动窗口。
    if std::env::args().any(|a| a == "--node-plan") {
        std::process::exit(cli_plan());
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
    tauri::Builder::default()
        // 单实例管控（2026-09）：同一 user 会话内只允许一个壳实例——重复启动第二实例时
        // 插件自动让新进程退出，回调里唤起既有主窗口（show+focus+导航面板），避免双壳/多壳并存。
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app);
        }))
        // 壳自更新插件：强制 minisign 验签；平台安装语义内部处理。
        // 未配置 pubkey 时插件仍可注册（check 会失败并返回错误，由引导页按「失败放行」处理）。
        .plugin(tauri_plugin_updater::Builder::new().build())
        // app.restart()：更新安装后重启进入新版本（旧进程装、新进程跑）。
        .plugin(tauri_plugin_process::init())
        .manage(Mutex::new(RunState::default()))
        .invoke_handler(tauri::generate_handler![node_status, core_status, core_plan, core_apply, guard_start, guard_ready, start_node_install, finish_boot, win_ctl, shell_identity, shell_update_check, shell_update_apply, shell_restart, shell_set_phase])
        .setup(|app| {
            // 托盘直发本地 API 的端口：显式 DSH_SUPERVISOR_TRAY_PORT 优先，否则从用户 config.apiPort 解析
            let port: u16 = std::env::var("DSH_SUPERVISOR_TRAY_PORT")
                .ok().and_then(|p| p.parse().ok()).unwrap_or_else(|| env::api_port());
            let handle = app.handle().clone();

            // 壳身份初始化（2026-09-11）：写 ~/.dsh/shell/identity.json + shell.log，
            // 并推进「更新护栏」状态（pendingVersion 是否生效 / attempt 计数 / 拉黑）。
            // 必须尽量早执行：即使后续任一环节失败，也留下可诊断的落盘痕迹。
            let _ = update::init_identity(&app.package_info().version.to_string());

            // 环境判定（2026-09 改）：Node 缺失或低于最低标准(>=22.12, DSH commander 硬门槛) → 引导页安装；
            // 达标（即使不是最新 LTS）→ 直接拉起守卫进入面板，不卡升级。初始 url 即 bootstrap.html。
            let have = env::probe_system_node();
            {
                let st = handle.state::<Mutex<RunState>>();
                if let Some((_p, v)) = have.as_ref() { st.lock().unwrap().installed = Some(v.clone()); }
            }
            // 单一引导流程（K3 修复）：壳启动只做「环境信息探测」，**不再并行拉起守卫**。
            // 守卫的安装/升级/启动全部由引导页显式驱动
            // （core_plan → core_apply → guard_start → guard_ready），杜绝两条互不知晓的流程竞争，
            // 以及「Rust 侧已尝试拉起但页面不知情」的假成功。
            std::thread::spawn(move || {
                if let Ok((v, _f)) = node::latest_lts() {
                    let state = handle.state::<Mutex<RunState>>();
                    let mut s = state.lock().unwrap();
                    s.latest = Some(v.clone());
                    drop(s);
                    let _ = handle.emit("env_status", serde_json::json!({ "latest": v }));
                }
            });

            // ── 托盘 ──
            let show_m = tauri::menu::MenuItem::with_id(app, "show", "显示面板", true, None::<&str>)?;
            let start = tauri::menu::MenuItem::with_id(app, "start", "启动 DSH", true, None::<&str>)?;
            let stop = tauri::menu::MenuItem::with_id(app, "stop", "停止 DSH", true, None::<&str>)?;
            let restart = tauri::menu::MenuItem::with_id(app, "restart", "重启一次", true, None::<&str>)?;
            let quit = tauri::menu::MenuItem::with_id(app, "quit", "退出管家", true, None::<&str>)?;
            let menu = tauri::menu::Menu::with_items(app, &[&show_m, &start, &stop, &restart, &quit])?;

            tauri::tray::TrayIconBuilder::with_id("dsh-supervisor-tray")
                .icon(app.default_window_icon().expect("no default icon").clone())
                .tooltip("dsh-supervisor")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(move |app, event| {
                    match event.id.as_ref() {
                        "show" => show_main(app),
                        // 归一化：启停唯一入口 /lifecycle/dsh/*（旧 /start|/stop|/restart 已删，2026-09）
                        "start" => post_local(port, "/lifecycle/dsh/start"),
                        "stop" => post_local(port, "/lifecycle/dsh/stop"),
                        "restart" => post_local(port, "/lifecycle/dsh/restart"),
                        // 退出管家 = 完全退出：通知守卫停止全部服务链，随后壳退出
                        "quit" => {
                            // 契约 §4.1：请求内核停被管对象（等回执）→ 由所有者停止守卫 → 壳退出
                            shutdown_all(port);
                            app.exit(0);
                        }
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
                // 关闭窗口行为（读守卫 config.closeAction，系统级开关 2026-09）：
                // 'exit' = 退出管家（通知守卫停止全部服务链 + 壳退出）；默认 'hide' = 隐藏至托盘常驻。
                if env::close_action() == "exit" {
                    let app = window.app_handle();
                    let port = env::api_port();
                    // 契约 §4.1：停被管对象（等回执）→ 由所有者停止守卫 → 壳退出
                    shutdown_all(port);
                    let _ = app.exit(0);
                    return;
                }
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running dsh-supervisor-gui");
}
