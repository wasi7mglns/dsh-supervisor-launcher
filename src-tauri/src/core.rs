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
use std::io::Read;
use std::path::{Path, PathBuf};

/// 内建默认镜像（与内核 config.registries 同集合；registry.json 缺失时的兜底）。
const DEFAULT_ORIGINS: [&str; 4] = [
    "https://registry.npmmirror.com",
    "https://registry.npmjs.org",
    "https://mirrors.cloud.tencent.com/npm",
    "https://repo.huaweicloud.com/repository/npm/",
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

/// npm 可执行名（Windows 需 .cmd 后缀）。
pub fn npm_exe() -> &'static str {
    if cfg!(windows) { "npm.cmd" } else { "npm" }
}

fn num_ok(s: &str) -> bool {
    if s.is_empty() { return false; }
    if s.len() > 1 && s.starts_with('0') { return false; } // 禁止前导零（对齐 semver）
    s.chars().all(|c| c.is_ascii_digit())
}

/// 版本字面量合法性（对齐内核 VERSION_RE 的语义子集）：X.Y.Z[-pre]（忽略 +build）。
pub fn is_valid_version(v: &str) -> bool {
    let core = v.split('+').next().unwrap_or("");
    let mut it = core.splitn(2, '-');
    let nums = it.next().unwrap_or("");
    let parts: Vec<&str> = nums.split('.').collect();
    if parts.len() != 3 || !parts.iter().all(|p| num_ok(p)) { return false; }
    if let Some(pre) = it.next() {
        if pre.is_empty() { return false; }
        for seg in pre.split('.') {
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

/// 镜像候选：优先内核 registry.json（mode/origins/manualOrigin），缺失用内建默认。
pub fn registry_origins() -> Vec<String> {
    let path = crate::env::supervisor_dir().join("registry.json");
    if let Ok(s) = std::fs::read_to_string(&path) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            let mode = v.get("mode").and_then(|x| x.as_str()).unwrap_or("auto");
            if mode == "manual" {
                if let Some(m) = v.get("manualOrigin").and_then(|x| x.as_str()) {
                    if !m.is_empty() { return vec![m.to_string()]; }
                }
            }
            if let Some(arr) = v.get("origins").and_then(|x| x.as_array()) {
                let list: Vec<String> = arr.iter().filter_map(|x| x.as_str())
                    .map(|s| s.to_string()).filter(|s| !s.is_empty()).collect();
                if !list.is_empty() { return list; }
            }
        }
    }
    DEFAULT_ORIGINS.iter().map(|s| s.to_string()).collect()
}

fn http_json(url: &str, timeout_ms: u64) -> Result<Value, String> {
    let resp = ureq::get(url)
        .timeout(std::time::Duration::from_millis(timeout_ms))
        .call()
        .map_err(|e| format!("{}", e))?;
    let mut buf = Vec::new();
    resp.into_reader().read_to_end(&mut buf).map_err(|e| format!("读取响应失败: {}", e))?;
    serde_json::from_slice(&buf).map_err(|e| format!("JSON 解析失败: {}", e))
}

/// 包名 URL 编码：scope 的 / 编码为 %2F（npm registry 两种写法均可，编码更稳）。
fn encode_pkg(pkg: &str) -> String {
    pkg.chars().map(|c| if c == '/' { "%2F".to_string() } else { c.to_string() }).collect()
}

/// 最新版本：逐镜像尝试，取包元数据里 dist-tags + versions 的**全量最高**。
/// 返回 (version, 命中镜像)。
pub fn latest_version(pkg: &str) -> Result<(String, String), String> {
    if pkg.is_empty() { return Err("包名为空".into()); }
    let mut last_err = String::from("无候选镜像");
    for origin in registry_origins() {
        let base = origin.trim_end_matches('/');
        let url = format!("{}/{}", base, encode_pkg(pkg));
        match http_json(&url, 12000) {
            Ok(j) => {
                let mut best: Option<String> = None;
                let consider = |v: &str, best: &mut Option<String>| {
                    if !is_valid_version(v) { return; }
                    let better = best.as_deref().map(|b| semver_cmp(v, b) > 0).unwrap_or(true);
                    if better { *best = Some(v.to_string()); }
                };
                if let Some(obj) = j.get("dist-tags").and_then(|x| x.as_object()) {
                    for v in obj.values() { if let Some(s) = v.as_str() { consider(s, &mut best); } }
                }
                if let Some(obj) = j.get("versions").and_then(|x| x.as_object()) {
                    for k in obj.keys() { consider(k, &mut best); }
                }
                match best {
                    Some(b) => return Ok((b, origin.clone())),
                    None => last_err = format!("{} 无可用版本", origin),
                }
            }
            Err(e) => last_err = format!("{}: {}", origin, e),
        }
    }
    Err(format!("全部镜像不可用（{}）", last_err))
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
    let out = std::process::Command::new(bin).arg("--version").output().ok()?;
    if !out.status.success() { return None; }
    parse_version_output(&String::from_utf8_lossy(&out.stdout))
}

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
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW：GUI 进程调 npm 不弹控制台
    }
    let out = cmd.output().map_err(|e| format!("npm 启动失败: {}（Node 就绪后才能安装内核）", e))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if out.status.success() { return Ok(tail(&stdout, 500)); }
    let code = out.status.code().map(|c| c.to_string()).unwrap_or_else(|| "killed".into());
    Err(format!("npm 退出码 {}：{}", code, tail(&stderr, 800)))
}


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
