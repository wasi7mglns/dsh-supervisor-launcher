use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

const OFFICIAL: &str = "https://nodejs.org/dist";
const MIRROR: &str = "https://npmmirror.com/mirrors/node";

fn http_get_bytes(url: &str) -> Result<Vec<u8>, String> {
    let resp = ureq::get(url)
        .timeout(std::time::Duration::from_secs(60))
        .call()
        .map_err(|e| format!("下载失败 {}: {}", url, e))?;
    let mut buf = Vec::new();
    resp.into_reader().read_to_end(&mut buf).map_err(|e| format!("读取响应失败: {}", e))?;
    Ok(buf)
}

/// index.json files[] 里的平台标签（如 linux-x64 / osx-arm64-tar / win-x64-msi）。
fn platform_tag() -> &'static str {
    #[cfg(target_os = "linux")]
    { return "linux-x64"; }
    #[cfg(target_os = "macos")]
    { return if std::env::consts::ARCH == "aarch64" { "osx-arm64-tar" } else { "osx-x64-tar" }; }
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
    { Some(format!("node-v{}-darwin-{}.tar.gz", version, arch)) }
    #[cfg(target_os = "windows")]
    { Some(format!("node-v{}-x64.msi", version)) }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    { None }
}

fn is_candidate(base: &str) -> Result<Option<(String, String)>, String> {
    let raw = http_get_bytes(&format!("{}/index.json", base))?;
    let idx: Value = serde_json::from_slice(&raw).map_err(|e| format!("版本清单解析失败: {}", e))?;
    let arr = idx.as_array().ok_or("版本清单结构异常")?;
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
    Ok(best)
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

/// 官方（或镜像）最新 LTS：返回 (version, platform file)。
pub fn latest_lts() -> Result<(String, String), String> {
    match is_candidate(OFFICIAL) {
        Ok(Some(x)) => return Ok(x),
        Ok(None) => return Err("官方清单中没有可用的 LTS 版本".into()),
        Err(a) => {
            // 官方不可达 → 镜像兜底（前面策略 2：绝不静默降级，明确来源）
            match is_candidate(MIRROR) {
                Ok(Some(x)) => return Ok(x),
                Ok(None) => return Err("官方与镜像清单均无可用 LTS".into()),
                Err(b) => return Err(format!("官方({}) 与镜像({}) 均失败", a, b)),
            }
        }
    }
}

fn sources(_version: &str) -> Vec<(&'static str, &'static str)> {
    vec![(OFFICIAL, "官方"), (MIRROR, "镜像(npmmirror)")]
}

/// 下载 + 官方 SHASUMS256 校验（并发尝试官方/镜像），返回本地文件路径。
pub fn download_verified(version: &str, file: &str, dl_dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dl_dir).map_err(|e| e.to_string())?;
    let mut last_err: Option<String> = None;
    for (base, _label) in sources(version) {
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

/// 平台安装：官方产物 + 一次性系统授权弹窗。
pub fn install(file: &Path) -> Result<PathBuf, String> {
    let abs = file.canonicalize().map_err(|e| e.to_string())?;
    #[cfg(target_os = "linux")]
    {
        let cmd = format!("tar -xJf '{}' -C /usr/local --strip-components=1", abs.display());
        match Command::new("pkexec").args(["sh", "-c", &cmd]).status() {
            Ok(s) if s.success() => { /* 落点检查 */ }
            Ok(s) => return Err(format!("pkexec 退出码 {:?}（用户取消或安装失败）", s.code())),
            Err(e) => return Err(format!("无法启动 pkexec（{}）。请确认系统已安装 pkexec（policykit）。", e)),
        }
        let node = PathBuf::from("/usr/local/bin/node");
        if !node.is_file() { return Err("安装完成但 /usr/local/bin/node 未就位".into()); }
        return Ok(node);
    }
    #[cfg(target_os = "macos")]
    {
        let esc = abs.display().to_string().replace('"', "\"");
        let script = format!("do shell script \"installer -pkg '{}' -target /\" with administrator privileges", esc);
        let out = Command::new("osascript").arg("-e").arg(&script).output()
            .map_err(|e| format!("无法启动 osascript: {}", e))?;
        if !out.status.success() {
            return Err(format!("macOS 安装失败（用户取消或 installer 报错）: {}", String::from_utf8_lossy(&out.stderr)));
        }
        return Ok(PathBuf::from("/usr/local/bin/node"));
    }
    #[cfg(target_os = "windows")]
    {
        let esc = abs.display().to_string().replace('\'', "''");
        let ps = format!("Start-Process -FilePath msiexec -ArgumentList '/i','{}','/qn','/norestart' -Verb RunAs -Wait", esc);
        let out = Command::new("powershell").args(["-NoProfile", "-Command", &ps]).output()
            .map_err(|e| format!("无法启动 msiexec: {}", e))?;
        if !out.status.success() { return Err("Windows 安装失败（用户取消或 msiexec 报错）".into()); }
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

pub fn now_iso() -> String {
    #[cfg(target_os = "windows")]
    { "".to_string() }
    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(o) = Command::new("date").arg("-u").arg("+%Y-%m-%dT%H:%M:%SZ").output() {
            return String::from_utf8_lossy(&o.stdout).trim().to_string();
        }
        "".to_string()
    }
}
