//! 镜像源适配（**壳自持**，2026-09-11）。
//!
//! == 为什么必须在壳内做（用户指正） ==
//!
//! 用户装机的那一刻，机器上**没有内核** —— 内核是随后由壳自己安装的。
//! 因此壳的每一处下载（Node 运行时、内核 npm 包、壳自身更新）都**不能**依赖
//! 内核的 registry.json（那时它还不存在）。镜像适配必须由壳自带。
//!
//! == 三条下载链路 ==
//!   ① Node 运行时   —— node.rs  （index.json + 安装包 + SHASUMS256）
//!   ② 内核 npm 包   —— core.rs  （包元数据 + npm install -g）
//!   ③ 壳自更新      —— main.rs  （Tauri updater 清单与安装包）
//!
//! == 策略：并行测速 + 缓存 + 取最高版本 ==
//!
//! 1. **并行**探测全部候选（串行会让最慢的源拖死整体；旧实现名为"并发"实为串行）；
//! 2. Node 版本发现**取全部可达源中的最高版本** —— 实测腾讯云镜像会**滞后一个版本**，
//!    若"首个成功即采用"会静默装到旧版；
//! 3. 选择**最快**且确实提供该版本的源做下载；
//! 4. 结果写入 ~/.dsh/shell/mirrors.json 缓存（TTL），并**导出给内核**
//!    （registry.json）以便后续继承同一份镜像偏好。
//!
//! == 为什么探测 Node 用 index.json ==
//! 我们无论如何都要拉它来发现版本，故一次并行拉取同时得到「延迟」与「版本」，
//! 不额外增加请求。

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// npm registry 预设（**全部经真实 tarball 下载验证**，2026-09-11）。
///
/// 验证方法：请求该源包元数据 → 取 dist.tarball → **真实下载** → 确认体积合理。
/// 仅「元数据可读」不足以判定可用（部分镜像只代理元数据、不代理 tarball）。
///
/// 已排除（实测不可用）：
///   · mirrors.aliyun.com/npm            —— 元数据不可用（非标准 registry 路径）
///   · mirrors.tuna.tsinghua.edu.cn/npm  —— 同上
pub const NPM_PRESETS: [&str; 6] = [
    "https://registry.npmmirror.com",
    "https://registry.npmjs.org",
    "https://repo.huaweicloud.com/repository/npm/",
    "https://mirrors.cloud.tencent.com/npm",
    "https://npmreg.proxy.ustclug.org",
    "https://r.cnpmjs.org",
];

/// Node 发行镜像预设（**全部经真实下载 + SHA256 校验验证**，2026-09-11）。
///
/// 验证方法：对每个源，取它自己 index.json 里**最高的可用 LTS**，下载安装包，
/// 并用该源 SHASUMS256.txt 的期望值做 SHA256 校验 —— 只有校验通过才算可用。
/// 这比「URL 可达」严格得多：能过滤代理不完整、文件损坏、清单与文件不匹配的镜像。
///
/// ⚠ 各源同步进度不同（官方/npmmirror/华为/阿里/上海交大有最新 LTS；
///   腾讯云/南京大学滞后一版；清华/北外/北大滞后数版）。这不影响使用 ——
///   latest_lts() 会**跨全部可达源取最高版本**，再在提供该版本的源中选最快者；
///   滞后源仍可作为回退。
///
/// 已排除（实测 SHA256 校验失败）：mirrors.ustc.edu.cn/node
pub const NODE_PRESETS: [&str; 10] = [
    "https://nodejs.org/dist",
    "https://npmmirror.com/mirrors/node",
    "https://mirrors.huaweicloud.com/nodejs",
    "https://mirrors.aliyun.com/nodejs-release",
    "https://mirror.sjtu.edu.cn/nodejs-release",
    "https://mirrors.cloud.tencent.com/nodejs-release",
    "https://mirror.nju.edu.cn/nodejs-release",
    "https://mirrors.tuna.tsinghua.edu.cn/nodejs-release",
    "https://mirrors.bfsu.edu.cn/nodejs-release",
    "https://mirrors.pku.edu.cn/nodejs-release",
];

/// 壳自更新清单预设（Tauri updater 的 endpoints；此处存完整清单 URL）。
///
/// ⚠ 仅 2 个可用（2026-09-11 实测）：Tauri updater 需要一个**静态 JSON 文件**直链，
///   而多数 npm 镜像只提供 registry 元数据 API，不提供包内静态文件直链。
///   实测排除：npmmirror /files/ 路径返回 403；npm 官方不提供静态文件服务。
pub const SHELL_PRESETS: [&str; 2] = [
    "https://unpkg.com/@dsh-sup/shell-release@latest/shell-manifest.json",
    "https://cdn.jsdelivr.net/npm/@dsh-sup/shell-release@latest/shell-manifest.json",
];

/// 单次探测的总超时（元数据很小，8 秒足够；避免坏源拖慢整体）。
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 壳自持镜像配置（~/.dsh/shell/mirrors.json）。
#[derive(Clone)]
pub struct Mirrors {
    pub node: Vec<String>,
    pub npm: Vec<String>,
    pub shell: Vec<String>,
    pub selected_node: Option<String>,
    pub selected_npm: Option<String>,
    pub checked_at: Option<u64>,
}

impl Default for Mirrors {
    fn default() -> Self {
        Mirrors {
            node: NODE_PRESETS.iter().map(|s| s.to_string()).collect(),
            npm: NPM_PRESETS.iter().map(|s| s.to_string()).collect(),
            shell: SHELL_PRESETS.iter().map(|s| s.to_string()).collect(),
            selected_node: None,
            selected_npm: None,
            checked_at: None,
        }
    }
}

fn cfg_path() -> PathBuf {
    crate::update::state_dir().join("mirrors.json")
}

/// 读取壳镜像配置；缺失或损坏时返回内置预设（**绝不失败** —— 装机首启必须可用）。
pub fn load() -> Mirrors {
    let mut m = Mirrors::default();
    if let Ok(s) = std::fs::read_to_string(cfg_path()) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
            let take = |key: &str| -> Option<Vec<String>> {
                v.get(key).and_then(|x| x.as_array()).map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str())
                        .map(|x| x.to_string())
                        .filter(|x| !x.is_empty())
                        .collect::<Vec<_>>()
                })
            };
            if let Some(list) = take("node") {
                if !list.is_empty() {
                    m.node = list;
                }
            }
            if let Some(list) = take("npm") {
                if !list.is_empty() {
                    m.npm = list;
                }
            }
            if let Some(list) = take("shell") {
                if !list.is_empty() {
                    m.shell = list;
                }
            }
            if let Some(s2) = v.get("selectedNode").and_then(|x| x.as_str()) {
                m.selected_node = Some(s2.to_string());
            }
            if let Some(s2) = v.get("selectedNpm").and_then(|x| x.as_str()) {
                m.selected_npm = Some(s2.to_string());
            }
            m.checked_at = v.get("checkedAt").and_then(|x| x.as_u64());
        }
    }
    m
}

/// 写壳镜像配置（原子写）。
pub fn save(m: &Mirrors) -> Result<(), String> {
    let path = cfg_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建壳状态目录失败: {}", e))?;
    }
    let v = serde_json::json!({
        "node": m.node,
        "npm": m.npm,
        "shell": m.shell,
        "selectedNode": m.selected_node,
        "selectedNpm": m.selected_npm,
        "checkedAt": m.checked_at,
    });
    let body = serde_json::to_string_pretty(&v).unwrap_or_default();
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body + "\n").map_err(|e| format!("写入镜像配置失败: {}", e))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("提交镜像配置失败: {}", e))?;
    Ok(())
}

/// 契约版本（内核据此判断格式是否兼容）。
///
/// 变更历史：
///   1 —— 仅 `mode` / `origins` / `manualOrigin`（旧格式）
///   2 —— 增加 `catalog`（全集）/ `selected`（选择结果）/ `probe`（**探测规格**）
///
/// ⚠ 为什么要 `probe`：修复「两侧选源不一致」——
///   内核用 `/-/ping`、壳用真实包元数据，同一镜像测出的延迟可差 **6.7 倍**
///   （实测 ustclug 2613ms vs 389ms），导致内核选 huaweicloud、壳选 npmmirror ——
///   用户看到「面板显示一个源、实际用另一个」。把探测规格随契约投放，
///   内核照做即可得到**同一答案**。
pub const CONTRACT_SCHEMA: u64 = 2;

/// 导出镜像契约给内核（`~/.dsh/supervisor/registry.json`）。
///
/// ## 为什么由**壳**写（所有权）
///
/// 用户在装壳那一刻机器上**没有内核** —— 壳必须先于内核完成镜像选择
/// （否则连内核都装不上）。故目录与探测方法的所有权在壳，内核**消费产物**。
///
/// ## 与旧行为的两处关键差异
///
/// 1. **不再只写被选中的 `origins`** —— 改为写全集 `catalog` + `selected` + `probe`。
///    只写 origins 会导致：内核无法感知「壳测过哪些源」，且没有探测规格 → 方法分叉。
/// 2. **不再依赖 `latest_lts()` 成功** —— 旧实现只在 `node::latest_lts()` 内调用，
///    而该函数在「离线」或「全部 Node 镜像不可达」时返回 Err，**契约便完全不写**。
///    现由 `main.rs` 的 setup 在**壳启动时无条件调用**一次（见 `export_on_boot`）。
///
/// 保留：内核已写为 `manual`（用户在面板手动固定）时**不覆盖**用户选择。
pub fn export_to_kernel(m: &Mirrors) -> Result<(), String> {
    export_to_kernel_with(m, None)
}

/// 同 [`export_to_kernel`]，但可携带选中源的实测延迟（`selected.latencyMs`）。
pub fn export_to_kernel_with(m: &Mirrors, latency_ms: Option<u128>) -> Result<(), String> {
    if m.npm.is_empty() {
        return Ok(());
    }
    let path = crate::env::supervisor_dir().join("registry.json");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建内核状态目录失败: {}", e))?;
    }
    // 用户在内核面板手动固定过 → 不覆盖（尊重显式意图）。
    if let Ok(s) = std::fs::read_to_string(&path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
            if v.get("mode").and_then(|x| x.as_str()) == Some("manual") {
                return Ok(());
            }
        }
    }
    let selected = m.selected_npm.as_ref().map(|origin| {
        serde_json::json!({
            "origin": origin,
            "latencyMs": latency_ms,
            "checkedAt": m.checked_at,
        })
    });
    let v = serde_json::json!({
        "schema": CONTRACT_SCHEMA,
        "writtenBy": format!("shell@{}", env!("CARGO_PKG_VERSION")),
        "writtenAt": now_secs(),
        "mode": "auto",
        // 既有字段：保持向后兼容（旧内核只读这三项也能工作）
        "origins": m.npm,
        "manualOrigin": m.selected_npm.clone().unwrap_or_else(|| m.npm.first().cloned().unwrap_or_default()),
        // v2 新增：全集 + 选择结果 + 探测规格
        "catalog": m.npm,
        "selected": selected,
        "probe": {
            "kind": "package-metadata",
            "pathTemplate": npm_probe_path(),
            "timeoutMs": 6000,
        },
    });
    let body = serde_json::to_string_pretty(&v).unwrap_or_default();
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body + "\n").map_err(|e| format!("写入内核 registry.json 失败: {}", e))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("提交内核 registry.json 失败: {}", e))?;
    Ok(())
}

/// **壳启动时无条件导出契约**（修复「`latest_lts()` 失败 ⇒ 契约完全不写」）。
///
/// 时机：`main.rs` 的 `setup()` 中、`init_identity` 之后（此时 HOME/状态目录已就绪）。
/// 失败只记日志，**绝不阻断引导** —— 契约是增强，不是壳启动的前提。
pub fn export_on_boot() {
    let m = load();
    if let Err(e) = export_to_kernel(&m) {
        crate::update::log(&format!("启动导出镜像契约失败（不影响引导）: {}", e));
    }
}

/// 一次并行探测的结果。
pub struct Probe {
    pub source: String,
    pub ok: bool,
    pub latency_ms: u128,
    pub body: Option<String>,
}

/// npm registry 的探测探针包名（**必须是一个真实存在的包**）。
///
/// ⚠ 为什么不能用空路径或根路径（2026-09-11 修复）：
///   原实现对 npm 源传 `""`，实际请求 `https://<源>/`—— 而多数 registry 根路径返回
///   **404**（它们只服务包元数据 API）。于是**健康的源被判为「不可达」**：
///   实测腾讯云 npm 镜像连测 3 次均 HTTP 200、能正确返回我们的包，
///   却因为根路径 404 而在测速中显示「不可达」，进而被排除在选择之外。
///   这是**探测方法错误**，不是源失效 —— 会让壳无谓地少一个可用镜像。
///
/// 改用我们自己的平台包做探针：它是真实存在的包，且与最终用途一致。
fn npm_probe_path() -> String {
    // 探测用包：优先内核平台包（真实存在）；失败时退回一个必然存在的小包。
    crate::core::package_name().unwrap_or_else(|_| "@dsh-sup/dsh-core-linux-x64".to_string())
}

/// **并行**探测全部候选：对每个源请求 path，记录延迟与响应体。
///
/// 用 std::thread::scope（std 自带，无需新依赖）实现并发；
/// 单源超时 PROBE_TIMEOUT，整体耗时约为其中最慢者而非累加。
///
/// `path` 为空时视为 **npm registry 探测**：自动使用真实包名而非根路径。
pub fn probe_all(sources: &[String], path: &str) -> Vec<Probe> {
    // 空 path → npm 探测：用真实包名（根路径会 404，导致健康源被误判不可达）。
    let owned;
    let path = if path.is_empty() {
        owned = npm_probe_path();
        owned.as_str()
    } else {
        path
    };
    let out: Mutex<Vec<Probe>> = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for src in sources {
            let out = &out;
            scope.spawn(move || {
                let url = format!(
                    "{}/{}",
                    src.trim_end_matches("/"),
                    path.trim_start_matches("/")
                );
                let started = Instant::now();
                let mut probe = Probe {
                    source: src.clone(),
                    ok: false,
                    latency_ms: 0,
                    body: None,
                };
                match ureq::get(&url).timeout(PROBE_TIMEOUT).call() {
                    Ok(resp) => {
                        let mut buf = Vec::new();
                        use std::io::Read;
                        if resp.into_reader().read_to_end(&mut buf).is_ok() {
                            probe.latency_ms = started.elapsed().as_millis();
                            probe.ok = true;
                            probe.body = Some(String::from_utf8_lossy(&buf).into_owned());
                        }
                    }
                    Err(_) => {
                        probe.latency_ms = started.elapsed().as_millis();
                    }
                }
                out.lock().unwrap().push(probe);
            });
        }
    });
    let mut v = out.into_inner().unwrap_or_default();
    v.sort_by(|a, b| match (a.ok, b.ok) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.latency_ms.cmp(&b.latency_ms),
    });
    v
}

// ════════════════════════════════════════════════════════════════════
// 镜像「一等公民」：全程可见 + 预热缓存（2026-09-11 架构修复）
//
// == 问题（用户实测指出） ==
//
// 用户反馈「连镜像源都看不到，根本不会去选择镜像源」。查代码后确证：
//   `afterEnv` 只在「未装 Node」或「Node 版本过低」时才调 probeMirrorThen ——
//   **Node 达标的用户（主力用户）永远看不到镜像**，诊断串必然 mirror=none。
//   而 `core_plan` / `core_apply` 内部算了镜像源，却**完全不回传** ——
//   用户看着「正在检查内核版本」，无从知晓壳选了哪个源。
//
// == 修复思路 ==
//
// 镜像不是「下载 Node 的辅助」，而是**壳所有网络动作的基础设施**。
// 故把它提升为一等公民：
//   1) **预热**：引导开始即在后台并行测速（不阻塞任何步骤）；
//   2) **全程可读**：任何时刻查询都返回缓存结果（无网络 I/O）；
//   3) **写透诊断**：诊断串始终带 mirror / mirror_probes。
//
// 这样「有没有选镜像、选了谁、延迟多少」在任何情况下都是**事实**，而不是观感。
// ════════════════════════════════════════════════════════════════════

/// 预热结果缓存：`None` 表示尚未测速完成。
static WARM: std::sync::OnceLock<Mutex<Option<ProbeSnapshot>>> = std::sync::OnceLock::new();

fn warm() -> &'static Mutex<Option<ProbeSnapshot>> {
    WARM.get_or_init(|| Mutex::new(None))
}

/// 一次预热的结果快照（含逐源延迟，供诊断展示）。
#[derive(Clone)]
pub struct ProbeSnapshot {
    /// 选中的 Node 源（最快且提供目标版本者；预热阶段仅取最快可达）。
    pub node_best: Option<String>,
    pub node_latency_ms: Option<u128>,
    pub npm_best: Option<String>,
    pub npm_latency_ms: Option<u128>,
    pub npm_probes: Vec<(String, bool, u128)>,
    pub at: u64,
}

/// 读预热缓存（**无 I/O**，任何时刻可安全调用）。
pub fn cached() -> Option<ProbeSnapshot> {
    match warm().lock() {
        Ok(g) => g.clone(),
        Err(e) => e.into_inner().clone(),
    }
}

/// 是否已在飞（避免重复预热）。
static WARMING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 后台预热镜像测速：**立即返回**，结果稍后经 `cached()` 读取。
///
/// 设计要点：预热在独立线程中做全部网络 I/O，故调用方（引导页）**绝不阻塞**；
/// 这使「镜像全程可见」不再与「是否需要下载 Node」耦合。
pub fn warmup_async() {
    if WARMING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return; // 已有在飞预热
    }
    let _ = std::thread::Builder::new()
        .name("mirror-warmup".to_string())
        .spawn(|| {
            let m = load();
            // 两组并行探测（各自内部已并行）
            let node_p = probe_all(&m.node, "index.json");
            let npm_p = probe_all(&m.npm, "");
            // 选中的源 + 其延迟。
            let pick = |v: &[Probe]| -> (Option<String>, Option<u128>) {
                let best = v.iter().find(|p| p.ok);
                (best.map(|p| p.source.clone()), best.map(|p| p.latency_ms))
            };
            // 逐源明细：只给 npm（`ProbeSnapshot::npm_probes` 消费）；
            // node 侧的明细此前也返回，但**从未被读取** —— 2026-09-11 清理时去掉。
            let pick_all = |v: &[Probe]| -> Vec<(String, bool, u128)> {
                v.iter().map(|p| (p.source.clone(), p.ok, p.latency_ms)).collect()
            };
            let (nb, nl) = pick(&node_p);
            let (mb, ml) = pick(&npm_p);
            let mp = pick_all(&npm_p);
            let snap = ProbeSnapshot {
                node_best: nb,
                node_latency_ms: nl,
                npm_best: mb,
                npm_latency_ms: ml,
                npm_probes: mp,
                at: now_secs(),
            };
            if let Ok(mut g) = warm().lock() {
                *g = Some(snap);
            }
            WARMING.store(false, std::sync::atomic::Ordering::SeqCst);
        });
}
