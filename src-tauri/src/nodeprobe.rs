//! 有界 Node 探测（**架构层修复**，2026-09-11）。
//!
//! == 为什么需要独立模块（根因分析） ==
//!
//! 1.0.3/1.0.4 都卡在「检测环境」，说明上一轮修复**没有触及根因**。根因有两层：
//!
//! **根因一：探测里存在无法用「预算」约束的阻塞系统调用。**
//!   · is_file() / metadata() 底层是 GetFileAttributesW，在断开的网络盘、部分过滤驱动下
//!     会阻塞数十秒甚至更久；
//!   · Command::spawn() 底层是 CreateProcessW，在应用执行别名（App Execution Alias）、
//!     网络路径、杀软实时扫描下同样可能长时间不返回。
//!   而 env.rs 的 5 秒/20 秒「预算」只在候选**之间**、spawn 返回**之后**才被检查 ——
//!   单个阻塞调用即可击穿全部预算。**那不是真正的上限，只是提示。**
//!
//! **根因二：探测任务被 Tauri 命令 await，于是「探测阻塞」等价于「命令永不返回」。**
//!   前端 45 秒超时只是**停止等待**，并不会取消 Rust 任务；每次重试再起一个挂起线程。
//!   用户看到的正是这个签名：node=unknown、shell=unknown，且永不恢复。
//!
//! == 修复（分层） ==
//!
//! A. **分离线程 + 通道 + recv_timeout**：命令的返回时间与任何阻塞调用**完全无关**。
//!    探测只允许一个在飞（Idle / Running / Done 状态机），重试不再堆积线程。
//!    这是唯一能真正给 CreateProcessW / GetFileAttributesW 加界的手段。
//! B. **廉价优先**：先读我们自己记录的 runtime.json，再查已知安装目录，PATH 放最后；
//!    Windows 上跳过非固定磁盘与 UNC 路径 —— 从根上避开「网络路径挂起」这一整类。
//! C. **可观测**：记录每个候选的耗时，并记录「当前正在探测谁」；卡住时也能看到前因。
//!    环境特有问题无法靠读代码确定，必须靠这份追踪定位。
//!
//! 注意：本文件刻意不使用反引号、单引号字面量与反斜杠字面量（用数值 92/58 表达
//! 反斜杠与冒号），以免文档与代码在跨格式传递时被转义破坏。

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// 单个候选的探测记录（诊断用）。
#[derive(Clone)]
pub struct TraceEntry {
    pub source: String,
    pub path: String,
    pub ms: u128,
    pub ok: bool,
    pub note: String,
}

/// 一次探测的结论。finished=false 表示探测线程仍在跑，本对象只是阶段快照。
#[derive(Clone)]
pub struct Outcome {
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    pub trace: Vec<TraceEntry>,
    pub elapsed_ms: u128,
    pub finished: bool,
}

/// 在飞探测的实时状态（与 Outcome 分离：即使探测永不返回，也能读到进展）。
struct Live {
    current: Option<(String, Instant)>,
    done: Vec<TraceEntry>,
}

fn live() -> &'static Mutex<Live> {
    static L: OnceLock<Mutex<Live>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(Live { current: None, done: Vec::new() }))
}

fn live_reset() {
    if let Ok(mut l) = live().lock() {
        l.current = None;
        l.done.clear();
    }
}

fn live_stage(desc: String) {
    if let Ok(mut l) = live().lock() {
        l.current = Some((desc, Instant::now()));
    }
}

fn live_finish(entry: TraceEntry) {
    if let Ok(mut l) = live().lock() {
        l.current = None;
        l.done.push(entry);
    }
}

fn live_snapshot() -> Vec<TraceEntry> {
    match live().lock() {
        Ok(l) => l.done.clone(),
        Err(_) => Vec::new(),
    }
}

/// 当前正在探测的候选与已耗时（毫秒）。诊断用：卡住时能看到「卡在谁身上、多久」。
pub fn current_stuck() -> Option<(String, u128)> {
    let l = live().lock().ok()?;
    let (s, t) = l.current.as_ref()?;
    Some((s.clone(), t.elapsed().as_millis()))
}

/// 判定在飞探测是否已「陈旧」。
///
/// 阻塞调用可能永久挂起（网络路径上的 CreateProcessW 无上限），若不允许重启，
/// 探测能力会被永久剥夺。超过该时长即允许重新发起 —— 代价是极端情况下多出若干
/// 挂起线程（受用户重试次数限制），但**探测能力不被永久剥夺**，这是更重要的性质。
const STALE_AFTER: Duration = Duration::from_secs(90);

enum State {
    Idle,
    Running(Option<Receiver<Outcome>>, Instant),
    Done(Outcome),
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State::Idle))
}

/// 使缓存失效（Node 安装完成后调用：必须让下一次探测重新发现新装的 Node）。
pub fn invalidate() {
    if let Ok(mut g) = state().lock() {
        *g = State::Idle;
    }
}

/// 阶段快照（探测未完成时返回）。
pub fn partial() -> Outcome {
    let mut trace = live_snapshot();
    if let Some((desc, ms)) = current_stuck() {
        trace.push(TraceEntry {
            source: "进行中".to_string(),
            path: desc,
            ms,
            ok: false,
            note: "探测中（尚未返回；该候选若有阻塞型系统调用，可能长时间无响应）".to_string(),
        });
    }
    Outcome { path: None, version: None, trace, elapsed_ms: 0, finished: false }
}

/// 取探测结论，**绝不阻塞超过 budget**。
///
/// 这是整套修复的核心：无论内部系统调用是否挂起，本函数都在 budget 内返回。
pub fn status(budget: Duration) -> Outcome {
    // 阶段一：决定「复用 / 重启 / 新建」在飞探测（持锁但几乎不耗时）
    {
        let mut g = match state().lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        match &mut *g {
            State::Done(o) => return o.clone(),
            State::Running(rx, started) => {
                if started.elapsed() >= STALE_AFTER {
                    // 陈旧：重启（旧线程若日后返回，其 send 会失败，无副作用）
                    let (tx, rx2) = channel();
                    live_reset();
                    spawn_worker(tx);
                    *rx = Some(rx2);
                    *started = Instant::now();
                }
            }
            State::Idle => {
                let (tx, rx) = channel();
                live_reset();
                spawn_worker(tx);
                *g = State::Running(Some(rx), Instant::now());
            }
        }
    }

    // 阶段二：取出 receiver 等待（**不持锁阻塞**，避免与 invalidate 等调用相互影响）
    let rx = {
        let mut g = match state().lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        match &mut *g {
            State::Running(rx, _) => rx.take(),
            State::Done(o) => return o.clone(),
            State::Idle => None,
        }
    };
    let Some(rx) = rx else {
        return partial();
    };

    let res = rx.recv_timeout(budget);

    let mut g = match state().lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    match res {
        Ok(o) => {
            *g = State::Done(o.clone());
            o
        }
        Err(RecvTimeoutError::Timeout) => {
            // 放回 receiver：下次调用可继续等同一个在飞探测（不新起线程）
            if let State::Running(slot, _) = &mut *g {
                *slot = Some(rx);
            }
            partial()
        }
        Err(RecvTimeoutError::Disconnected) => {
            // 工作线程异常结束（不应发生）→ 回到 Idle，允许下次重建
            *g = State::Idle;
            partial()
        }
    }
}

/// 仅触发探测、不等待（UI 启动时预热，避免「首次调用才起线程」的额外延迟）。
pub fn start() {
    let _ = status(Duration::from_millis(1));
}

/// 便捷入口：在有界预算内尽力取回结论；未完成或未找到则 None。
/// 注意：**不要**用它做「Node 不存在」的强判断 —— 未完成也会返回 None，
/// 调用方应区分「已确认不存在」与「尚未探完」（用 status 的 finished 字段）。
pub fn resolve(budget: Duration) -> Option<(PathBuf, String)> {
    let out = status(budget);
    match (out.path, out.version) {
        (Some(p), Some(v)) => Some((p, v)),
        _ => None,
    }
}

fn spawn_worker(tx: Sender<Outcome>) {
    let _ = std::thread::Builder::new()
        .name("node-probe".to_string())
        .spawn(move || {
            let started = Instant::now();
            let (path, version) = detect();
            let outcome = Outcome {
                path,
                version,
                trace: live_snapshot(),
                elapsed_ms: started.elapsed().as_millis(),
                finished: true,
            };
            let _ = tx.send(outcome);
        });
}

fn detect() -> (Option<PathBuf>, Option<String>) {
    for (source, cand) in candidates() {
        let p = cand.to_string_lossy().to_string();
        live_stage(source.clone() + " " + &p);
        let t0 = Instant::now();
        if !crate::env::is_usable_candidate(&cand) {
            live_finish(TraceEntry {
                source,
                path: p,
                ms: t0.elapsed().as_millis(),
                ok: false,
                note: "不可用（不存在 / 应用别名存根 / 空文件）".to_string(),
            });
            continue;
        }
        match crate::env::node_version(&cand) {
            Some(v) => {
                live_finish(TraceEntry {
                    source,
                    path: p,
                    ms: t0.elapsed().as_millis(),
                    ok: true,
                    note: v.clone(),
                });
                return (Some(cand), Some(v));
            }
            None => live_finish(TraceEntry {
                source,
                path: p,
                ms: t0.elapsed().as_millis(),
                ok: false,
                note: "无响应或不是有效 Node（已按上限终止）".to_string(),
            }),
        }
    }
    (None, None)
}

/// 候选清单，**按代价从低到高**排序：
///   1) 我们自己记录过的路径（runtime.json）—— 一次本地读，最廉价也最可信；
///   2) 已知安装落点（含版本管理器）—— 少数几次本地 stat；
///   3) PATH 扫描 —— 放最后，且过滤掉非固定盘/UNC。
///
/// 为什么顺序重要：执行外部二进制是**最贵且最危险**的探测方式。
/// 把「猜 PATH」当成首选探测手段，正是本缺陷的架构根源。
fn candidates() -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    let push = |src: String, p: PathBuf, out: &mut Vec<(String, PathBuf)>| {
        if !out.iter().any(|(_, e)| *e == p) {
            out.push((src, p));
        }
    };

    // ① 自己记录的路径
    if let Some(p) = crate::env::recorded_node_path() {
        push("记录".to_string(), p, &mut out);
    }

    // ② 已知安装落点
    for (src, p) in known_locations() {
        push(src, p, &mut out);
    }

    // ③ PATH（最后）
    for dir in crate::env::path_dirs_local_only() {
        push("PATH".to_string(), dir.join(crate::env::node_exe()), &mut out);
    }

    out
}

/// 已知安装落点。全部通过 PathBuf::push 拼接（不写字面量反斜杠，避免跨格式转义问题）。
fn known_locations() -> Vec<(String, PathBuf)> {
    let mut v: Vec<(String, PathBuf)> = Vec::new();

    #[cfg(target_os = "windows")]
    {
        let exe = "node.exe";
        if let Ok(pf) = std::env::var("ProgramFiles") {
            v.push(("已知".to_string(), PathBuf::from(&pf).join("nodejs").join(exe)));
        }
        if let Ok(pf) = std::env::var("ProgramFiles(x86)") {
            v.push(("已知".to_string(), PathBuf::from(&pf).join("nodejs").join(exe)));
        }
        if let Ok(pd) = std::env::var("ProgramData") {
            v.push(("已知".to_string(), PathBuf::from(&pd).join("chocolatey").join("bin").join(exe)));
        }
        if let Ok(la) = std::env::var("LOCALAPPDATA") {
            v.push(("已知".to_string(), PathBuf::from(&la).join("Programs").join("nodejs").join(exe)));
            // Volta
            v.push(("已知".to_string(), PathBuf::from(&la).join("Volta").join("bin").join(exe)));
            // nvm-windows：%LOCALAPPDATA%\nvm\vX.Y.Z\node.exe
            if let Some(p) = latest_versioned_node(PathBuf::from(&la).join("nvm")) {
                v.push(("已知".to_string(), p));
            }
        }
        if let Ok(ap) = std::env::var("APPDATA") {
            // nvm-windows 的另一种常见落点
            if let Some(p) = latest_versioned_node(PathBuf::from(&ap).join("nvm")) {
                v.push(("已知".to_string(), p));
            }
        }
        // nvm 环境变量（显式设置时最权威）
        if let Ok(nh) = std::env::var("NVM_HOME") {
            if let Some(p) = latest_versioned_node(PathBuf::from(&nh)) {
                v.push(("已知".to_string(), p));
            }
            v.push(("已知".to_string(), PathBuf::from(&nh).join(exe)));
        }
        if let Ok(link) = std::env::var("NVM_SYMLINK") {
            v.push(("已知".to_string(), PathBuf::from(&link).join(exe)));
        }
        // scoop
        if let Ok(up) = std::env::var("USERPROFILE") {
            v.push((
                "已知".to_string(),
                PathBuf::from(&up).join("scoop").join("apps").join("nodejs").join("current").join(exe),
            ));
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        for p in ["/usr/local/bin/node", "/opt/homebrew/bin/node", "/usr/bin/node", "/bin/node"] {
            v.push(("已知".to_string(), PathBuf::from(p)));
        }
        if let Ok(home) = std::env::var("HOME") {
            let h = PathBuf::from(&home);
            v.push(("已知".to_string(), h.join(".volta").join("bin").join("node")));
            // nvm：~/.nvm/versions/node/vX.Y.Z/bin/node
            if let Some(p) = latest_versioned_node(h.join(".nvm").join("versions").join("node")) {
                v.push(("已知".to_string(), p.join("bin").join("node")));
            }
            // fnm：~/.local/share/fnm/node-versions/vX/installation/bin/node
            if let Some(p) = latest_versioned_node(h.join(".local").join("share").join("fnm").join("node-versions")) {
                v.push(("已知".to_string(), p.join("installation").join("bin").join("node")));
            }
        }
    }

    v
}

/// 在版本目录（形如 v22.12.0）中挑**版本最高**的一个，返回其中 node 可执行文件的路径。
///
/// 为什么必须挑最高：版本管理器下常并存多个 Node，低版本会因不满足 DSH 最低门槛而被拒。
fn latest_versioned_node(root: PathBuf) -> Option<PathBuf> {
    let rd = std::fs::read_dir(&root).ok()?;
    let mut best: Option<(Vec<u64>, PathBuf)> = None;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let trimmed = name.trim_start_matches('v');
        let nums: Vec<u64> = trimmed.split('.').map_while(|x| x.parse::<u64>().ok()).collect();
        if nums.is_empty() {
            continue;
        }
        let full = if cfg!(target_os = "windows") {
            e.path().join(crate::env::node_exe())
        } else {
            e.path().join("bin").join(crate::env::node_exe())
        };
        let better = match &best {
            None => true,
            Some((bv, _)) => nums > *bv,
        };
        if better {
            best = Some((nums, full));
        }
    }
    best.map(|(_, p)| p)
}

/// 供诊断：候选数量与来源分布（不执行任何探测）。
pub fn candidate_summary() -> String {
    let c = candidates();
    let recorded = c.iter().filter(|(s, _)| s == "记录").count();
    let known = c.iter().filter(|(s, _)| s == "已知").count();
    let path = c.iter().filter(|(s, _)| s == "PATH").count();
    format!("候选 {} 个（记录 {} / 已知 {} / PATH {}）", c.len(), recorded, known, path)
}

/// 把追踪渲染为多行文本（诊断 / CLI 自检用）。
pub fn render_trace(trace: &[TraceEntry]) -> String {
    let mut s = String::new();
    for e in trace {
        s.push_str(&format!(
            "  [{}] {} {} ms ok={} {}{}",
            e.source,
            e.path,
            e.ms,
            e.ok,
            e.note,
            String::from_utf8_lossy(&[10u8])
        ));
    }
    s
}


