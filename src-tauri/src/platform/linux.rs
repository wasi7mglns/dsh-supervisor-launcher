//! Linux 平台实现（systemd --user）。
//!
//! 本文件是 Linux 的**全部**平台知识 —— 其它任何文件都不应出现 `target_os = "linux"`（门禁 G1）。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::service::ServiceControl;
use super::{home_dir, user_name, Capabilities, Platform, SVC_NORMAL, SVC_QUICK};

pub const NAME: &str = "linux";

/// 安装命令超时（15 分钟：下载 + 解包 + 系统授权）。
const INSTALL_CMD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// Linux 可用提权通道（**按优先级**）。单一事实源：
///   · has_privilege_channel() 用它判定「能否提权」；
///   · install_node() 用它选**实际执行**的命令。
///
/// ⚠ 两者必须同源（2026-09-13 P3 修复）。此前前者探测「pkexec 或 sudo」、
///   后者硬编码 pkexec → 有 sudo 无 pkexec 的机器被误判「可自更新」却在安装时失败。
pub const PRIVILEGE_COMMANDS: [&str; 2] = ["pkexec", "sudo"];

/// 在 PATH 中定位第一个可用的提权命令（**不执行**，只判存在性）。
///
/// 返回 None 表示系统两种通道都没有 —— 调用方据此给出**明确的**环境错误，
/// 而不是去执行一个不存在的命令再报一个含混的 spawn 失败。
pub fn find_privilege_command() -> Option<&'static str> {
    let path = std::env::var_os("PATH")?;
    let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    PRIVILEGE_COMMANDS
        .iter()
        .find(|c| dirs.iter().any(|d| d.join(c).is_file()))
        .copied()
}

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
            native_service: true, // systemd --user
            privilege_channel: true, // pkexec / sudo
            node_artifact: "tar.xz",
        }
    }

    fn node_artifact(&self, version: &str) -> Option<super::NodeArtifact> {
        // 官方 files[] 两个标签都存在（实测）。
        //
        // ⚠ 2026-09-12（P2）：原为「非 aarch64 即 x64」——静默把未知架构当 x64，
        //   而 core.rs::package_name() 对同一事实是显式 match + Err（两处两种态度）。
        //   本函数返回 Option，故未知架构用 None 表达「无可用制品」，
        //   由调用方如实报告，而不是拿到一个架构不符的包（那比明确失败更糟）。
        let arch = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            _ => return None,
        };
        let tag = if arch == "arm64" { "linux-arm64" } else { "linux-x64" };
        Some(super::NodeArtifact {
            tag,
            file: format!("node-v{}-linux-{}.tar.xz", version, arch),
        })
    }

    fn node_candidate_paths(&self) -> Vec<PathBuf> {
        let mut v = vec![
            PathBuf::from("/usr/local/bin/node"),
            PathBuf::from("/usr/bin/node"),
            PathBuf::from("/bin/node"),
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
        let cmd = format!("tar -xJf '{}' -C /usr/local --strip-components=1", abs.display());
        // ⚠ P3 修复（2026-09-13）：**经与 has_privilege_channel 同一个 helper** 选提权命令。
        //
        //   缺陷：has_privilege_channel 探测「pkexec **或** sudo」，而这里**硬编码 pkexec**。
        //     一台**有 sudo、无 pkexec** 的机器（常见于精简发行版/容器/自建环境）上：
        //       · self_update_capable() / 引导页诊断显示「可自更新」；
        //       · 但真正安装 Node 时立刻失败于「无法启动 pkexec」。
        //     用户看到的是「已经说可以，却装不上」，且错误指向 pkexec —— 与实际环境不符。
        //     （桌面壳自更新走 tauri-plugin-updater，它内部是 pkexec → GUI sudo → sudo 回退，
        //       故只有本壳自己的 Node 安装这一条路径漏了回退。）
        //
        //   修法：两者共用 find_privilege_command()（单一事实源）—— 见 PRIVILEGE_COMMANDS。
        let priv_cmd = find_privilege_command().ok_or_else(|| {
            format!(
                "未找到可用提权通道（需要 {} 之一）。请安装 policykit（提供 pkexec）或 sudo。",
                PRIVILEGE_COMMANDS.join(" 或 ")
            )
        })?;
        let out = crate::bounded::run(
            Command::new(priv_cmd).args(["sh", "-c", &cmd]),
            INSTALL_CMD_TIMEOUT,
        )
        .map_err(|e| format!("无法启动 {}（{}）。", priv_cmd, e))?;
        if !out.success {
            return Err(format!(
                "{} 退出码 {}（用户取消或安装失败）：{}",
                priv_cmd,
                out.code.unwrap_or_else(|| "killed".into()),
                out.stderr.trim()
            ));
        }
        let node = self.node_bin_after_install();
        if !node.is_file() {
            return Err(format!("安装完成但 {} 未就位", node.display()));
        }
        Ok(node)
    }

    fn core_extra_candidates(&self, _names: &[&str], _pkg: Option<&str>) -> Vec<PathBuf> {
        // Linux：PATH 与 ~/.local/bin（内核 install 写入的软链）已覆盖全部落点。
        Vec::new()
    }

    fn is_local_fixed_dir(&self, _dir: &Path) -> bool {
        // Unix：无「网络盘 / 可移动盘」概念上的 is_file() 触网风险，
        // 本地文件系统调用不会因路径本身而阻塞数十秒。
        true
    }

    fn has_privilege_channel(&self) -> bool {
        // **不主动执行提权**，只探测命令存在性（用于「不可自更新」的提前判定）。
        // ⚠ 与 install_node **共用** find_privilege_command —— 同一事实一处实现。
        find_privilege_command().is_some()
    }

    // ── 可执行文件名的平台差异（P2/G1：原为平台层之外的 cfg!() 宏）──
    fn node_exe_name(&self) -> &'static str { "node" }
    fn npm_exe_name(&self) -> &'static str { "npm" }
    fn core_exe_names(&self) -> &'static [&'static str] { &["dsh-supervisor"] }
}

impl ServiceControl for Impl {
    fn kind(&self) -> &'static str {
        "systemd"
    }

    fn definition_path(&self) -> PathBuf {
        home_dir()
            .join(".config")
            .join("systemd")
            .join("user")
            .join("dsh-supervisor.service")
    }

    /// 建立 systemd 用户单元（幂等，且**内容过时时自愈**）。
    ///
    /// ⚠ 2026-09-12（P2 修复）：原实现是「`path.is_file()` → 直接返回」，
    ///   即**只创建、永不更新**。后果：模板演进后（例如 P3 给 ExecStart 加引号），
    ///   老用户磁盘上的旧 unit **永远不会被重写** → 修复到不了已装用户。
    ///   macOS plist 与 Windows 计划任务同病。
    ///
    ///   现改为：先算出**期望内容**，与磁盘上的比对；
    ///     · 不存在 → 写入（原行为）；
    ///     · 存在但内容不同 → **重写**并 reload（自愈，使模板修复能触达用户）；
    ///     · 存在且一致 → 不触碰（真正的幂等，避免每次启动都写盘/reload）。
    fn ensure_defined(&self, guard: &Path) -> Result<String, String> {
        let path = self.definition_path();
        // 模板内嵌（不再依赖外部 systemd/*.service 文件 —— npm 发行包不含该目录，
        // 旧实现因此静默跳过服务部署，是本次死锁的直接成因）。
        //
        // ⚠ `ExecStart` 的可执行路径**必须自带引号**（P3 修复，2026-09-12）。
        //   systemd 对 ExecStart 的第一参数按 shell-like 规则解析：
        //   **未加引号的空格会被当作参数分隔符** → 路径被拆成两段 →
        //   systemd 报 `Command /home/user is not executable`（实测 systemd-analyze verify 确认）。
        //   而用户家目录**可以含空格**（Linux 亦如此，/home/john smith/...），
        //   且 `~/.local/bin/dsh-supervisor` 也可能落在含空格的路径下。
        //
        //   对照：Windows 侧同类问题（cmd 引号、schtasks /TR）已修并有测试；
        //   内核侧 launchd plist 用数组形式 <string>@BIN@</string>（天然无此问题）——
        //   唯独 systemd 这处漏网。
        //
        //   注意：引号写在**值内部**（systemd 需要它来界定第一个参数），
        //   这与给 Rust args 加引号不同 —— 后者只会被原样作为路径的一部分。
        let exec_start = format!("\"{}\" daemon", guard.display());
        let body = "[Unit]\nDescription=dsh-supervisor - DSH lifecycle guard\nAfter=network.target\nStartLimitIntervalSec=600\nStartLimitBurst=3\n\n[Service]\nType=simple\nExecStart=@EXEC@\nRestart=always\nRestartSec=5\nKillMode=process\n\n[Install]\nWantedBy=default.target\n"
            .replace("@EXEC@", &exec_start);
        // ── 内容比对：决定「新写」「重写」还是「不动」──
        let existing = std::fs::read_to_string(&path).ok();
        let needs_write = match &existing {
            Some(cur) => cur != &body, // 存在但内容过时 → 重写（自愈）
            None => true,              // 不存在 → 新建
        };
        if !needs_write {
            // 真正的幂等：内容一致就不动盘、不 reload（避免每次启动都触发 daemon-reload）。
            return Ok(format!("已存在且为最新 {}", path.display()));
        }
        let is_update = existing.is_some();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("创建 systemd 目录失败: {}", e))?;
        }
        std::fs::write(&path, &body).map_err(|e| format!("写入 unit 失败: {}", e))?;
        // 全部经 bounded::run：超时即返回，绝不把引导挂在 systemctl 上。
        crate::bounded::run_lossy(
            Command::new("systemctl").args(["--user", "daemon-reload"]),
            SVC_QUICK,
        );
        let en = crate::bounded::run(
            Command::new("systemctl").args(["--user", "enable", "dsh-supervisor.service"]),
            SVC_NORMAL,
        );
        // linger：未登录也保持用户服务（否则注销后守卫停止）。
        crate::bounded::run_lossy(
            Command::new("loginctl").arg("enable-linger").arg(user_name()),
            SVC_NORMAL,
        );
        // 注意：enable 失败不算致命 —— 服务定义已写入，start 时仍可拉起（并另有 spawn 兜底）。
        // 故这里只如实描述状态，不返回 Err（否则会把「可继续」的情形误判为彻底失败）。
        // 文案区分「新建」与「更新」（自愈时用户从 shell.log 就能看出定义被升级过）。
        let verb = if is_update { "已更新并启用" } else { "已建立并启用" };
        match en {
            Ok(o) if o.success => Ok(format!("{} {}", verb, path.display())),
            Ok(o) => Ok(format!(
                "{}（enable 未成功：{}，start 时重试）{}",
                verb,
                o.stderr.trim(),
                path.display()
            )),
            Err(e) => Ok(format!(
                "{}（enable 超时/失败：{}，start 时重试）{}",
                verb,
                e,
                path.display()
            )),
        }
    }

    fn start(&self) -> Result<(), String> {
        crate::bounded::run_checked(
            Command::new("systemctl").args(["--user", "start", "dsh-supervisor"]),
            SVC_NORMAL,
            "systemctl --user start dsh-supervisor",
        )
        .map(|_| ())
    }

    fn stop(&self) -> Result<(), String> {
        crate::bounded::run_checked(
            Command::new("systemctl").args(["--user", "stop", "dsh-supervisor"]),
            SVC_NORMAL,
            "systemctl --user stop",
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

#[cfg(test)]
mod tests {
    //! A-2 门禁：提权通道的「探测」与「使用」必须**同源**（2026-09-13）。
    //!
    //! 缺陷：has_privilege_channel 探测「pkexec **或** sudo」，而 install_node 硬编码
    //! pkexec。有 sudo 无 pkexec 的机器被误判「可自更新」，却在安装 Node 时失败。
    //! 该缺陷在二进制里无法用普通单测覆盖（要真提权），故按本仓既有惯例做**结构断言**；
    //! 但断言的是**特征调用**（install_node 里必须出现 find_privilege_command，
    //! 且不得再出现字面量 pkexec），而不是易撞名的局部变量。
    use super::*;

    /// 去掉注释后再断言 —— 本仓两次被自己写的说明文字骗过（见 AUDIT-HANDOFF 9.2）。
    fn strip_comments(src: &str) -> String {
        src.lines()
            .map(|l| {
                let t = l.trim_start();
                if t.starts_with("//") { String::new() } else { l.to_string() }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a2_probe_and_use_share_one_privilege_source() {
        let raw = include_str!("linux.rs");
        let code = strip_comments(raw);

        // 正向：探测与实际执行都必须经同一 helper
        assert!(
            code.contains("find_privilege_command()"),
            "A-2 FAIL 未使用共享的 find_privilege_command"
        );
        let install = code
            .split("fn install_node")
            .nth(1)
            .expect("A-2 FAIL 未找到 install_node");
        let install_body = install.split("fn core_extra_candidates").next().unwrap_or(install);
        assert!(
            install_body.contains("find_privilege_command()"),
            "A-2 FAIL install_node 未用共享 helper 选提权命令"
        );
        // 反向：install_node 内不得再**硬编码** pkexec 作为执行命令
        assert!(
            !install_body.contains("Command::new(\"pkexec\")"),
            "A-2 FAIL install_node 仍硬编码 pkexec —— 有 sudo 无 pkexec 的机器会被误判可自更新"
        );
        let probe = code
            .split("fn has_privilege_channel")
            .nth(1)
            .expect("A-2 FAIL 未找到 has_privilege_channel");
        let probe_body = probe.split("fn node_exe_name").next().unwrap_or(probe);
        assert!(
            probe_body.contains("find_privilege_command()"),
            "A-2 FAIL has_privilege_channel 未用共享 helper"
        );
        // 单一事实源：命令清单只定义一次。
        //   ⚠ 针脚必须**在运行时拼出**（不能是源码里逐字出现的字面量）——
        //     include_str! 把**本测试自己也读进来了**，逐字字面量必然自匹配，
        //     正是 AUDIT-HANDOFF 9.2 记录的「断言命中自己的文字」陷阱。
        let needle = format!("const {}: ", "PRIVILEGE_COMMANDS");
        assert_eq!(
            code.matches(&needle).count(),
            1,
            "A-2 FAIL PRIVILEGE_COMMANDS 不是单一定义"
        );
    }

    /// A-2（行为）：find_privilege_command 只认 PATH 里**真实存在**的命令。
    #[test]
    fn a2_find_privilege_command_respects_path() {
        // 空 PATH → 必然 None（不依赖机器上是否真有 pkexec/sudo）
        let saved = std::env::var_os("PATH");
        std::env::set_var("PATH", "");
        let none = find_privilege_command();
        // 恢复 PATH（后续测试可能依赖）
        match &saved {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
        assert!(none.is_none(), "A-2 FAIL 空 PATH 下仍报告有提权通道: {:?}", none);

        // 造一个只含伪 pkexec 的临时目录 → 必须命中它
        let dir = std::env::temp_dir().join(format!("dsh-priv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("pkexec");
        std::fs::write(&fake, b"#!/bin/sh\n").unwrap();
        std::env::set_var("PATH", dir.display().to_string());
        let got = find_privilege_command();
        match &saved {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(got, Some("pkexec"), "A-2 FAIL 未按 PATH 命中伪造的 pkexec");
    }
}