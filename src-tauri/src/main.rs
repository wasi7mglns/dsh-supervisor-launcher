#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// dsh-supervisor-gui：桌面壳 + 环境引导器（开源：dsh-supervisor-launcher）。
// 职责：
//   1. 探测系统 Node.js；缺失/过旧 → 内嵌引导页 → 一键安装官方最新 LTS（下载/校验/授权）。
//   2. Node 就绪 → 定位已安装内核（dsh-supervisor SEA 二进制，npm 子包 / ~/.local/bin / PATH）
//      → 拉起守卫 daemon → 面板 127.0.0.1:3100。内核闭源（npm 安装），壳不内嵌任何内核资产。
// 托盘常驻：关窗 = 隐藏；菜单动作直发本地 API（裸 TCP，无额外依赖）。

// 有界子进程执行（公共设施）：所有外部命令一律经它，避免「无界阻塞分散潜伏」。
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
mod platform;
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

/// 环境状态查询（**纯本地、有界、可轮询**）。
///
/// == 架构修复（2026-09-11 二次，根因见 nodeprobe.rs） ==
///
/// 1) **必须快速返回**，无论探测内部是否卡在阻塞型系统调用。旧实现直接
///    await 阻塞任务 —— 探测一挂，本命令就永不返回，前端 45 秒超时后只能报
///    node=unknown，且 Rust 侧那条线程永久悬挂（每次重试再添一条）。
///    现改为：探测在分离线程中进行，本命令只在极短预算内等待；未完成即返回
///    probing=true，由引导页轮询。**命令的返回时间与任何系统调用无关。**
///
/// 2) **不含任何网络 I/O**。旧实现会顺带触网查最新 LTS，使「本地环境判定」
///    被网络质量左右 —— 而这两件事在因果上毫无关系。网络侧信息改由 node_latest
///    单独提供（也便于把镜像选择显式呈现给用户）。
#[tauri::command]
async fn node_status(app: tauri::AppHandle) -> serde_json::Value {
    // 首次调用会启动探测线程；此后每次调用都复用在飞结果（不会堆积线程）。
    let budget = std::time::Duration::from_millis(900);
    let out = match tauri::async_runtime::spawn_blocking(move || nodeprobe::status(budget)).await {
        Ok(o) => o,
        Err(_) => nodeprobe::partial(),
    };

    let mut o = {
        let state = app.state::<Mutex<RunState>>();
        let st = state.lock().unwrap_or_else(|e| e.into_inner());
        let installed = out.version.clone().or_else(|| st.installed.clone());
        let mut o = log(&st);
        o["installed"] = serde_json::json!(installed);
        // DSH 最低门槛判定（>=22.12）：引导页据此决定是否需装 Node
        o["minOk"] = serde_json::json!(node::meets_minimum(installed.as_deref()));
        o["minRequired"] = serde_json::json!(node::MIN_NODE);
        if let Some(latest) = &st.latest {
            o["outdated"] = serde_json::json!(node::outdated(installed.as_deref(), latest));
        }
        o
    };
    o["probing"] = serde_json::json!(!out.finished);
    // 明确失败原因（到硬上限 / worker 异常）。前端据此立即给出可操作结论，
    // 而非等自己的预算耗尽后只报一句「超时」。
    o["probeError"] = match &out.error {
        Some(e) => serde_json::json!(e),
        None => serde_json::Value::Null,
    };
    o["nodePath"] = serde_json::json!(out.path.as_ref().map(|p| p.display().to_string()));
    o["elapsedMs"] = serde_json::json!(out.elapsed_ms);
    o["candidates"] = serde_json::json!(nodeprobe::candidate_summary());
    // 「当前卡在哪个候选多久」——环境特有问题无法靠读代码确定，必须靠这份追踪。
    o["stuck"] = match nodeprobe::current_stuck() {
        Some((d, ms)) => serde_json::json!({ "on": d, "ms": ms }),
        None => serde_json::Value::Null,
    };
    o["trace"] = serde_json::json!(out
        .trace
        .iter()
        .map(|t| serde_json::json!({
            "source": t.source, "path": t.path, "ms": t.ms, "ok": t.ok, "note": t.note
        }))
        .collect::<Vec<_>>());
    o
}

/// 预热镜像测速（立即返回，结果稍后经 mirror_cached 读取）。
///
/// 为什么单独成命令：镜像信息必须**与「是否需要下载 Node」解耦** ——
/// 否则 Node 已达标的用户（主力用户）永远看不到壳选了哪个源。
#[tauri::command]
fn mirror_warmup() -> serde_json::Value {
    mirror::warmup_async();
    serde_json::json!({ "ok": true })
}

/// 读镜像预热缓存（**无网络 I/O**，任何时刻可安全轮询）。
#[tauri::command]
fn mirror_cached() -> serde_json::Value {
    match mirror::cached() {
        Some(s) => serde_json::json!({
            "ready": true,
            "nodeBest": s.node_best,
            "nodeLatencyMs": s.node_latency_ms,
            "npmBest": s.npm_best,
            "npmLatencyMs": s.npm_latency_ms,
            "npmProbes": s.npm_probes.iter().map(|(src, ok, ms)| serde_json::json!({
                "source": src, "ok": ok, "latencyMs": ms
            })).collect::<Vec<_>>(),
            "at": s.at,
        }),
        None => serde_json::json!({ "ready": false }),
    }
}

/// 网络侧元数据：最新 LTS + **镜像选择结果**（与本地环境检测彻底分离）。
#[tauri::command]
async fn node_latest() -> serde_json::Value {
    match tauri::async_runtime::spawn_blocking(node::latest_lts).await {
        Ok(Ok(c)) => serde_json::json!({
            "ok": true,
            "version": c.version,
            "file": c.file,
            "mirror": c.source,
            "latencyMs": c.latency_ms,
            "probes": c.probes.iter().map(|(s, ok, ms)| serde_json::json!({
                "source": s, "ok": ok, "latencyMs": ms
            })).collect::<Vec<_>>(),
        }),
        Ok(Err(e)) => serde_json::json!({ "ok": false, "error": e }),
        Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
    }
}


/// 引导页走完所有检测步骤后调用：进入面板（go_panel —— emit 守卫 URL，壳框架 iframe 切换）。
/// 由引导页 JS 在展示完整启动过程后触发，避免步骤一闪而过无感知。
#[tauri::command]
fn finish_boot(app: tauri::AppHandle) -> Result<(), String> {
    go_panel(&app, false); // 引导完成：URL 变化即导航
    Ok(())
}

/// 互斥锁中毒恢复的**统一约定**（2026-09-11 架构修复）：
///
/// 全部 `RunState` 加锁点使用 `.unwrap_or_else(|e| e.into_inner())` 而非 `.unwrap()`。
/// 原实现一旦有任何线程在持锁期间 panic，该锁**永久中毒**，此后**所有**命令
/// 都在加锁处 panic —— 用户看到的是「重启也没用、功能永久失效」。
/// 锁内是普通状态快照（不承载跨字段不变式），中毒后仍可用，故取回内部值继续。
///
/// 原则：**一次 panic 不应让整个应用的功能不可恢复地失效。**
#[tauri::command]
fn start_node_install(state: tauri::State<Mutex<RunState>>, app: tauri::AppHandle) -> Result<(), String> {
    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
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
        let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
        s.busy = false;
        match out {
            Ok((node_path, version)) => {
                s.installed = Some(version.clone());
                s.progress = 1.0;
                s.status = format!("Node.js {} 就绪，正在启动守卫…", version);
                s.logs.push(format!("安装完成: {} @ {}", version, node_path));
                node::record_runtime_meta(&node_path, &version);
                // 探测缓存必须失效：新装的 Node 只有重新探测才会被发现
                // （否则引导页会在「已装好」之后仍报未检测到）。
                nodeprobe::invalidate();
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

fn core_exe_names() -> &'static [&'static str] {
    if cfg!(windows) { &["dsh-supervisor.exe", "dsh-supervisor.cmd", "dsh-supervisor"] }
    else { &["dsh-supervisor"] }
}

/// 收集全部内核候选（去重 + 解析符号链接），供「按版本最高仲裁」使用。
/// 跨平台路径规范：
///   - PATH（env::find_in_path，Windows 走 PATHEXT）
///   - Windows: %APPDATA%\npm（npm 全局 bin 目录）+ 包内真实脚本
///   - macOS:   /opt/homebrew/bin（Apple Silicon）、/usr/local/bin（Intel）
///   - Unix:    ~/.npm-global/bin、~/.local/bin（内核 install 写入的软链）
///   - 资源目录内嵌兜底（旧版过渡）
///
/// `resource_dir` 为 None 时跳过「资源目录内嵌兜底」——CLI 自检（无 AppHandle）走这条。
fn locate_core_candidates(resource_dir: Option<PathBuf>) -> Vec<PathBuf> {
    let home = env::home();
    let mut out: Vec<PathBuf> = Vec::new();
    // ⚠ 与 env.rs 的 PATH 探测同一类防护（2026-09-11 架构修复）：
    //   is_file() / canonicalize() 底层会触网 —— 在断开的映射盘或 UNC 路径上
    //   可能阻塞数十秒，而本函数在**内核定位的关键路径**上（引导页每一步都要用）。
    //   故先做「本地固定盘」判定（GetDriveTypeW 自身不触网），再访问文件系统。
    let add = |p: PathBuf, out: &mut Vec<PathBuf>| {
        if let Some(dir) = p.parent() {
            if !env::is_local_fixed_dir(dir) { return; }
        }
        if !p.is_file() { return; }
        let real = std::fs::canonicalize(&p).unwrap_or(p); // 解析 ~/.local/bin 软链到包内真实路径
        if !out.contains(&real) { out.push(real); }
    };
    for name in core_exe_names().iter().copied() {
        if let Some(p) = env::find_in_path(name) { add(p, &mut out); }
    }
    // 平台额外候选（Windows 的 %APPDATA%\npm 与包内真实脚本；macOS 的 Homebrew 落点）
    // —— 已下沉到 platform 层（2026-09-11），本文件不再出现平台分支。
    {
        let names: Vec<&str> = core_exe_names().iter().copied().collect();
        let pkg = core::package_name().ok();
        for p in crate::platform::current().core_extra_candidates(&names, pkg.as_deref()) {
            add(p, &mut out);
        }
    }
    for name in core_exe_names().iter().copied() {
        add(home.join(".npm-global").join("bin").join(name), &mut out);
        add(home.join(".local").join("bin").join(name), &mut out);
    }
    if let Some(res) = resource_dir {
        for name in core_exe_names().iter().copied() { add(res.join("bin").join(name), &mut out); }
    }
    out
}

/// 定位已安装内核：多候选**按版本最高**仲裁（K5 修复）——旧内核不得遮蔽新内核。
fn locate_core(app: &tauri::AppHandle) -> Option<PathBuf> {
    locate_core_with_version(app).map(|(p, _)| p)
}

/// 定位内核并**一并返回其版本**（避免调用方再执行一次二进制取版本）。
///
/// 仲裁规则（K5）：多候选中**按版本最高**选取 —— 旧内核不得遮蔽新内核。
/// 每个候选的版本探测都经有界执行器（10 秒上限），单个坏候选不会拖死定位。
fn locate_core_with_version(app: &tauri::AppHandle) -> Option<(PathBuf, String)> {
    let cands = locate_core_candidates(app.path().resource_dir().ok());
    if cands.is_empty() { return None; }
    let mut best: Option<(PathBuf, String)> = None;
    for c in &cands {
        let v = core::installed_version(c).unwrap_or_else(|| "0.0.0".into());
        let better = best.as_ref().map(|(_, bv)| core::semver_cmp(&v, bv) > 0).unwrap_or(true);
        if better { best = Some((c.clone(), v)); }
    }
    best.or_else(|| cands.into_iter().next().map(|p| (p, "0.0.0".into())))
}

/// 引导页查询用：内核是否已安装 + 当前版本 + 真实包名提示（不再硬编码平台字符串）。
///
/// 内核状态查询。
///
/// == 架构修复（2026-09-11）==
///
/// 1) **必须 async**：旧实现是同步命令 → Tauri 在**主线程**执行 →
///    `locate_core` 会**逐个候选执行内核二进制**（每个 10 秒上限）取版本做仲裁，
///    且随后又对选中项再执行一次取版本。候选一多（PATH + npm 目录 + 资源目录）
///    即可把主线程占住数十秒 —— 界面完全无响应。
///    现把全部工作放进阻塞线程池，主线程立即返回。
///
/// 2) **不重复执行**：`locate_core` 内部已按版本仲裁并取过版本，
///    旧实现在外面又调一次 `installed_version`，属纯浪费（每次都可能是一次进程启动）。
///    现让 `locate_core` 一并返回版本。
#[tauri::command]
async fn core_status(app: tauri::AppHandle) -> serde_json::Value {
    let pkg = core::package_name().unwrap_or_else(|_| "@dsh-sup/dsh-core-<platform>".into());
    let located = tauri::async_runtime::spawn_blocking(move || {
        let a = app.clone();
        locate_core_with_version(&a)
    })
    .await
    .ok()
    .flatten();
    let (installed, version, path) = match located {
        Some((p, v)) => (true, Some(v), Some(p.display().to_string())),
        None => (false, None, None),
    };
    serde_json::json!({
        "installed": installed,
        "version": version,
        "path": path,
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

/// 引导页驱动：申请所有者启动守卫（唯一启停权威，见 platform::service::start）。阻塞放线程池。
#[tauri::command]
async fn guard_start(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    // ⚠ 必须有界（2026-09-11 架构修复）。
    //
    // 旧实现：`spawn_blocking(ensure_guard).await` —— **无超时**。
    // 而 ensure_guard 内部会：① 调服务管理器（原为无界 .output()）
    // ② wait_alive 最多 30 秒 ③ 兜底 spawn 后再等 60 秒。
    // 任一处挂起 → 本命令永不返回 → 前端 invoke('guard_start') 永不 settle
    // → 引导页**永久停在「正在启动守卫…」**，无重试、无出口。
    // 这与「卡在检测环境」是**同一根因模式**，只是发生在另一个步骤上。
    //
    // 两层保证：① 内部各命令已全部有界；② 此处再加外层兜底。
    // 注：超时不会取消 spawn_blocking 中已启动的任务（它会自然结束），
    // 但**命令会返回**，前端因此能拿到结论并给出重试/诊断入口。
    const GUARD_TOTAL_BUDGET: std::time::Duration = std::time::Duration::from_secs(180);
    let a = app.clone();
    let task = tauri::async_runtime::spawn_blocking(move || ensure_guard(&a));
    let r = match tokio::time::timeout(GUARD_TOTAL_BUDGET, task).await {
        Ok(Ok(inner)) => inner,
        Ok(Err(e)) => Err(format!("守卫启动任务异常: {}", e)),
        Err(_) => Err(format!(
            "守卫启动超时（{} 秒未完成）。可能原因：服务管理器无响应，或守卫进程无法启动。请用 dsh-supervisor-gui --service-plan 查看服务定义状态。",
            GUARD_TOTAL_BUDGET.as_secs()
        )),
    };
    Ok(match r {
        Ok(()) => serde_json::json!({"ok": true}),
        Err(e) => serde_json::json!({"ok": false, "error": e}),
    })
}

/// 守卫就绪探针（TCP + HTTP /healthz 双确认）：引导页据此决定进面板，替代固定延时（K6）。
///
/// ⚠ 必须 async（2026-09-11 审计）：旧实现是**同步命令** → 在**主线程**执行一次
///   最多 400ms 的 TCP 探测 + 最多 3 秒的 HTTP 往返；而引导页每 500ms 轮询一次、
///   最多 40 次 —— 合计可占住主线程十几秒，界面在此期间**无法重绘**。
///   现放进阻塞线程池：主线程立即返回。
#[tauri::command]
async fn guard_ready() -> serde_json::Value {
    tauri::async_runtime::spawn_blocking(|| {
        let port = env::api_port();
        if !is_alive(port) { return serde_json::json!({"ready": false, "reason": "tcp", "port": port}); }
        match http_get_local(port, "/healthz", std::time::Duration::from_secs(3)) {
            Some((code, _)) if (200..300).contains(&code) => serde_json::json!({"ready": true, "port": port}),
            Some((code, _)) => serde_json::json!({"ready": false, "reason": "http", "status": code, "port": port}),
            None => serde_json::json!({"ready": false, "reason": "http", "port": port}),
        }
    })
    .await
    .unwrap_or_else(|_| serde_json::json!({"ready": false, "reason": "probe-panic"}))
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

// ── 守卫服务的「启停」已迁入 platform 层（2026-09-11）──
//
// 此处原定义 start_guard_service / stop_guard_service（含 8 份 #[cfg]），
// 与 service.rs 的「服务定义」（另 4 份 #[cfg]）分居两层 —— 同一概念的 per-OS
// 知识被切开，加一个平台要改两处**不同层**，且很容易只改一处。
//
// 现统一在 platform::service::ServiceControl：**定义与启停永远是同一个对象**。
//
// · 原 GUARD_CMD_TIMEOUT=20s 的有界性 → platform::SVC_NORMAL
// · 原「壳绝不直接 spawn 守卫」的政策 → platform::service::spawn_daemon 的文档
//   （该约束不凌驾于可用性：容器/无 user session/策略拦截等场景需 spawn 兜底）

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
    if let Err(e) = platform::service().stop() {
        eprintln!("[shell] 停止守卫失败: {}（可手动 systemctl --user stop dsh-supervisor）", e);
    }
}

/// 拉起守卫（定位已安装内核 + 请求所有者启动），等面板就绪。
/// 端口从用户 config.apiPort 解析（非硬编码 3100）。
fn ensure_guard(app: &tauri::AppHandle) -> Result<(), String> {
    let port = env::api_port();
    // 进度上报（2026-09-11 架构修复）：本函数最长可耗时约 2 分钟（服务管理器启动最多 30s，
    // 兜底 spawn 后再等 60s），而这段时间前端只有一句静态的「正在启动守卫…」——
    // **静默等待与卡死无法区分**，用户会误判为卡住并强杀。故每个阶段都上报。
    let step = |s: &str| {
        let _ = app.emit("guard_progress", serde_json::json!({ "status": s }));
        update::log(s);
    };
    if is_alive(port) { step("守卫已在运行"); return Ok(()); }

    let guard = locate_core(app).ok_or_else(|| match core::package_name() {
        Ok(p) => format!("未检测到内核。请先安装：npm i -g {}", p),
        Err(e) => format!("未检测到内核：{}", e),
    })?;

    // ① 建立服务定义（**首次安装的关键一步**）。
    //    旧实现直接跳到 start，而首启时服务定义根本不存在 -> 必然失败 -> 卡在「守卫就绪」。
    //    这一步是幂等的：已存在则直接返回。
    step("正在建立守卫服务定义…");
    match platform::service().ensure_defined(&guard) {
        Ok(desc) => update::log(&format!("守卫服务定义: {}", desc)),
        Err(e) => update::log(&format!("守卫服务定义失败（稍后走 spawn 兜底）: {}", e)),
    }

    // ② 请求服务管理器启动（正常路径：由 systemd/launchd/schtasks 托管，具备开机自启与崩溃自拉）
    step("正在请求服务管理器启动守卫…");
    let started = platform::service().start();
    if let Err(e) = &started {
        update::log(&format!("服务管理器启动失败: {}", e));
    }
    step("等待守卫就绪（服务管理器路径）…");
    if wait_alive(port, 60) { return Ok(()); }

    // ③ 兜底：直接拉起守护进程。
    //    服务管理器不可用的场景真实存在（容器/无 user session/策略拦截），
    //    此时若不给兜底，用户将被永久挡在门外。
    step("服务管理器未能在 30s 内拉起守卫 · 改用直接启动兜底…");
    match platform::service().spawn_daemon(&guard) {
        Ok(pid) => update::log(&format!("兜底 spawn 守卫 pid={}", pid)),
        Err(e) => {
            return Err(format!(
                "守卫启动失败：服务管理器错误({}) 且直接拉起也失败({})",
                started.err().unwrap_or_else(|| "无".into()),
                e
            ));
        }
    }
    if wait_alive(port, 120) { return Ok(()); }

    Err(format!(
        "守卫启动超时（服务管理器与直接拉起均未就绪）。服务管理器错误：{}",
        started.err().unwrap_or_else(|| "无".into())
    ))
}

/// 轮询等待端口存活（每 tick 500ms）。
fn wait_alive(port: u16, ticks: u32) -> bool {
    for _ in 0..ticks {
        if is_alive(port) { return true; }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    false
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

/// 无头自检：**环境探测**（架构修复后的可诊断入口）。
///
/// 为什么需要：探测根因是「环境特有」的（某个候选上有阻塞型系统调用），
/// 靠读代码无法确定。本入口在有界预算内跑完探测并打印**逐候选追踪**，
/// 卡住时也能看到「卡在谁、多久」—— 这是定位该类问题唯一可靠的手段。
/// 用法：dsh-supervisor-gui --env-plan
/// 无头自检：**镜像测速与选择**。
///
/// 为什么需要：用户曾反馈「连镜像源都看不到，根本不会去选择镜像源」。
/// 功能本身是好的，但可见性缺失会被合理地理解为能力不存在。
/// 本入口把「测了哪些源、各自延迟、最终选了谁」变成**可核对的事实**。
/// 用法：dsh-supervisor-gui --mirror-plan
fn cli_mirror_plan() -> i32 {
    println!("== 镜像测速自检 ==");
    let m = mirror::load();
    println!("Node 候选 {} 个 / npm 候选 {} 个", m.node.len(), m.npm.len());
    println!("");
    println!("--- 并行测速（Node index.json）---");
    let np = mirror::probe_all(&m.node, "index.json");
    for p in &np {
        println!(
            "  {:<48} {} {:>6} ms",
            p.source,
            if p.ok { "可达" } else { "不可达" },
            p.latency_ms
        );
    }
    println!("");
    println!("--- 并行测速（npm registry）---");
    let pp = mirror::probe_all(&m.npm, "");
    for p in &pp {
        println!(
            "  {:<48} {} {:>6} ms",
            p.source,
            if p.ok { "可达" } else { "不可达" },
            p.latency_ms
        );
    }
    println!("");
    match np.iter().find(|p| p.ok) {
        Some(b) => println!("Node 选中: {} ({} ms)", b.source, b.latency_ms),
        None => println!("Node 选中: 无（全部不可达）"),
    }
    match pp.iter().find(|p| p.ok) {
        Some(b) => println!("npm  选中: {} ({} ms)", b.source, b.latency_ms),
        None => println!("npm  选中: 无（全部不可达）"),
    }
    0
}

fn cli_env_plan() -> i32 {
    println!("== 环境探测自检 ==");
    println!("平台          = {}", std::env::consts::OS);
    // 注意顺序：候选摘要由**探测线程**写入缓存，必须在 status() 之后再读，
    // 否则首次调用会读到空串（诊断输出出现空白，易被误读为「没有候选」）。
    let out = nodeprobe::status(std::time::Duration::from_secs(60));
    println!("{}", nodeprobe::candidate_summary());
    println!("完成          = {}", out.finished);
    println!("耗时          = {} ms", out.elapsed_ms);
    if let Some(e) = &out.error {
        println!("失败原因      = {}", e);
    }
    match (&out.path, &out.version) {
        (Some(p), Some(v)) => println!("node          = {} @ {}", v, p.display()),
        _ => println!("node          = （未找到）"),
    }
    if let Some((on, ms)) = nodeprobe::current_stuck() {
        println!("⚠ 仍在探测    = {} （已 {} ms）", on, ms);
    }
    println!("逐候选追踪:");
    print!("{}", nodeprobe::render_trace(&out.trace));
    0
}

fn cli_plan() -> i32 {
    let out = nodeprobe::status(std::time::Duration::from_secs(30));
    let sys = match (&out.path, &out.version) {
        (Some(p), Some(v)) => Some((p.clone(), v.clone())),
        _ => None,
    };
    match &sys {
        Some((p, v)) => println!("node=present {} @ {}", v, p.display()),
        None => println!("node=missing（finished={}）", out.finished),
    }
    println!("node_probe_candidates={}", nodeprobe::candidate_summary());
    print!("{}", nodeprobe::render_trace(&out.trace));
    match node::latest_lts() {
        Ok(c) => {
            println!("latest_lts={} file={}", c.version, c.file);
            println!("mirror_selected={}", c.source);
            for (s, ok, ms) in &c.probes {
                println!("mirror_probe={} ok={} latency_ms={}", s, ok, ms);
            }
            println!("node_outdated={}", node::outdated(sys.as_ref().map(|x| x.1.as_str()), &c.version));
            0
        }
        Err(e) => { eprintln!("latest_lts_error={}", e); 1 }
    }
}

/// 打开面板（分体架构 2026-09-07 定稿）：
/// 壳 = 自绘窗口容器(shell.html 唯一窗口栏 + 内容 iframe)；面板由守卫内核 HTTP 托管（同源）。
/// 切面板 = emit 守卫实际 API 基址(读 config.apiPort, 动态端口不硬编码) → 壳 iframe 导航该 URL，
/// 页面与守卫 API 同源直连（无跨源/CORS 透传）。
/// 返回控制面板 URL（供壳框架在导航后自行取得面板地址）。
///
/// 为什么需要：引导页与壳框架是**两个主帧页面**（引导完成后导航切换），
/// 而 Rust 的 shell:goto-panel 事件在首帧可能早于 listener 注册而被丢弃。
/// 由壳框架主动索取，可彻底避免事件竞态。
#[tauri::command]
fn shell_panel_url() -> serde_json::Value {
    let url = env::api_base_url();
    // 落盘一行：**证明主帧导航确实完成**（引导页 → 壳框架）。
    // 这条日志也是可观测性的关键一环：从 shell.log 就能看出卡在引导页还是壳框架。
    update::log(&format!("壳框架就绪（主帧导航完成），面板 URL: {}", url));
    serde_json::json!({ "url": url })
}

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

/// 把本地 API 调用派发到独立线程（**绝不阻塞 UI 线程**）。
///
/// 用于托盘菜单等由 UI 线程派发的回调：这些回调里的网络 I/O 一旦阻塞，
/// 整个界面（含重绘）都会被冻结 —— 实测观感是「点击无反应」。
///
/// 退出流程同样经此派发：即使守卫无响应，菜单也立即响应，
/// 用户不会觉得「程序关不掉」。
fn spawn_local_post(port: u16, path: &'static str) {
    std::thread::spawn(move || post_local(port, path));
}

/// 同 post_local，但带读写超时（防止守卫挂起时壳无限阻塞）。返回响应体（解码 utf8 尽力）。
/// 与本地守卫建立连接，**带连接超时**。
///
/// ⚠ 必须用 `connect_timeout` 而非 `connect`（2026-09-11 审计）：`TcpStream::connect`
///   **没有超时** —— 若端口被防火墙 DROP（而非 REJECT），连接会一直等到操作系统的
///   SYN 重试耗尽，Windows 上默认可达 20+ 秒。而本函数被 `guard_ready`（引导页每次
///   500ms 轮询一次、最多 40 次）与托盘动作调用，等同于反复长时间阻塞。
///   回环地址正常时是微秒级，但**不能依赖「正常时很快」来省略上限**。
fn connect_local(port: u16, timeout: std::time::Duration) -> Option<TcpStream> {
    use std::net::ToSocketAddrs;
    let addr = format!("127.0.0.1:{}", port);
    let sa = addr.to_socket_addrs().ok()?.next()?;
    TcpStream::connect_timeout(&sa, timeout).ok()
}

/// 本地 HTTP 请求的连接预算（回环地址，正常为微秒级；此处仅作兜底上限）。
const LOCAL_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(800);

fn post_local_timeout(port: u16, path: &str, timeout: std::time::Duration) -> Option<String> {
    let mut stream = connect_local(port, LOCAL_CONNECT_TIMEOUT)?;
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
    let mut stream = connect_local(port, LOCAL_CONNECT_TIMEOUT)?;
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
    let mut stream = connect_local(port, LOCAL_CONNECT_TIMEOUT)?;
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

/// 读取当前镜像配置 + 并行探测延迟（供引导页展示与选择）。
#[tauri::command]
async fn mirror_status() -> Result<serde_json::Value, String> {
    let m = crate::mirror::load();
    let node_probes = tauri::async_runtime::spawn_blocking(|| {
        let m = crate::mirror::load();
        crate::mirror::probe_all(&m.node, "index.json")
    })
    .await
    .map_err(|e| e.to_string())?;
    let npm_probes = tauri::async_runtime::spawn_blocking(|| {
        let m = crate::mirror::load();
        crate::mirror::probe_all(&m.npm, "")
    })
    .await
    .map_err(|e| e.to_string())?;
    let fmt = |v: Vec<crate::mirror::Probe>| {
        v.into_iter()
            .map(|p| serde_json::json!({ "source": p.source, "ok": p.ok, "latencyMs": p.latency_ms }))
            .collect::<Vec<_>>()
    };
    Ok(serde_json::json!({
        "ok": true,
        "node": m.node,
        "npm": m.npm,
        "shell": m.shell,
        "selectedNode": m.selected_node,
        "selectedNpm": m.selected_npm,
        "nodeProbes": fmt(node_probes),
        "npmProbes": fmt(npm_probes),
    }))
}

/// 保存用户自定义镜像（引导页失败时的自助出口）。
/// 入参为 URL 列表；保存后使缓存失效，并在 npm 类型时立即导出给内核（若已安装）。
#[tauri::command]
fn mirror_set(kind: String, urls: Vec<String>) -> Result<serde_json::Value, String> {
    let mut m = crate::mirror::load();
    let list: Vec<String> = urls
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if list.is_empty() {
        return Err("镜像列表不能为空".into());
    }
    for u in &list {
        if !(u.starts_with("http://") || u.starts_with("https://")) {
            return Err(format!("镜像地址必须以 http(s):// 开头：{}", u));
        }
    }
    match kind.as_str() {
        "node" => m.node = list.clone(),
        "npm" => m.npm = list.clone(),
        "shell" => m.shell = list.clone(),
        _ => return Err(format!("未知镜像类型: {}（支持 node / npm / shell）", kind)),
    }
    m.checked_at = None; // 使缓存失效，下次重新测速
    if kind == "npm" {
        m.selected_npm = None; // 用户改了候选集 → 旧的选择结果失效
    }
    crate::mirror::save(&m)?;
    if kind == "npm" {
        // 导出**完整契约**（schema/catalog/selected/probe），而非仅 origins。
        let _ = crate::mirror::export_to_kernel(&m);
    }
    update::log(&format!("镜像配置已更新 {}: {}", kind, list.join(", ")));
    Ok(serde_json::json!({ "ok": true, "kind": kind, "urls": list }))
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
    use std::path::PathBuf;
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
        std::process::exit(cli_mirror_plan());
    }
    // 无头自检：环境探测（架构修复后的可诊断入口）。
    if std::env::args().any(|a| a == "--env-plan") {
        std::process::exit(cli_env_plan());
    }
    // 无头自检：守卫服务定义（P0 修复的功能验证入口，任何平台可用）。
    if std::env::args().any(|a| a == "--service-plan") {
        std::process::exit(cli_service_plan());
    }
    // 无头冒烟入口：--node-plan 仅打印环境探针 + 官方最新 LTS，不启动窗口。
    if std::env::args().any(|a| a == "--node-plan") {
        std::process::exit(cli_plan());
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
            show_main(app);
        }))
        // 壳自更新插件：强制 minisign 验签；平台安装语义内部处理。
        // 未配置 pubkey 时插件仍可注册（check 会失败并返回错误，由引导页按「失败放行」处理）。
        .plugin(tauri_plugin_updater::Builder::new().build())
        // app.restart()：更新安装后重启进入新版本（旧进程装、新进程跑）。
        .plugin(tauri_plugin_process::init())
        .manage(Mutex::new(RunState::default()))
        .invoke_handler(tauri::generate_handler![node_status, core_status, core_plan, core_apply, guard_start, guard_ready, start_node_install, finish_boot, win_ctl, shell_identity, shell_update_check, shell_update_apply, shell_restart, shell_set_phase, mirror_status, mirror_set, node_latest, mirror_warmup, mirror_cached, shell_panel_url])
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
                //   导致右键时既弹菜单又调用 show_main() 抢焦点 → 菜单被顶掉，
                //   用户感知为「右键不好用」（2026-09-11 Windows 真机实测）。
                // 注：上游文档明确 Linux 不支持该开关（菜单由桌面环境决定）——
                //     故 Linux 上左键可能仍显示菜单，属平台限制，非本仓可控。
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| {
                    match event.id.as_ref() {
                        "show" => show_main(app),
                        // ⚠ 网络 I/O **必须离开 UI 线程**（2026-09-11 架构修复）。
                        //   托盘菜单事件由 UI 线程派发，而 post_local 最多阻塞 60 秒
                        //   （TCP 连接 + 读写超时）。守卫挂起或端口无响应时，
                        //   点击「启动/停止/重启」会**把整个界面冻结 60 秒** ——
                        //   用户看到的是「点了没反应」，且期间窗口无法重绘。
                        //   改为派发到独立线程：菜单立即响应，结果异步生效。
                        "start" => spawn_local_post(port, "/lifecycle/dsh/start"),
                        "stop" => spawn_local_post(port, "/lifecycle/dsh/stop"),
                        "restart" => spawn_local_post(port, "/lifecycle/dsh/restart"),
                        // 退出管家 = 完全退出：通知守卫停止全部服务链，随后壳退出
                        "quit" => {
                            // 契约 §4.1：请求内核停被管对象（等回执）→ 由所有者停止守卫 → 壳退出。
                            // ⚠ 同样离开 UI 线程：退出握手最坏可耗时约 70 秒（/session/stop 60s
                            //   + 轮询 10s）。若在 UI 线程做，用户会看到窗口卡住不动，
                            //   误以为「程序关不掉」而强杀 —— 那会跳过退出握手，留下未停的 DSH。
                            let h = app.clone();
                            std::thread::spawn(move || {
                                shutdown_all(port);
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
    bt!("main exit (run returned)");
}
