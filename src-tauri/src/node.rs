use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

// 镜像候选已收敛到 mirror.rs 的 NODE_PRESETS（壳自持配置，支持用户自定义）。

/// 整个请求的时间上限。
///
/// ⚠ 必须足够大（2026-09-11 修复）：ureq 的 timeout 覆盖**整次调用**（含响应体读取），
///   而 Node 安装包体积为 30-90 MB（macOS .pkg 实测 89.4 MB）。原值 60 秒在网络稍慢时
///   必然超时 —— 表现为「运行环境」步骤失败且看似网络问题，实为超时配置过小。
const HTTP_TOTAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15 * 60);

fn http_get_bytes(url: &str) -> Result<Vec<u8>, String> {
    let resp = ureq::get(url)
        .timeout(HTTP_TOTAL_TIMEOUT)
        .call()
        .map_err(|e| format!("下载失败 {}: {}", url, e))?;
    let mut buf = Vec::new();
    resp.into_reader().read_to_end(&mut buf).map_err(|e| format!("读取响应失败: {}", e))?;
    Ok(buf)
}

/// index.json `files[]` 里的平台标签 —— **必须与 `platform_file()` 选定的产物语义一致**。
///
/// ⚠ macOS 标签语义修正（2026-09-11，实测驱动）：
///   原实现 arm64 返回 `osx-arm64-tar`、x64 返回 `osx-x64-tar`，而 `platform_file()`
///   返回的是官方 **`.pkg`** —— 判定依据（tar 标签）与下载对象（pkg）**不是一回事**。
///   它能工作只是因为两者恰好都存在，属**侥幸而非正确**。
///
///   实测（逐版本核对官方 index.json 的 files[]）：
///     · `osx-x64-pkg`   —— 所有 LTS 版本**都存在**（这就是通用 pkg 的标签）
///     · `osx-arm64-pkg` —— **从不存在**
///     · `osx-arm64-tar` / `osx-x64-tar` —— 存在，但那是 tarball 的标签
///   故 pkg 路径的正确标签是 `osx-x64-pkg`（适用于两种架构 —— 官方只发一个通用 pkg）。
///
///   补充实测证据：解包 `node-v24.21.0.pkg` 可见其 payload 同时含 x86_64 与 arm64
///   两个 Mach-O 切片（fat 二进制），即 .pkg 确为**通用包**，两种 Arch 都能原生安装。
/// 判定标签必须**按架构**给出，且与 `platform_file()` 的产物语义一致。
///
/// ⚠ Linux 也曾硬编码 `linux-x64`（2026-09-11 审计发现，与 macOS 同类）：
///   在 arm64 上 `platform_file()` 返回 `node-v{V}-linux-arm64.tar.xz`，
///   而标签却是 `linux-x64` —— 判定依据与产物不是一回事。
///   它能通过 `has` 检查只是因为「恰好 x64 标签也在 files[] 里」，
///   属**侥幸而非正确**：若某版本只有 x64 而无 arm64，代码仍会判定可用，
///   随后去下载一个不存在的 arm64 文件（404）。
///
/// 各平台实测口径：
///   Linux   : `linux-x64` / `linux-arm64`（files[] 两者均存在）
///   macOS   : `osx-x64-pkg`（官方**只发一个通用 pkg**，无 osx-arm64-pkg；实测确认）
///   Windows : `win-x64-msi`（官方**无 win-arm64-msi**，只有 win-arm64-7z/zip）
///
/// ⚠ 已知限制（诚实记录）：Windows arm64 上只能装 x64 msi（依赖系统模拟执行），
///   因为官方未提供 arm64 msi。若要原生 arm64，需改用 zip 解包路径 —— 当前未实现。
fn platform_tag() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        return if std::env::consts::ARCH == "aarch64" { "linux-arm64" } else { "linux-x64" };
    }
    #[cfg(target_os = "macos")]
    { return "osx-x64-pkg"; }
    #[cfg(target_os = "windows")]
    { return "win-x64-msi"; }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    { "linux-x64" }
}

/// 当前平台对应的官方发布文件名（version 不带 v，如 "22.2.0"）。
fn platform_file(version: &str) -> Option<String> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => return None,
    };
    #[cfg(target_os = "linux")]
    { Some(format!("node-v{}-linux-{}.tar.xz", version, arch)) }
    #[cfg(target_os = "macos")]
    {
        // ⚠ macOS 必须用官方 **.pkg**（2026-09-11 修复）：
        //   原实现下载 `node-v<ver>-darwin-<arch>.tar.gz`（tarball），却交给
        //   `installer -pkg` 执行 —— 格式不匹配，**必然安装失败**。
        //   官方提供的是通用 .pkg（`node-v<ver>.pkg`，arm64/x64 通用，见 index.json
        //   的 files 条目标签 `osx-*-tar` 仅表示 tarball 存在，与本路径无关）。
        //   选择 .pkg 而非 tarball 的原因：它带**签名与安装位置语义**，
        //   可用一次系统授权装到 /usr/local，与其它平台行为一致。
        let _ = arch;
        Some(format!("node-v{}.pkg", version))
    }
    #[cfg(target_os = "windows")]
    {
        // 官方**没有** win-arm64-msi（files[] 只有 win-arm64-7z / win-arm64-zip）。
        // 故 arm64 Windows 也取 x64 msi —— 依赖系统的 x64 模拟执行。这是**有意为之的
        // 折中**（原生 arm64 需改用 zip 解包，当前未实现），并在 platform_tag() 中如实记录。
        let _ = arch;
        Some(format!("node-v{}-x64.msi", version))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    { None }
}

/// 从一个 index.json 文本中解析「本平台可用的最高 LTS」。
fn best_from_index(raw: &str) -> Option<(String, String)> {
    let idx: Value = serde_json::from_str(raw).ok()?;
    let arr = idx.as_array()?;
    let mut best: Option<(String, String)> = None;
    for item in arr {
        let lts = item.get("lts");
        let is_lts = match lts { Some(v) => !v.is_null(), None => false };
        if !is_lts { continue; }
        let ver = item.get("version").and_then(|v| v.as_str()).unwrap_or("");
        if ver.is_empty() || !ver.starts_with('v') { continue; }
        let file = match platform_file(&ver[1..]) { Some(f) => f, None => continue };
        let tag = platform_tag();
        let has = item.get("files").and_then(|v| v.as_array())
            .map(|a| a.iter().any(|x| x.as_str().map(|s| s == file || (s == tag)).unwrap_or(false)))
            .unwrap_or(false);
        if !has { continue; }
        let newer = best.as_ref().map(|b| version_gt(ver, &b.0)).unwrap_or(true);
        if newer { best = Some((ver.to_string(), file)); }
    }
    best
}

/// 镜像发现结果：最高 LTS + 提供该版本的**最快**源。
pub struct LtsChoice {
    pub version: String,
    pub file: String,
    pub source: String,
    pub latency_ms: u128,
    pub probes: Vec<(String, bool, u128)>,
}

/// **并行**探测全部 Node 镜像，取「最高 LTS」并选最快且提供该版本的源。
///
/// 设计要点（2026-09-11 重写）：
///   · 旧实现是「官方优先、报错才回退」的**串行**逻辑（注释还错误地写着"并发"）：
///     nodejs.org 在受限网络下常常**可达但极慢**，于是永远不会回退到镜像，
///     而安装包有 30-90MB —— 用户要等很久甚至超时。
///   · 现改为并行探测所有候选（一次 index.json 同时得到延迟与版本，不额外增加请求），
///     再取**全部可达源中的最高版本** —— 这很重要：实测腾讯云镜像会滞后一个版本，
///     「首个成功即采用」会静默装到旧版。
///   · 最后在「提供该版本」的源中选**延迟最低**者下载，避免用慢源拉大包。
pub fn latest_lts() -> Result<LtsChoice, String> {
    let mirrors = crate::mirror::load();
    let probes = crate::mirror::probe_all(&mirrors.node, "index.json");
    let diag: Vec<(String, bool, u128)> = probes
        .iter()
        .map(|p| (p.source.clone(), p.ok, p.latency_ms))
        .collect();

    let mut best: Option<(String, String, u128, String)> = None; // (ver, file, latency, src)
    for p in &probes {
        if !p.ok { continue; }
        let Some(body) = &p.body else { continue };
        let Some((ver, file)) = best_from_index(body) else { continue };
        let better = match &best {
            None => true,
            Some((bv, _, _, _)) => version_gt(&ver, bv),
        };
        if better {
            best = Some((ver, file, p.latency_ms, p.source.clone()));
        }
    }
    match best {
        Some((version, file, latency_ms, source)) => {
            // 落盘缓存：记录选中的源与探测时间（TTL 内后续调用可直接复用；
            // 同时让镜像选择**可观测**——用户与排障都能看到当前用的是哪个源）。
            let mut m = crate::mirror::load();
            m.selected_node = Some(source.clone());
            m.checked_at = Some(crate::mirror::now_secs());
            if let Err(e) = crate::mirror::save(&m) {
                crate::update::log(&format!("镜像配置写入失败（不影响本次安装）: {}", e));
            }
            // 同步导出给内核（若内核已存在则继承同一偏好；不存在时也无害，
            // 内核首次安装后会读到这份文件）。
            if let Err(e) = crate::mirror::export_to_kernel(&m.npm) {
                crate::update::log(&format!("导出内核镜像偏好失败（不影响本次安装）: {}", e));
            }
            Ok(LtsChoice { version, file, source, latency_ms, probes: diag })
        }
        None => {
            let detail = diag
                .iter()
                .map(|(s, ok, ms)| format!("{}:{}", s, if *ok { format!("{}ms", ms) } else { "不可达".into() }))
                .collect::<Vec<_>>()
                .join(", ");
            Err(format!("全部 Node 镜像均不可用或无可用 LTS（{}）", detail))
        }
    }
}

fn version_gt(a: &str, b: &str) -> bool {
    let va: Vec<u64> = a.trim_start_matches('v').split('.').filter_map(|x| x.parse().ok()).collect();
    let vb: Vec<u64> = b.trim_start_matches('v').split('.').filter_map(|x| x.parse().ok()).collect();
    for i in 0..3 {
        let x = va.get(i).copied().unwrap_or(0);
        let y = vb.get(i).copied().unwrap_or(0);
        if x != y { return x > y; }
    }
    false
}




/// 下载 + SHASUMS256 强校验，返回本地文件路径。
///
/// 源顺序（2026-09-11 重写）：**优先使用发现阶段选出的最快源**（`preferred`），
/// 其余候选作为回退。校验失败（哈希不符）视为该源不可信，**换下一个源重试** ——
/// 这既保证正确性，也避免被单个镜像的损坏文件卡死。
pub fn download_verified(
    version: &str,
    file: &str,
    dl_dir: &Path,
    preferred: Option<&str>,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dl_dir).map_err(|e| e.to_string())?;
    let mirrors = crate::mirror::load();
    let mut order: Vec<String> = Vec::new();
    if let Some(p) = preferred {
        if !p.is_empty() {
            order.push(p.to_string());
        }
    }
    for s in &mirrors.node {
        if !order.iter().any(|x| x == s) {
            order.push(s.clone());
        }
    }
    let mut last_err: Option<String> = None;
    for base in &order {
        let base: &str = base.as_str();
        let file_url = format!("{}/{}/{}", base, version, file);
        let data = match http_get_bytes(&file_url) {
            Ok(d) => d,
            Err(e) => { last_err = Some(e); continue; }
        };
        let digest = hex::encode(Sha256::digest(&data));
        let sums = match String::from_utf8(http_get_bytes(&format!("{}/{}/SHASUMS256.txt", base, version)).map_err(|e| e)?) {
            Ok(s) => s,
            Err(e) => { last_err = Some(format!("SHASUMS 读取失败: {}", e)); continue; }
        };
        let expect = sums.lines().find_map(|l| {
            let l = l.trim();
            if l.ends_with(&format!("  {}", file)) {
                let h = l.split_whitespace().next().unwrap_or("");
                if h.len() == 64 { Some(h.to_string()) } else { None }
            } else { None }
        }).ok_or_else(|| format!("SHASUMS256.txt 中未找到条目 {}", file))?;
        if digest != expect {
            last_err = Some(format!("SHA256 校验失败：期望 {} 实得 {}（拒绝安装）", expect, digest));
            continue;
        }
        let dst = dl_dir.join(file);
        std::fs::write(&dst, &data).map_err(|e| e.to_string())?;
        return Ok(dst);
    }
    Err(last_err.unwrap_or_else(|| "下载失败".into()))
}

/// 安装命令的时间上限。
///
/// 这类命令会弹出系统授权对话框（pkexec / osascript / UAC），**必须等用户操作**，
/// 故预算要给足（用户可能需要一两分钟输入密码）。
/// 但**不能无界**：在无图形会话、策略禁弹窗、对话框被其它窗口遮挡等环境下，
/// 进程可能永不返回 —— 原实现用 `.status()`/`.output()`，会让安装线程永久悬挂，
/// 用户永远停在「正在安装运行环境…」。
const INSTALL_CMD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// 平台安装：官方产物 + 一次性系统授权弹窗。
pub fn install(file: &Path) -> Result<PathBuf, String> {
    let abs = file.canonicalize().map_err(|e| e.to_string())?;
    #[cfg(target_os = "linux")]
    {
        let cmd = format!("tar -xJf '{}' -C /usr/local --strip-components=1", abs.display());
        let out = crate::bounded::run(
            Command::new("pkexec").args(["sh", "-c", &cmd]),
            INSTALL_CMD_TIMEOUT,
        )
        .map_err(|e| format!("无法启动 pkexec（{}）。请确认系统已安装 pkexec（policykit）。", e))?;
        if !out.success {
            return Err(format!("pkexec 退出码 {}（用户取消或安装失败）：{}", out.code.unwrap_or_else(|| "killed".into()), out.stderr.trim()));
        }
        let node = PathBuf::from("/usr/local/bin/node");
        if !node.is_file() { return Err("安装完成但 /usr/local/bin/node 未就位".into()); }
        return Ok(node);
    }
    #[cfg(target_os = "macos")]
    {
        let esc = abs.display().to_string().replace('"', "\"");
        let script = format!("do shell script \"installer -pkg '{}' -target /\" with administrator privileges", esc);
        let out = crate::bounded::run(
            Command::new("osascript").arg("-e").arg(&script),
            INSTALL_CMD_TIMEOUT,
        )
        .map_err(|e| format!("无法启动 osascript: {}", e))?;
        if !out.success {
            return Err(format!("macOS 安装失败（用户取消或 installer 报错）: {}", out.stderr.trim()));
        }
        return Ok(PathBuf::from("/usr/local/bin/node"));
    }
    #[cfg(target_os = "windows")]
    {
        let esc = abs.display().to_string().replace('\'', "''");
        let ps = format!("Start-Process -FilePath msiexec -ArgumentList '/i','{}','/qn','/norestart' -Verb RunAs -Wait", esc);
        let out = crate::bounded::run(
            Command::new("powershell").args(["-NoProfile", "-Command", &ps]),
            INSTALL_CMD_TIMEOUT,
        )
        .map_err(|e| format!("无法启动 msiexec: {}", e))?;
        if !out.success {
            return Err(format!("Windows 安装失败（用户取消或 msiexec 报错）：{}", out.stderr.trim()));
        }
        return Ok(PathBuf::from(r"C:\Program Files\nodejs\node.exe"));
    }
    #[allow(unreachable_code)]
    Err("当前平台暂不支持自动安装 Node.js".into())
}

/// 是否需要升级：未安装（None）或已装版本低于最新 LTS。
pub fn outdated(installed: Option<&str>, latest: &str) -> bool {
    match installed {
        None => true,
        Some(v) => version_gt(latest, v),
    }
}

/// DSH 运行最低 Node 门槛（commander 要求 Node >= 22.12.0，2026-09 核实）。
/// 引导策略：达到最低标准即放行（不要求最新 LTS）——旧于最新但 >= 门槛直接进后续。
pub const MIN_NODE: &str = "v22.12.0";

/// 是否达到 DSH 最低 Node 要求：None（未装）→ false；已装 → 版本 >= MIN_NODE。
pub fn meets_minimum(installed: Option<&str>) -> bool {
    match installed {
        None => false,
        Some(v) => !version_gt(MIN_NODE, v),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn version_compare() {
        assert!(version_gt("v2.0.0", "v1.9.9"));
        assert!(version_gt("v22.25.0", "v22.24.0"));
        assert!(!version_gt("v1.9.9", "v2.0.0"));
        assert!(!version_gt("v22.24.0", "v22.24.0"));
    }
    #[test]
    fn outdated_logic() {
        assert!(outdated(None, "v26.8.1"));
        assert!(outdated(Some("v26.7.0"), "v26.8.1"));
        assert!(!outdated(Some("v26.8.1"), "v26.8.1"));
        assert!(!outdated(Some("v27.0.0"), "v26.8.1"));
    }
    #[test]
    fn meets_minimum_logic() {
        assert!(!meets_minimum(None));
        assert!(!meets_minimum(Some("v18.20.0")));
        assert!(!meets_minimum(Some("v21.0.0")));
        assert!(meets_minimum(Some("v22.12.0")));
        assert!(meets_minimum(Some("v22.25.0")));
        assert!(meets_minimum(Some("v26.7.0")));
        assert!(meets_minimum(Some("v27.0.0")));
    }
}

/// 安装后复探（PATH 优先，其次已知落点）。
pub fn probe_after() -> Option<(PathBuf, String)> {
    if let Some((p, v)) = crate::env::probe_system_node() { return Some((p, v)); }
    crate::env::known_install_node_path().and_then(|p| crate::env::node_version(&p).map(|v| (p, v)))
}

/// 写运行时纪要（版本/路径/时间）供面板透明展示。
pub fn record_runtime_meta(node_path: &str, version: &str) {
    let dir = crate::env::supervisor_dir();
    let _ = std::fs::create_dir_all(&dir);
    let meta = serde_json::json!({
        "nodeVersion": version,
        "nodePath": node_path,
        "installedAt": now_iso(),
        "source": "official-lts",
    });
    let _ = std::fs::write(dir.join("runtime.json"), serde_json::to_string_pretty(&meta).unwrap_or_default());
}

/// 当前 UTC 时间，ISO 8601（`YYYY-MM-DDTHH:MM:SSZ`）。
///
/// == 架构修复（2026-09-11）==
///
/// 旧实现在 Unix 上**执行外部 `date` 进程**取时间，且 Windows 分支**直接返回空串**。
/// 两个问题：
///   ① 为一个纯计算的值去 spawn 进程 —— 无界（原用 `.output()`）、且可能不存在；
///   ② Windows 上写空值，使 `installedAt` 在 Windows 丢失，跨平台行为不一致。
/// 现改为纯 std 计算（Howard Hinnant 的 civil-from-days 算法），三平台一致、无副作用。
/// 格式与内核侧 `new Date().toISOString()` 同族，下游（仅展示）可直接解析。
pub fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, m, d) = civil_from_days(days);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, m, d, h, mi, s)
}

/// 把「自 1970-01-01 起的天数」转为 (年, 月, 日)。
/// 算法来源：Howard Hinnant 的 `civil_from_days`（公有领域，已被广泛验证）。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as i64;                       // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);          // [0, 365]
    let mp = (5 * doy + 2) / 153;                               // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;              // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;       // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}
