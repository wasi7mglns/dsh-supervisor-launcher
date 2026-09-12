// 内核（dsh-supervisor）版本治理 —— 引导期的「强制更新 + 失败回退」（方案 B）。
//
// 跨平台规范性（本模块的全部设计依据）：
//   1) 包名按 os/arch 映射：@dsh-sup/dsh-core-<linux|darwin|win>-<x64|arm64>。
//   2) npm 可执行文件：Windows = npm.cmd，其余 = npm。
//   3) 全局前缀从「已定位内核的真实路径」反推——**绝不用 npm prefix -g**：
//      nvm/自定义 prefix 下 npm prefix -g 与内核实际安装位置可能不一致（2026-09 实证：
//      prefix 报 nvm 路径，内核却在 ~/.npm-global），直接用 npm i -g 会装到别处、
//      旧内核继续遮蔽新内核（「更新了却没生效」）。
//   4) 镜像顺序尊重内核 registry.json（mode=manual 用 manualOrigin；否则 origins；缺失用内建默认）。
//   5) 版本比较取 dist-tags + versions 的**全量最高**（与内核 fetchNpmLatest 同语义）——
//      dist-tags.latest 可能落后于实际最高版本（实证：latest=0.1.1-BETA.1 而实际有 0.1.2-BETA.7），
//      只信 latest 会导致「强制更新」变「强制降级」。

use serde_json::Value;
// ⚠ 原 `use std::io::Read;` 已移除（2026-09-12）：read_log 改用 fs::read + from_utf8_lossy，
//   不再需要 Read trait（会触发 unused_imports 警告）。
use std::path::{Path, PathBuf};

/// 内建默认镜像（与内核 config.registries 同集合；registry.json 缺失时的兜底）。
// 与 mirror.rs 的 NPM_PRESETS / 内核 config.registries 保持同一集合。
// 这只是在 mirror.rs 配置损坏时的最后兜底；正常路径由 mirror::load() 提供。
const DEFAULT_ORIGINS: [&str; 6] = [
    "https://registry.npmmirror.com",
    "https://registry.npmjs.org",
    "https://repo.huaweicloud.com/repository/npm/",
    "https://mirrors.cloud.tencent.com/npm",
    "https://npmreg.proxy.ustclug.org",
    "https://r.cnpmjs.org",
];

/// 平台 → npm 子包名（唯一真源；错误提示/安装/查询共用，杜绝散落硬编码）。
pub fn package_name() -> Result<String, String> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "darwin",
        "windows" => "win",
        other => return Err(format!("不支持的平台: {}", other)),
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => return Err(format!("不支持的架构: {}", other)),
    };
    Ok(format!("@dsh-sup/dsh-core-{}-{}", os, arch))
}

/// npm 可执行名（Windows 需 .cmd 后缀）—— 下沉到 trait（P2/G1）。
pub fn npm_exe() -> &'static str {
    crate::platform::current().npm_exe_name()
}

fn num_ok(s: &str) -> bool {
    if s.is_empty() { return false; }
    if s.len() > 1 && s.starts_with('0') { return false; } // 禁止前导零（对齐 semver）
    s.chars().all(|c| c.is_ascii_digit())
}

/// 版本字面量合法性：`X.Y.Z[-pre][+build]`。
///
/// ## 2026-09-11 对齐 semver（修复与内核的 3 处分歧）
///
/// 旧实现 `v.split('+').next()` 在验证前**丢弃 build 段**，于是：
///   `1.0.0+`、`1.0.0+!!!`、`1.0.0+あ` 被判**合法**，而内核 `VERSION_RE` 判**非法**。
/// 两侧对同一输入给出不同答案 —— 正是「同一逻辑两处实现」的典型风险。
///
/// 按 semver 规范，build 段必须匹配 `[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*`，
/// 故上述三例**应当非法**（内核正确、本实现偏宽）。现对齐。
///
/// 行为规格由 `shell-release/version-vectors.json` 锁定（内核侧有同一份，
/// 两侧测试套件都按它断言）—— 跨语言无法共享代码，但可共享行为规格。
pub fn is_valid_version(v: &str) -> bool {
    let mut it = v.splitn(2, '+');
    let core = it.next().unwrap_or("");
    let build = it.next();

    // ── 主段 + 预发布段 ──
    let mut core_it = core.splitn(2, '-');
    let nums = core_it.next().unwrap_or("");
    let parts: Vec<&str> = nums.split('.').collect();
    if parts.len() != 3 || !parts.iter().all(|p| num_ok(p)) { return false; }
    if let Some(pre) = core_it.next() {
        if pre.is_empty() { return false; }
        for seg in pre.split('.') {
            if seg.is_empty() || !seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') { return false; }
        }
    }

    // ── build 段（**不再忽略**）：非空，且每段为 [0-9A-Za-z-]+ ──
    if let Some(b) = build {
        if b.is_empty() { return false; }
        for seg in b.split('.') {
            if seg.is_empty() || !seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') { return false; }
        }
    }
    true
}

/// semver 比较（与内核 semverCompare 同语义）：数值段优先；release > prerelease；
/// 预发布内「数字段 < 字符串段」；build metadata 不参与。返回 -1/0/1。
pub fn semver_cmp(a: &str, b: &str) -> i32 {
    let parse = |v: &str| -> (Vec<i64>, String) {
        let clean = v.split('+').next().unwrap_or("").to_string();
        let mut it = clean.splitn(2, '-');
        let core = it.next().unwrap_or("").to_string();
        let pre = it.next().unwrap_or("").to_string();
        (core.split('.').map(|x| x.parse::<i64>().unwrap_or(0)).collect(), pre)
    };
    let (an, ap) = parse(a);
    let (bn, bp) = parse(b);
    for i in 0..3 {
        let x = an.get(i).copied().unwrap_or(0);
        let y = bn.get(i).copied().unwrap_or(0);
        if x != y { return if x > y { 1 } else { -1 }; }
    }
    if ap == bp { return 0; }
    if ap.is_empty() { return 1; }  // release > prerelease
    if bp.is_empty() { return -1; }
    let av: Vec<&str> = ap.split('.').collect();
    let bv: Vec<&str> = bp.split('.').collect();
    let n = av.len().max(bv.len());
    for i in 0..n {
        match (av.get(i), bv.get(i)) {
            (None, _) => return -1,
            (_, None) => return 1,
            (Some(x), Some(y)) => {
                let xn = x.chars().all(|c| c.is_ascii_digit());
                let yn = y.chars().all(|c| c.is_ascii_digit());
                if xn && yn {
                    let xi: i64 = x.parse().unwrap_or(0);
                    let yi: i64 = y.parse().unwrap_or(0);
                    if xi != yi { return if xi > yi { 1 } else { -1 }; }
                } else if xn != yn {
                    return if xn { -1 } else { 1 }; // 数字段 < 字符串段
                } else if x != y {
                    return if x < y { -1 } else { 1 };
                }
            }
        }
    }
    0
}

/// 镜像候选集合（2026-09-11 重写）：**壳自持配置优先**，其次内核 registry.json，最后内建默认。
///
/// 为什么壳配置优先：装机时**没有内核**（registry.json 尚不存在），壳必须自带镜像能力；
/// 而壳在引导阶段选出的最快源若能被内核继承，就不必让内核再盲选一次。
/// 若内核已进入 manual 模式（用户在面板里手动锁定），则**尊重内核的选择**。
pub fn registry_origins() -> Vec<String> {
    let path = crate::env::supervisor_dir().join("registry.json");
    let mut kernel_manual: Option<String> = None;
    let mut kernel_list: Option<Vec<String>> = None;
    if let Ok(s) = std::fs::read_to_string(&path) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            let mode = v.get("mode").and_then(|x| x.as_str()).unwrap_or("auto");
            if mode == "manual" {
                if let Some(m) = v.get("manualOrigin").and_then(|x| x.as_str()) {
                    if !m.is_empty() { kernel_manual = Some(m.to_string()); }
                }
            }
            if let Some(arr) = v.get("origins").and_then(|x| x.as_array()) {
                let list: Vec<String> = arr.iter().filter_map(|x| x.as_str())
                    .map(|s| s.to_string()).filter(|s| !s.is_empty()).collect();
                if !list.is_empty() { kernel_list = Some(list); }
            }
        }
    }
    // 1) 内核手动模式：最高优先（用户显式选择）
    if let Some(m) = kernel_manual {
        return vec![m];
    }
    // 2) 壳自持配置（引导阶段已测速选择）
    let m = crate::mirror::load();
    if !m.npm.is_empty() {
        return m.npm;
    }
    // 3) 内核 registry.json 的 auto 列表
    if let Some(l) = kernel_list {
        return l;
    }
    // 4) 内建默认
    DEFAULT_ORIGINS.iter().map(|s| s.to_string()).collect()
}

/// 包名 URL 编码：scope 的 / 编码为 %2F（npm registry 两种写法均可，编码更稳）。
fn encode_pkg(pkg: &str) -> String {
    pkg.chars().map(|c| if c == '/' { "%2F".to_string() } else { c.to_string() }).collect()
}

/// 最新版本：**并行**探测全部镜像，取全量最高版本，并选最快且提供该版本的源。
/// 返回 (version, 命中镜像)。
///
/// 2026-09-11 重写：原实现是「串行、首个成功即返回」—— 既慢（慢源拖死整体），
/// 又可能因某个镜像**元数据滞后**而选到旧版本（镜像同步存在延迟）。
/// 现改为并行 + 跨源取最高，与 Node 侧策略一致。
pub fn latest_version(pkg: &str) -> Result<(String, String), String> {
    if pkg.is_empty() { return Err("包名为空".into()); }
    let path = encode_pkg(pkg);
    let origins = registry_origins();
    let probes = crate::mirror::probe_all(&origins, &path);

    // 跨全部可达源取最高版本；同版本时保留延迟最低者。
    let mut best: Option<(String, u128, String)> = None; // (version, latency, source)
    let mut reachable = 0usize;
    for p in &probes {
        if !p.ok { continue; }
        reachable += 1;
        let Some(body) = &p.body else { continue };
        let Ok(j) = serde_json::from_str::<Value>(body) else { continue };
        let mut local: Option<String> = None;
        let mut consider = |v: &str| {
            if !is_valid_version(v) { return; }
            let better = local.as_deref().map(|b| semver_cmp(v, b) > 0).unwrap_or(true);
            if better { local = Some(v.to_string()); }
        };
        if let Some(obj) = j.get("dist-tags").and_then(|x| x.as_object()) {
            for v in obj.values() { if let Some(s) = v.as_str() { consider(s); } }
        }
        if let Some(obj) = j.get("versions").and_then(|x| x.as_object()) {
            for k in obj.keys() { consider(k); }
        }
        let Some(v) = local else { continue };
        let better = match &best {
            None => true,
            Some((bv, _, _)) => semver_cmp(&v, bv) > 0,
        };
        if better {
            best = Some((v, p.latency_ms, p.source.clone()));
        }
    }

    match best {
        Some((version, _, source)) => Ok((version, source)),
        None => {
            let detail = probes
                .iter()
                .map(|p| format!("{}:{}", p.source, if p.ok { format!("{}ms", p.latency_ms) } else { "不可达".into() }))
                .collect::<Vec<_>>()
                .join(", ");
            Err(format!("全部镜像不可用或均无该包（可达 {} 个；{}）", reachable, detail))
        }
    }
}

/// 无 GUI 场景下定位内核可执行文件（与 main.rs 的 locate_core 同一候选集）。
/// 供 --service-plan 等 CLI 自检使用：它们没有 AppHandle。
pub fn locate_core_for_cli() -> Option<std::path::PathBuf> {
    // CLI 无 AppHandle → 不提供资源目录兜底（那是 GUI 形态的最后一层）
    crate::domain::coreloc::locate_core_candidates(None)
        .into_iter()
        .find(|p| p.is_file())
}

/// 内核包目录（<pkg>/bin/<exe> → <pkg>）。
fn package_dir_of(bin: &Path) -> Option<PathBuf> {
    let bin_dir = bin.parent()?;              // <pkg>/bin
    if bin_dir.file_name().and_then(|s| s.to_str()) != Some("bin") { return None; }
    bin_dir.parent().map(|p| p.to_path_buf()) // <pkg>
}

/// 已安装版本：优先读包内 package.json（无进程开销、跨平台一致）；兜底执行 --version。
pub fn installed_version(bin: &Path) -> Option<String> {
    if let Some(dir) = package_dir_of(bin) {
        if let Ok(s) = std::fs::read_to_string(dir.join("package.json")) {
            if let Ok(v) = serde_json::from_str::<Value>(&s) {
                if let Some(ver) = v.get("version").and_then(|x| x.as_str()) {
                    if is_valid_version(ver) { return Some(ver.to_string()); }
                }
            }
        }
    }
    // 兜底：执行 --version。
    // ⚠ 必须有界（2026-09-11 修复，与「检测环境卡死」同一类缺陷）：
    //   原实现用 Command::output() **无限阻塞**，且 locate_core 会对**每个候选**都调用一次；
    //   一旦某个候选不可执行（损坏的 shim、被安全软件拦截、架构不符），
    //   引导页就会永久停在「正在检查内核版本」。
    let mut cmd = std::process::Command::new(bin);
    cmd.arg("--version");
    match run_command_bounded(cmd, VERSION_PROBE_TIMEOUT) {
        Ok(o) if o.success => parse_version_output(&o.stdout),
        _ => None,
    }
}

/// 内核 `--version` 探测的时间上限。
const VERSION_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// 从 --version 输出解析版本（形如 "dsh-supervisor v0.1.2-BETA.7"）。
pub fn parse_version_output(s: &str) -> Option<String> {
    for tok in s.split_whitespace() {
        let t = tok.trim().trim_start_matches('v');
        if is_valid_version(t) { return Some(t.to_string()); }
    }
    None
}

/// 从内核真实路径反推 npm 全局前缀（跨平台布局差异见下）。
///   Unix    : <prefix>/lib/node_modules/@scope/pkg/bin/exe  → <prefix>
///   Windows : <prefix>/node_modules/@scope/pkg/bin/exe      → <prefix>
pub fn global_prefix_for(bin: &Path) -> Option<PathBuf> {
    let comps: Vec<std::path::Component> = bin.components().collect();
    for i in 0..comps.len() {
        if comps[i].as_os_str() == std::ffi::OsStr::new("node_modules") {
            let is_lib = i >= 1 && comps[i - 1].as_os_str() == std::ffi::OsStr::new("lib");
            let cut = if is_lib { i - 1 } else { i };
            let mut p = PathBuf::new();
            for c in &comps[..cut] { p.push(c.as_os_str()); }
            if p.as_os_str().is_empty() { return None; }
            return Some(p);
        }
    }
    // Windows npm 垫片兜底（2026-09-11 修复）：
    //   路径形如 `%APPDATA%\npm\dsh-supervisor.cmd` —— **不含 node_modules 段**，
    //   故上面的循环返回 None，进而 install_version 丢失 --prefix，
    //   可能装到 npm 默认前缀而非内核当前所在前缀（旧内核遮蔽新内核）。
    //   判据：该目录直接含 node_modules 时，它本身就是 npm 全局前缀。
    if let Some(dir) = bin.parent() {
        if dir.join("node_modules").is_dir() {
            return Some(dir.to_path_buf());
        }
    }
    None
}

fn tail(s: &str, n: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= n { return t.to_string(); }
    t.chars().skip(t.chars().count() - n).collect()
}

/// 安装/升级/回退到指定版本（npm install -g [--prefix] pkg@version）。
/// 显式 --prefix 保证装回「内核当前所在前缀」，避免 npm 默认前缀不一致导致旧内核遮蔽新内核。
/// 返回 npm 输出（成功）或含退出码与 stderr 的错误（失败——供引导页如实呈现，不再吞错）。
pub fn install_version(pkg: &str, version: &str, prefix: Option<&Path>, registry: Option<&str>) -> Result<String, String> {
    if !is_valid_version(version) { return Err(format!("非法目标版本: {}", version)); }
    let spec = format!("{}@{}", pkg, version);
    let mut cmd = std::process::Command::new(npm_exe());
    cmd.args(["install", "-g", "--no-audit", "--no-fund"]).arg(&spec);
    if let Some(p) = prefix { cmd.arg("--prefix").arg(p); }
    if let Some(r) = registry { if !r.is_empty() { cmd.env("npm_config_registry", r); } }
    // CREATE_NO_WINDOW：GUI 进程调 npm 不弹控制台。
    // 经 bounded::prepare（**infra 原语**，与 bounded::run 同一处实现）——
    // 本文件因此不再需要平台分支（门禁 G1）。
    crate::bounded::prepare(&mut cmd);
    // ⚠ 必须有界（2026-09-11 修复，与引导页「网络步骤无超时 → 永久卡住」属同一类缺陷）：
    //   原实现用 `cmd.output()` **无限阻塞** —— npm 因网络停滞/registry 无响应而挂起时，
    //   引导页会永久停在「正在安装内核…」，用户除了杀进程别无选择。
    //   实现要点：输出重定向到**临时文件**而非管道 —— 若用 Stdio::piped() 且不读取，
    //   冗长的 npm 输出（npm 会打印大量进度）填满 OS 管道缓冲区（约 64KB）后子进程会阻塞，
    //   反而制造死锁。临时文件无此问题，且便于超时后保留现场。
    let out = run_command_bounded(cmd, NPM_INSTALL_TIMEOUT)?;
    if out.success { return Ok(tail(&out.stdout, 500)); }
    let code = out.code.unwrap_or_else(|| "killed".into());
    Err(format!("npm 退出码 {}：{}", code, tail(&out.stderr, 800)))
}

/// npm install 的时间上限。npm 在慢网下确实可能耗时数分钟，故给足预算；
/// 但绝不无限等待 —— 超时即杀进程并如实报错（引导页据此给出重试/回退）。
const NPM_INSTALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// 有界执行子进程 —— **委托给 `bounded.rs` 的统一实现**（P2 去重，2026-09-12）。
///
/// ⚠ 此处原有 `struct BoundedOutput` + 一份 `run_command_bounded` 的**完整复制**：
///   字段与 `bounded::Output` 逐一相同，逻辑也几乎逐行相同，但**行为已经分叉**：
///     · 漏 `cmd.stdin(Stdio::null())` —— 子进程会继承 GUI 进程的 stdin；
///     · 曾用 `as_millis()` 做临时名（并发撞名）而 bounded 一直用 nanos；
///     · `prepare()`（Windows CREATE_NO_WINDOW）只在 npm 路径手动调过，物探路径漏了 → 闪控制台。
///   这正是 `bounded.rs` 顶部「所有外部命令一律经它执行」被违反的又一例。
///
///   现统一走 `crate::bounded::run`：stdin/prepare/nanos/超时杀进程全部一致，
///   返回类型直接用 `bounded::Output`（字段本就相同，无需再定义一份）。
fn run_command_bounded(
    mut cmd: std::process::Command,
    timeout: std::time::Duration,
) -> Result<crate::bounded::Output, String> {
    crate::bounded::run(&mut cmd, timeout)
}


// ── 注：本文件原有的 `read_log` 已删除（2026-09-12 去重）。
//    它随 `run_command_bounded` 的复制体一起存在；统一到 `bounded.rs` 后成为死代码。
//    GBK/非 UTF-8 的容忍现由 `bounded.rs::read_log` 单点负责（B62 锁定）。


/// 计算规划：只有 latest 严格大于 installed 才需更新（**绝不降级**）。
pub fn build_plan(installed: Option<String>, latest: Result<(String, String), String>) -> Value {
    let (latest_v, origin, err) = match latest {
        Ok((v, o)) => (Some(v), Some(o), None),
        Err(e) => (None, None, Some(e)),
    };
    let action = match (&installed, &latest_v) {
        (None, Some(_)) => "install",
        (Some(i), Some(l)) => if semver_cmp(l, i) > 0 { "upgrade" } else { "none" },
        _ => "unknown",
    };
    serde_json::json!({
        "installed": installed,
        "latest": latest_v,
        "action": action,
        "updateAvailable": action == "upgrade",
        "registry": origin,
        "error": err,
    })
}

/// 无头自检输出（--core-plan 用，便于发布后冒烟验证，无需 GUI）。
pub fn plan_text() -> String {
    let pkg = match package_name() { Ok(p) => p, Err(e) => return format!("pkg_error={}", e) };
    let mut lines = vec![format!("package={}", pkg)];
    lines.push(format!("origins={}", registry_origins().join(",")));
    match latest_version(&pkg) {
        Ok((v, o)) => {
            lines.push(format!("latest={}", v));
            lines.push(format!("latest_origin={}", o));
        }
        Err(e) => lines.push(format!("latest_error={}", e)),
    }
    lines.join(" | ")
}
#[cfg(test)]
mod tests {
    use super::*;

    /// 版本语义**共享测试向量**（2026-09-11）。
    ///
    /// ## 为什么需要它
    ///
    /// 壳（Rust）与内核（JS）各自实现版本校验/比较 —— **实测 3 处分歧**：
    /// `1.0.0+` / `1.0.0+!!!` / `1.0.0+あ` 壳判合法、内核判非法
    /// （旧壳现在验证前 `split('+')` 丢弃 build 段）。
    ///
    /// 跨语言无法共享代码，故共享**行为规格**：
    /// `shell-release/version-vectors.json`（内核仓有逐字节相同的一份）。
    /// 两侧测试套件都加载它并按自己的实现断言。
    ///
    /// 任何一侧改了语义而没同步 → 本测试失败。这是防止再次分叉的唯一可靠手段。
    ///
    /// `include_str!` 是**编译期**嵌入：文件缺失或路径错误会直接编译失败，
    /// 比运行时读取的门禁更强（不会因「文件恰好不在」而静默跳过）。
    const VECTORS: &str = include_str!("../../shell-release/version-vectors.json");

    /// 从形如 `{"input": "x", "valid": true}` 的对象体里取字符串字段。
    fn str_field(body: &str, key: &str) -> Option<String> {
        let pat = format!("\"{}\":", key);
        let after = body.split(&pat).nth(1)?;
        let mut it = after.split('"');
        it.next()?;
        Some(it.next()?.to_string())
    }

    /// 取布尔字段（只认 `"key": true`）。
    fn bool_field(body: &str, key: &str) -> Option<bool> {
        let pat = format!("\"{}\":", key);
        let after = body.split(&pat).nth(1)?;
        let v = after.trim_start();
        if v.starts_with("true") { Some(true) } else { Some(false) }
    }

    /// 取整数字段。
    fn int_field(body: &str, key: &str) -> Option<i32> {
        let pat = format!("\"{}\":", key);
        let after = body.split(&pat).nth(1)?;
        let digits: String = after.trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '-')
            .collect();
        digits.parse().ok()
    }

    /// 把 JSON 文本里的**每个**对象块（`{` … `}`）都取出来。
    ///
    /// 实现：栈记录每个 `{` 的起始位置，遇 `}` 弹出即得一个完整对象。
    /// 随后按**大小**过滤掉根对象（根对象包住整份文件，必然最长）——
    /// 只留逐条向量的小对象。
    ///
    /// 已知足够：向量文件里字符串不含花括号（数据由本仓维护）。
    fn object_bodies(raw: &str) -> Vec<String> {
        let mut all = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        for (i, c) in raw.char_indices() {
            match c {
                '{' => stack.push(i),
                '}' => {
                    if let Some(s) = stack.pop() {
                        all.push(raw[s..=i].to_string());
                    }
                }
                _ => {}
            }
        }
        // 根对象 = 唯一「包住整份文件」的那个（长度接近全文）——过滤掉。
        let limit = raw.len() / 2;
        all.into_iter().filter(|b| b.len() < limit).collect()
    }

    #[test]
    fn shared_version_vectors_hold() {
        let bodies = object_bodies(VECTORS);
        assert!(
            bodies.len() >= 20,
            "向量块数异常：{}（模板可能被破坏）",
            bodies.len()
        );

        let (mut nv, mut nc) = (0, 0);
        for b in &bodies {
            if b.contains("\"input\"") {
                let input = str_field(b, "input").expect("input 缺失");
                let valid = bool_field(b, "valid").expect("valid 缺失");
                let got = is_valid_version(&input);
                assert_eq!(
                    got, valid,
                    "版本合法性分歧：{:?} → 本实现 {}，向量期望 {}",
                    input, got, valid
                );
                nv += 1;
            } else if b.contains("\"expected\"") {
                let a = str_field(b, "a").expect("a 缺失");
                let bq = str_field(b, "b").expect("b 缺失");
                let exp = int_field(b, "expected").expect("expected 缺失");
                let got = semver_cmp(&a, &bq);
                assert_eq!(
                    got, exp,
                    "版本比较分歧：{:?} vs {:?} → 本实现 {}，向量期望 {}",
                    a, bq, got, exp
                );
                nc += 1;
            }
        }
        assert!(nv >= 15, "合法性向量过少：{}", nv);
        assert!(nc >= 8, "比较向量过少：{}", nc);
        eprintln!("版本向量通过：合法性 {} 条 / 比较 {} 条", nv, nc);
    }
}