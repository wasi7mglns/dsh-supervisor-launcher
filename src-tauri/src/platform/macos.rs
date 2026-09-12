//! macOS 平台实现（LaunchAgent + launchctl）。
//!
//! 本文件是 macOS 的**全部**平台知识（门禁 G1）。
//!
//! ⚠ `launchctl` 的子命令语义（易错，此处固定）：
//!   · `bootstrap gui/<uid> <plist>`  —— 载入（RunAtLoad → 立即启动；KeepAlive → 崩溃重启）
//!   · `bootout  gui/<uid>/<label>`   —— 卸载
//!   · `kickstart -k gui/<uid>/<label>` —— 重启（本文件的 start）
//!   旧实现 `start` 用 `kickstart -k`，而 `stop` 用 `bootout` —— 不对称：
//!   `bootout` 会把任务从 launchd **完全移除**，此后 `kickstart` 无法命中，
//!   需再 `bootstrap`。本实现保留该语义（`stop` 后由内核的 enable/disable +
//!   `ensure_defined` 的重载路径恢复），并在 `start` 前尝试 bootstrap 兜底。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::service::ServiceControl;
use super::{home_dir, Capabilities, Platform, SVC_NORMAL};

pub const NAME: &str = "macos";
/// 守卫的 LaunchAgent 标签（**定义由本文件建立**；内核只做 enable/disable）。
pub const GUARD_LABEL: &str = "com.dsh.supervisor";

/// 安装命令超时（15 分钟：下载 + installer + 管理员授权）。
const INSTALL_CMD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15 * 60);

pub struct Impl;
static IMPL: Impl = Impl;

pub fn platform() -> &'static dyn Platform {
    &IMPL
}

pub fn service() -> &'static dyn ServiceControl {
    &IMPL
}

impl Platform for Impl {
    fn name(&self) -> &'static str {
        NAME
    }
    fn service(&self) -> &'static dyn ServiceControl {
        &IMPL
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            platform: NAME,
            native_service: true, // launchd
            privilege_channel: true, // osascript（管理员授权）
            node_artifact: "pkg",
        }
    }

    fn node_artifact(&self, version: &str) -> Option<super::NodeArtifact> {
        // ⚠ macOS 必须用官方 **.pkg**（2026-09-11 修复）：
        //   原实现下载 `node-v<ver>-darwin-<arch>.tar.gz`（tarball），却交给
        //   `installer -pkg` 执行 —— 格式不匹配，**必然安装失败**。
        //   官方提供的是通用 .pkg（`node-v<ver>.pkg`，arm64/x64 通用）。
        //   选 .pkg 而非 tarball 的原因：它带**签名与安装位置语义**，
        //   可用一次系统授权装到 /usr/local，与其它平台行为一致。
        //
        //   标签 `osx-x64-pkg` 是官方 index.json 的实际标签（实测确认；
        //   没有 osx-arm64-pkg 条目）。
        Some(super::NodeArtifact {
            tag: "osx-x64-pkg",
            file: format!("node-v{}.pkg", version),
        })
    }

    fn node_candidate_paths(&self) -> Vec<PathBuf> {
        let mut v = vec![
            PathBuf::from("/usr/local/bin/node"),
            // Apple Silicon 上的 Homebrew 落点（原生 arm64 安装常见于此）
            PathBuf::from("/opt/homebrew/bin/node"),
            PathBuf::from("/usr/bin/node"),
        ];
        let h = home_dir();
        v.push(h.join(".volta").join("bin").join("node"));
        if let Some(p) = super::latest_versioned_node(&h.join(".nvm").join("versions").join("node"), &["bin", "node"]) {
            v.push(p);
        }
        if let Some(p) = super::latest_versioned_node(&h.join(".local").join("share").join("fnm").join("node-versions"), &["installation", "bin", "node"]) {
            v.push(p);
        }
        v
    }

    fn node_bin_after_install(&self) -> PathBuf {
        PathBuf::from("/usr/local/bin/node")
    }

    fn is_usable_executable(&self, cand: &Path) -> bool {
        cand.is_file()
    }

    fn install_node(&self, file: &Path) -> Result<PathBuf, String> {
        let abs = file.canonicalize().map_err(|e| e.to_string())?;
        let esc = abs.display().to_string().replace('"', "");
        let script = format!(
            "do shell script "installer -pkg '{}' -target /" with administrator privileges",
            esc
        );
        let out = crate::bounded::run(
            Command::new("osascript").arg("-e").arg(&script),
            INSTALL_CMD_TIMEOUT,
        )
        .map_err(|e| format!("无法启动 osascript: {}", e))?;
        if !out.success {
            return Err(format!(
                "macOS 安装失败（用户取消或 installer 报错）: {}",
                out.stderr.trim()
            ));
        }
        Ok(self.node_bin_after_install())
    }

    fn core_extra_candidates(&self, names: &[&str], _pkg: Option<&str>) -> Vec<PathBuf> {
        // Apple Silicon 与 Intel 的 Homebrew 落点。
        let mut v = Vec::new();
        for name in names {
            v.push(PathBuf::from("/opt/homebrew/bin").join(name));
            v.push(PathBuf::from("/usr/local/bin").join(name));
        }
        v
    }

    fn is_local_fixed_dir(&self, _dir: &Path) -> bool {
        // Unix：无「网络盘 / 可移动盘」概念上的 is_file() 触网风险，
        // 本地文件系统调用不会因路径本身而阻塞数十秒。
        true
    }

    fn has_privilege_channel(&self) -> bool {
        // macOS 的 osascript 管理员授权**恒可用**（无需额外命令）。
        true
    }

    // ── 可执行文件名的平台差异（P2/G1：原为平台层之外的 cfg!() 宏）──
    fn node_exe_name(&self) -> &'static str { "node" }
    fn npm_exe_name(&self) -> &'static str { "npm" }
    fn core_exe_names(&self) -> &'static [&'static str] { &["dsh-supervisor"] }
}

impl ServiceControl for Impl {
    fn kind(&self) -> &'static str {
        "launchagent"
    }

    fn definition_path(&self) -> PathBuf {
        home_dir()
            .join("Library")
            .join("LaunchAgents")
            .join(format!("{}.plist", GUARD_LABEL))
    }

    /// 建立 LaunchAgent plist 并 bootstrap（幂等）。
    fn ensure_defined(&self, guard: &Path) -> Result<String, String> {
        let path = self.definition_path();
        if path.is_file() {
            return Ok(format!("已存在 {}", path.display()));
        }
        let log = home_dir()
            .join(".dsh")
            .join("supervisor")
            .join("log")
            .join("guard-stdio.log");
        let body = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n  <key>Label</key><string>com.dsh.supervisor</string>\n  <key>ProgramArguments</key>\n  <array><string>@BIN@</string><string>daemon</string></array>\n  <key>RunAtLoad</key><true/>\n  <key>KeepAlive</key><true/>\n  <key>ProcessType</key><string>Interactive</string>\n  <key>StandardOutPath</key><string>@LOG@</string>\n  <key>StandardErrorPath</key><string>@LOG@</string>\n</dict></plist>\n"
            .replace("@BIN@", &guard.display().to_string())
            .replace("@LOG@", &log.display().to_string());
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("创建 LaunchAgents 目录失败: {}", e))?;
        }
        if let Some(dir) = log.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(&path, body).map_err(|e| format!("写入 plist 失败: {}", e))?;
        // bootstrap 会因 RunAtLoad 立即启动；KeepAlive 负责崩溃重启。
        let cmd = format!("launchctl bootstrap gui/$(id -u) \"{}\"", path.display());
        let out = crate::bounded::run(Command::new("sh").args(["-c", &cmd]), SVC_NORMAL);
        match out {
            Ok(o) if o.success => Ok(format!("已建立并加载 {}", path.display())),
            Ok(o) => Ok(format!(
                "已建立（bootstrap 未成功: {}，start 时重试）{}",
                o.stderr.trim(),
                path.display()
            )),
            Err(e) => Ok(format!("已建立（bootstrap 超时/失败: {}）{}", e, path.display())),
        }
    }

    fn start(&self) -> Result<(), String> {
        // `kickstart -k` = 若在运行则先杀再启，否则启动。
        // 若任务已被 `stop`（bootout）移除，kickstart 会失败 → 退回 bootstrap 兜底。
        let uid = "$(id -u)";
        let kick = format!("launchctl kickstart -k gui/{}/{}", uid, GUARD_LABEL);
        let r = crate::bounded::run(Command::new("sh").args(["-c", &kick]), SVC_NORMAL);
        if matches!(&r, Ok(o) if o.success) {
            return Ok(());
        }
        let path = self.definition_path();
        if !path.is_file() {
            return Err(format!(
                "launchctl kickstart 失败且无 plist 可 bootstrap（路径 {}）",
                path.display()
            ));
        }
        let boot = format!("launchctl bootstrap gui/{} \"{}\"", uid, path.display());
        let b = crate::bounded::run(Command::new("sh").args(["-c", &boot]), SVC_NORMAL);
        match b {
            Ok(o) if o.success => Ok(()),
            Ok(o) => Err(format!(
                "launchctl bootstrap 失败（重启兜底）：{}",
                o.stderr.trim()
            )),
            Err(e) => Err(format!("launchctl bootstrap 超时/失败：{}", e)),
        }
    }

    fn stop(&self) -> Result<(), String> {
        let cmd = format!("launchctl bootout gui/$(id -u)/{}", GUARD_LABEL);
        crate::bounded::run_checked(
            Command::new("sh").args(["-c", &cmd]),
            SVC_NORMAL,
            "launchctl bootout",
        )
        .map(|_| ())
    }

    fn spawn_daemon(&self, guard: &Path) -> Result<u32, String> {
        let child = Command::new(guard)
            .arg("daemon")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("直接拉起守卫失败: {}", e))?;
        Ok(child.id())
    }
}