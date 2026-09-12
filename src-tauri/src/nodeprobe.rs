//! 有界 Node 探测（架构层修复，2026-09-11 二次修订）。
//!
//! == 根因（两轮修复的完整认识） ==
//!
//! 1.0.3/1.0.4/1.0.5 都卡在「检测环境」，三轮修复逐层揭开真相：
//!
//! **第一层**：探测里有无界阻塞系统调用（GetFileAttributesW / CreateProcessW /
//!   GetDriveTypeW），而原「预算」只在候选之间、spawn 返回之后才检查 —— 不是上限，只是提示。
//! **第二层**：探测被命令 await，于是「探测挂起」等价于「命令永不返回」；
//!   前端超时只是停止等待，Rust 线程仍永久悬挂。
//! **第三层（本文件本次修复）**：把探测搬进分离线程后，**候选枚举本身仍在进度上报之外** ——
//!   一旦卡在枚举阶段（GetDriveTypeW / read_dir），诊断串里 summary / stuck / trace
//!   **三项同时为空**，表现为「卡住且不报错、也没有任何线索」。
//!
//! == 因此本文件遵守两条硬规则 ==
//!
//! **规则一：任何可能阻塞的调用，之前都必须先 stage()。**
//!   没有例外 —— 包括候选枚举、盘符判定、目录读取。这正是第三次踩坑的地方：
//!   把 I/O 搬进线程只解决「命令不阻塞」，**不解决「卡住时看不见线索」**。
//! **规则二：Rust 侧必须有硬上限（HARD_DEADLINE）。**
//!   超过它即判定本次探测**明确失败**并返回原因，而不是让 probing 永远为真。
//!   即：**即使某个系统调用永久挂起，命令也一定给出结论。**
//!
//! 实现细节说明：本文件刻意不使用反引号与单引号字面量（用数值 92/58 表达反斜杠与冒号），
//! 以免文档与代码在跨格式传递时被转义破坏。

use std::path::{Path, PathBuf};
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

/// 一次探测的结论。
#[derive(Clone)]
pub struct Outcome {
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    pub trace: Vec<TraceEntry>,
    pub elapsed_ms: u128,
    /// true = 本次探测已结束（成功或明确失败）；false = 仍在进行（phase 快照）。
    pub finished: bool,
    /// 明确失败的原因（finished=true 且未找到时必有值）。
    pub error: Option<String>,
}

struct Live {
    current: Option<(String, Instant)>,
    done: Vec<TraceEntry>,
    summary: String,
}

fn live() -> &'static Mutex<Live> {
    static L: OnceLock<Mutex<Live>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(Live { current: None, done: Vec::new(), summary: String::new() }))
}

fn live_reset() {
    if let Ok(mut l) = live().lock() {
        l.current = None;
        l.done.clear();
    }
}

/// 标记「即将做一件可能阻塞的事」。**调用它必须在任何 I/O 之前**（规则一）。
fn stage(desc: &str) {
    if let Ok(mut l) = live().lock() {
        l.current = Some((desc.to_string(), Instant::now()));
    }
}

fn finish(entry: TraceEntry) {
    if let Ok(mut l) = live().lock() {
        l.current = None;
        l.done.push(entry);
    }
}

fn set_summary(s: String) {
    if let Ok(mut l) = live().lock() {
        l.summary = s;
    }
}

fn snapshot() -> Vec<TraceEntry> {
    match live().lock() {
        Ok(l) => l.done.clone(),
        Err(e) => e.into_inner().done.clone(),
    }
}

/// 当前正在做哪一步、已耗时多久。**卡住时的唯一线索。**
pub fn current_stuck() -> Option<(String, u128)> {
    let l = live().lock().ok()?;
    let (s, t) = l.current.as_ref()?;
    Some((s.clone(), t.elapsed().as_millis()))
}

/// 候选摘要（**只读缓存，不做任何 I/O**）。由探测线程在枚举完成后写入。
pub fn candidate_summary() -> String {
    match live().lock() {
        Ok(l) => l.summary.clone(),
        Err(e) => e.into_inner().summary.clone(),
    }
}

/// 在飞探测的实时状态（与 Outcome 分离：即使探测永不返回，也能读到进展）。
const STALE_AFTER: Duration = Duration::from_secs(90);

/// **硬上限**（规则二）：超过它即判定本次探测明确失败并给出原因。
///
/// 为什么必须有：即使前面所有 stage 与缓存都到位，若某个系统调用**永久**挂起，
/// 前端仍会等到自己的预算耗尽才报「超时」—— 那是个没有信息量的结论。
/// 有了硬上限，命令会主动返回「卡在 <阶段> 已 N 秒」，用户与排障都能直接定位。
/// 硬上限的实际取值（毫秒）。运行时可变**仅为测试可注入** —— 正式路径恒为 25000。
static HARD_DEADLINE_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(25_000);

fn hard_deadline() -> Duration {
    Duration::from_millis(HARD_DEADLINE_MS.load(std::sync::atomic::Ordering::Relaxed))
}

/** 仅供测试：注入硬上限。 */
#[cfg(test)]
pub fn set_hard_deadline_ms(ms: u64) {
    HARD_DEADLINE_MS.store(ms, std::sync::atomic::Ordering::Relaxed);
}

/** 仅供测试：让候选枚举阶段永久阻塞，用于验证「卡住时一定给出带阶段的结论」。 */
#[cfg(test)]
static HANG_IN_ENUMERATE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
pub fn set_hang_in_enumerate(v: bool) {
    HANG_IN_ENUMERATE.store(v, std::sync::atomic::Ordering::Relaxed);
}

enum State {
    Idle,
    Running(Option<Receiver<Outcome>>, Instant),
    Done(Outcome),
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State::Idle))
}

/// 使缓存失效（Node 安装完成后调用）。
pub fn invalidate() {
    if let Ok(mut g) = state().lock() {
        *g = State::Idle;
    }
}

/// 阶段快照（探测未完成，但尚未到硬上限）。
pub fn partial() -> Outcome {
    let mut trace = snapshot();
    if let Some((desc, ms)) = current_stuck() {
        trace.push(TraceEntry {
            source: "进行中".to_string(),
            path: desc,
            ms,
            ok: false,
            note: "该步骤尚未返回（若为 I/O 步骤，可能被系统调用阻塞）".to_string(),
        });
    }
    Outcome { path: None, version: None, trace, elapsed_ms: 0, finished: false, error: None }
}

/// 明确失败（带原因与已收集的追踪）。
fn failed(reason: String, elapsed_ms: u128) -> Outcome {
    let mut trace = snapshot();
    if let Some((desc, ms)) = current_stuck() {
        trace.push(TraceEntry {
            source: "卡住".to_string(),
            path: desc.clone(),
            ms,
            ok: false,
            note: format!("该步骤已 {} ms 无响应", ms),
        });
    }
    Outcome { path: None, version: None, trace, elapsed_ms, finished: true, error: Some(reason) }
}

/// 取探测结论，**绝不阻塞超过 budget**。
///
/// 三种返回：
///   · 正常完成（finished=true, error=None）
///   · 仍在进行（finished=false）—— 由前端轮询
///   · 明确失败（finished=true, error=Some）—— 到硬上限或工作线程异常
pub fn status(budget: Duration) -> Outcome {
    // ── 决定「复用 / 重启 / 新建」在飞探测（持锁但几乎不耗时）──
    let started_at;
    {
        let mut g = match state().lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        match &mut *g {
            State::Done(o) => return o.clone(),
            State::Running(rx, started) => {
                if started.elapsed() >= STALE_AFTER {
                    let (tx, rx2) = channel();
                    live_reset();
                    spawn_worker(tx);
                    *rx = Some(rx2);
                    *started = Instant::now();
                }
                started_at = *started;
            }
            State::Idle => {
                let (tx, rx) = channel();
                live_reset();
                // 规则一：在启动探测前先标记阶段 —— 线程调度本身也可能延迟，
                // 若此处不标记，前端会看到「probing 但没有任何阶段」，与卡死无法区分。
                stage("启动探测线程");
                spawn_worker(tx);
                let now = Instant::now();
                *g = State::Running(Some(rx), now);
                started_at = now;
            }
        }
    }

    // ── 取出 receiver 等待（不持锁阻塞，避免与 invalidate 等调用相互影响）──
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
            if started_at.elapsed() >= hard_deadline() {
                // 规则二：主动给出明确失败，而不是让 probing 永远为真。
                // 同时回到 Idle，让「重试」能重新开始一次干净探测。
                let el = started_at.elapsed().as_millis();
                let where_ = current_stuck()
                    .map(|(d, ms)| format!("卡在「{}」已 {} ms", d, ms))
                    .unwrap_or_else(|| "探测线程未报告任何阶段".to_string());
                *g = State::Idle;
                failed(
                    format!(
                        "环境探测超过 {} ms 未完成（{}）",
                        hard_deadline().as_millis(),
                        where_
                    ),
                    el,
                )
            } else {
                if let State::Running(slot, _) = &mut *g {
                    *slot = Some(rx);
                }
                partial()
            }
        }
        Err(RecvTimeoutError::Disconnected) => {
            // 工作线程异常结束（不应发生）：明确失败，而不是静默停在 probing。
            let el = started_at.elapsed().as_millis();
            *g = State::Idle;
            failed("探测线程异常退出".to_string(), el)
        }
    }
}

/// 触发探测但不等待（UI 启动时预热）。
pub fn start() {
    let _ = status(Duration::from_millis(1));
}

/// 便捷入口：在有界预算内尽力取回结论。
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
            // 规则一/二之外的最后一道保险：worker 内 panic 也必须产出结论。
            // 否则线程静默死亡 → 命令只能一直报 probing（这正是前几轮的现象之一）。
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(detect));
            let outcome = match r {
                Ok((path, version, err)) => Outcome {
                    path,
                    version,
                    trace: snapshot(),
                    elapsed_ms: started.elapsed().as_millis(),
                    finished: true,
                    error: err,
                },
                Err(_) => Outcome {
                    path: None,
                    version: None,
                    trace: snapshot(),
                    elapsed_ms: started.elapsed().as_millis(),
                    finished: true,
                    error: Some("探测过程内部异常（已捕获，未静默）".to_string()),
                },
            };
            let _ = tx.send(outcome);
        });
}

/// 返回 (路径, 版本, 明确失败原因)。
///
/// == 关键结构改进：**边枚举边探测，且把最可能慢的来源放到最后** == (2026-09-11 三次修复) ==
///
/// 上一版的顺序是「先把**全部**候选枚举完，再逐个探测」。这有一个致命后果：
/// **只要枚举阶段慢/卡（PATH 过滤要逐盘符调 GetDriveTypeW），
/// 就连已经枚举好的廉价候选都永远试不到** —— 用户本可瞬间命中「已知安装落点」，
/// 却因为 PATH 过滤卡住而完全失败。
///
/// 现改为**交错（interleaved）**：
///   ① 记录路径  → 立即探测
///   ② 已知落点  → 列一个、试一个
///   ③ PATH 过滤 → **最后才做**（这是唯一需要逐盘符系统调用的阶段）
///
/// 于是「Node 装在标准位置」的绝大多数用户（含本项目的目标场景）
/// **根本不会走到 PATH 过滤**，那个可疑的系统调用连一次都不会被调用。
/// 这比「把它加进进度上报」更根本：**不上报不如不调用。**
fn detect() -> (Option<PathBuf>, Option<String>, Option<String>) {
    #[cfg(test)]
    if HANG_IN_ENUMERATE.load(std::sync::atomic::Ordering::Relaxed) {
        // 精确复刻线上故障：卡在枚举阶段。
        // 若此处不 stage，诊断串里 summary/stuck/trace 三项会同时为空 ——
        // 即用户看到的「卡住且不报错、也没有任何线索」。
        stage("测试：模拟枚举阶段永久阻塞");
        std::thread::sleep(Duration::from_secs(10));
    }
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    let add = |src: &str, p: PathBuf, out: &mut Vec<(String, PathBuf)>| -> bool {
        if out.iter().any(|(_, e)| *e == p) {
            return false;
        }
        out.push((src.to_string(), p));
        true
    };

    // ───── ① 自己记录的路径（一次本地读；最廉价也最可信）─────
    stage("① 读取 runtime.json 记录路径");
    if let Some(p) = crate::env::recorded_node_path() {
        if add("记录", p.clone(), &mut out) {
            if let Some(v) = try_probe("记录", &p) {
                set_summary(summarize(&out));
                return (Some(p), Some(v), None);
            }
        }
    }
    set_summary(format!("{}（进行中：① 已试）", summarize(&out)));

    // ───── ② 已知安装落点（逐个：列一个、试一个）─────
    stage("② 枚举已知安装落点");
    for (src, p) in known_locations() {
        if !add(&src, p.clone(), &mut out) {
            continue;
        }
        set_summary(format!("{}（进行中：② 搜索中）", summarize(&out)));
        if let Some(v) = try_probe(&src, &p) {
            set_summary(summarize(&out));
            return (Some(p), Some(v), None);
        }
    }

    // ───── ③ PATH（最后：这是唯一需要逐盘符系统调用的阶段）─────
    let dirs = path_dirs_staged();
    let total = dirs.len();
    set_summary(format!("{}（进行中：③ PATH 已过滤 {} 条）", summarize(&out), total));
    for (i, dir) in dirs.into_iter().enumerate() {
        let cand = dir.join(crate::env::node_exe());
        if !add("PATH", cand.clone(), &mut out) {
            continue;
        }
        stage(&format!("③ 探测 PATH 候选 {}/{}", i + 1, total));
        if let Some(v) = try_probe("PATH", &cand) {
            set_summary(summarize(&out));
            return (Some(cand), Some(v), None);
        }
    }

    set_summary(summarize(&out));
    (None, None, None)
}

/// 探测单个候选：stage → 可用性判定 → 执行取版本 → 落追踪。返回版本（成功时）。
fn try_probe(source: &str, cand: &Path) -> Option<String> {
    let p = cand.to_string_lossy().to_string();
    stage(&format!("探测候选 {}（{}）", source, p));
    let t0 = Instant::now();
    if !crate::env::is_usable_candidate(cand) {
        finish(TraceEntry {
            source: source.to_string(),
            path: p,
            ms: t0.elapsed().as_millis(),
            ok: false,
            note: "不可用（不存在 / 应用别名存根 / 空文件）".to_string(),
        });
        return None;
    }
    match crate::env::node_version(cand) {
        Some(v) => {
            finish(TraceEntry {
                source: source.to_string(),
                path: p,
                ms: t0.elapsed().as_millis(),
                ok: true,
                note: v.clone(),
            });
            Some(v)
        }
        None => {
            finish(TraceEntry {
                source: source.to_string(),
                path: p,
                ms: t0.elapsed().as_millis(),
                ok: false,
                note: "无响应或不是有效 Node（已按上限终止）".to_string(),
            });
            None
        }
    }
}

fn summarize(c: &[(String, PathBuf)]) -> String {
    let recorded = c.iter().filter(|(s, _)| s == "记录").count();
    let known = c.iter().filter(|(s, _)| s == "已知").count();
    let path = c.iter().filter(|(s, _)| s == "PATH").count();
    format!("候选 {} 个（记录 {} / 已知 {} / PATH {}）", c.len(), recorded, known, path)
}


/// PATH 目录，逐条 stage 并做本地盘过滤（过滤本身也可能阻塞 —— 见 env.rs 的盘符缓存）。
fn path_dirs_staged() -> Vec<PathBuf> {
    /// PATH 条目上限：极端长的 PATH 不应把探测拖成分钟级。
    const MAX_PATH_ENTRIES: usize = 64;
    stage("③ 过滤 PATH（跳过网络盘/UNC）");
    let mut v = crate::env::path_dirs_local_only();
    if v.len() > MAX_PATH_ENTRIES {
        v.truncate(MAX_PATH_ENTRIES);
    }
    v
}

/// 已知安装落点（**平台判定**）。
///
/// 实现已下沉到 platform 层（2026-09-11）：
///   · Unix —— /usr/local、/opt/homebrew、/usr/bin + volta/nvm/fnm 布局
///   · Windows —— ProgramFiles(x86)、Chocolatey、scoop、volta、nvm 三种布局
///
/// ⚠ Windows 不得硬编码 `C:\Program Files`：真实路径随**系统盘符**与
///   **系统语言**变化（中文系统是本地化目录名），故一律经环境变量推导。
///
/// ⚠ 本函数会做 `read_dir`（版本管理器布局需要枚举版本目录），可能落在
///   漫游配置/慢速盘上 —— 调用方必须先 `stage()` 上报。
fn known_locations() -> Vec<(String, PathBuf)> {
    crate::platform::current()
        .node_candidate_paths()
        .into_iter()
        .map(|p| ("已知".to_string(), p))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 这两个用例共享全局注入（挂起标志 + 硬上限），必须**串行**执行，
    /// 否则并行时一个用例的注入会污染另一个（初次运行即遇到该问题）。
    static SERIAL: Mutex<()> = Mutex::new(());

    /// 核心性质：**枚举阶段永久阻塞时，必须给出带阶段信息的明确结论**。
    ///
    /// 这直接对应线上故障：卡在枚举（GetDriveTypeW / read_dir），
    /// 而 summary / stuck / trace 三项全空 —— 用户只看到「卡住且不报错」。
    #[test]
    fn hard_deadline_yields_actionable_failure_when_enumeration_hangs() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        set_hard_deadline_ms(500);
        invalidate();
        set_hang_in_enumerate(true);

        let t0 = Instant::now();
        // 第一次：仍在进行，但**阶段必须可见**（规则一已生效）
        let first = status(Duration::from_millis(30));
        assert!(!first.finished, "首查不应立刻结束");
        let stuck = current_stuck();
        assert!(stuck.is_some(), "卡住时必须能看到当前阶段（否则无从排障）");
        assert!(
            stuck.as_ref().unwrap().0.contains("模拟枚举"),
            "阶段名未反映真实位置: {:?}",
            stuck
        );

        // 持续轮询：应在硬上限附近得到**明确失败**，而不是永远 probing
        let mut last = first;
        while !last.finished && t0.elapsed() < Duration::from_secs(20) {
            std::thread::sleep(Duration::from_millis(50));
            last = status(Duration::from_millis(30));
        }
        assert!(last.finished, "超过硬上限仍未给出结论 —— 这正是要消除的故障");
        let err = last.error.clone().expect("明确失败必须带原因");
        assert!(err.contains("未完成"), "原因应说明超限: {}", err);
        assert!(err.contains("模拟枚举"), "原因必须包含卡住的阶段: {}", err);
        assert!(!last.trace.is_empty(), "追踪不应为空（诊断串要用）");

        set_hang_in_enumerate(false);
        set_hard_deadline_ms(25_000);
        invalidate();
    }

    /// 注入未开启时，正常探测必须完成（确认测试钩子未污染正常路径）。
    #[test]
    fn normal_probe_completes() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        set_hang_in_enumerate(false);
        invalidate();
        let out = status(Duration::from_secs(30));
        assert!(out.finished, "正常探测应完成");
        assert!(out.error.is_none(), "正常探测不应报错: {:?}", out.error);
        invalidate();
    }
}

/// 把追踪渲染为多行文本（诊断 / CLI 自检用）。
pub fn render_trace(trace: &[TraceEntry]) -> String {
    let mut s = String::new();
    for e in trace {
        s.push_str(&format!("  [{}] {} {} ms ok={} {}\n", e.source, e.path, e.ms, e.ok, e.note));
    }
    s
}
