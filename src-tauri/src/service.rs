//! 守卫服务定义（三平台）—— 桌面壳自持（2026-09-11 架构修正）。
//!
//! == 为什么由壳负责建立服务定义（用户质疑驱动） ==
//!
//! 旧设计：壳只「启动」服务（systemctl start / launchctl kickstart / schtasks /Run），
//! 「服务的建立」交给内核的「所有者」语义。该设计在**首次安装**场景下必然死锁：
//!
//!   1) 首次启动时唯一的在场组件是**壳** —— 内核此时可能尚未安装；
//!   2) 内核的 install 子命令**不会被任何环节自动调用**（壳只执行 npm install -g）；
//!   3) npm 发行包**不含** systemd/desktop 模板（发布 files 字段未包含它们），
//!      即便调用 install 也会打印「跳过系统服务部署」后直接返回；
//!   4) 于是服务定义从未被建立 -> 壳的 start 必然失败 -> 引导卡在「守卫就绪」。
//!
//! 结论：建立服务定义只能由壳完成（它是唯一在场的组件）。
//!
//! == 职责边界 ==
//!   壳   ：首次**建立**服务定义（本文件）+ 启动 + 服务不可用时的 spawn 兜底
//!   内核 ：运行时的自启**开关**（面板 autostart 开关，保持原状）
//!
//! == 三平台命名（须与壳既有的 start/stop 调用保持一致，不可随意改） ==
//!   Linux   : ~/.config/systemd/user/dsh-supervisor.service
//!   macOS   : ~/Library/LaunchAgents/com.dsh.supervisor.plist
//!   Windows : 计划任务 DSH-Supervisor（指向**守卫守护进程**，不是 GUI 壳）

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn user_name() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "user".into())
}

/// 服务定义文件的规范路径（Windows 为计划任务，返回一个标识串供日志使用）。
pub fn definition_path() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        return home_dir().join(".config").join("systemd").join("user").join("dsh-supervisor.service");
    }
    #[cfg(target_os = "macos")]
    {
        return home_dir().join("Library").join("LaunchAgents").join("com.dsh.supervisor.plist");
    }
    #[cfg(target_os = "windows")]
    {
        return PathBuf::from("schtasks://DSH-Supervisor");
    }
    #[allow(unreachable_code)]
    PathBuf::from("unsupported")
}

// ============================ Linux (systemd --user) ============================

#[cfg(target_os = "linux")]
pub fn ensure_defined(guard: &Path) -> Result<String, String> {
    let path = definition_path();
    if path.is_file() {
        return Ok(format!("已存在 {}", path.display()));
    }
    // 模板内嵌（不再依赖外部 systemd/*.service 文件——npm 发行包不含该目录，
    // 旧实现因此静默跳过服务部署，是本次死锁的直接成因）。
    let body = format!(
        "[Unit]\nDescription=dsh-supervisor - DSH lifecycle guard\nAfter=network.target\nStartLimitIntervalSec=600\nStartLimitBurst=3\n\n[Service]\nType=simple\nExecStart=@BIN@ daemon\nRestart=always\nRestartSec=5\nKillMode=process\n\n[Install]\nWantedBy=default.target\n"
    );
    let body = body.replace("@BIN@", &guard.display().to_string());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建 systemd 目录失败: {}", e))?;
    }
    std::fs::write(&path, body).map_err(|e| format!("写入 unit 失败: {}", e))?;
    let _ = Command::new("systemctl").args(["--user", "daemon-reload"]).output();
    let en = Command::new("systemctl")
        .args(["--user", "enable", "dsh-supervisor.service"])
        .output();
    // linger：未登录也保持用户服务（否则注销后守卫停止）
    let _ = Command::new("loginctl").arg("enable-linger").arg(user_name()).output();
    match en {
        Ok(o) if o.status.success() => Ok(format!("已建立并启用 {}", path.display())),
        _ => Ok(format!("已建立（enable 未成功，start 时重试）{}", path.display())),
    }
}

// ============================ macOS (LaunchAgent) ============================

#[cfg(target_os = "macos")]
pub fn ensure_defined(guard: &Path) -> Result<String, String> {
    let path = definition_path();
    if path.is_file() {
        return Ok(format!("已存在 {}", path.display()));
    }
    let log = home_dir()
        .join(".dsh")
        .join("supervisor")
        .join("log")
        .join("guard-stdio.log");
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n  <key>Label</key><string>com.dsh.supervisor</string>\n  <key>ProgramArguments</key>\n  <array><string>@BIN@</string><string>daemon</string></array>\n  <key>RunAtLoad</key><true/>\n  <key>KeepAlive</key><true/>\n  <key>ProcessType</key><string>Interactive</string>\n  <key>StandardOutPath</key><string>@LOG@</string>\n  <key>StandardErrorPath</key><string>@LOG@</string>\n</dict></plist>\n"
    );
    let body = body
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
    let out = Command::new("sh").args(["-c", &cmd]).output();
    match out {
        Ok(o) if o.status.success() => Ok(format!("已建立并加载 {}", path.display())),
        Ok(o) => Ok(format!(
            "已建立（bootstrap 未成功: {}，start 时重试）{}",
            String::from_utf8_lossy(&o.stderr).trim(),
            path.display()
        )),
        Err(e) => Ok(format!("已建立（bootstrap 调用失败: {}）{}", e, path.display())),
    }
}

// ============================ Windows (schtasks) ============================

#[cfg(target_os = "windows")]
pub fn ensure_defined(guard: &Path) -> Result<String, String> {
    if let Ok(o) = Command::new("schtasks")
        .args(["/Query", "/TN", "DSH-Supervisor"])
        .output()
    {
        if o.status.success() {
            return Ok("已存在 计划任务 DSH-Supervisor".into());
        }
    }
    // 计划任务的 /TR 引号转义极易出错（尤其是路径含空格与 .cmd 垫片）。
    // 改为写一个**包装脚本**再指向它 —— 与内核 watchdog.ps1 同一思路，规避转义地狱。
    let wrapper = home_dir()
        .join(".dsh")
        .join("supervisor")
        .join("guard-task.cmd");
    if let Some(dir) = wrapper.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建状态目录失败: {}", e))?;
    }
    let shim = format!("@echo off\r\n\"{}\" daemon\r\n", guard.display());
    std::fs::write(&wrapper, shim).map_err(|e| format!("写入包装脚本失败: {}", e))?;
    let out = Command::new("schtasks")
        .args([
            "/Create",
            "/TN", "DSH-Supervisor",
            "/SC", "ONLOGON",
            "/RL", "HIGHEST",
            "/F",
            "/TR", &wrapper.display().to_string(),
        ])
        .output()
        .map_err(|e| format!("schtasks 调用失败: {}", e))?;
    if out.status.success() {
        Ok(format!("已建立 计划任务 DSH-Supervisor -> {}", wrapper.display()))
    } else {
        Err(format!(
            "schtasks /Create 失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub fn ensure_defined(_guard: &Path) -> Result<String, String> {
    Err("当前平台不支持服务定义".into())
}

// ============================ spawn 兜底 ============================

/// 服务管理器不可用/未生效时，直接拉起守卫守护进程（保证用户仍能进入产品）。
///
/// 设计取舍：项目原约束为「壳绝不直接 spawn 守卫」，理由是可能产生游离于服务管理器的
/// 第二实例。但该约束不能凌驾于**可用性**之上——容器、无 user systemd session、
/// launchctl 被策略拦截、schtasks 被组策略禁止等场景下，服务管理器根本无法使用，
/// 若无兜底则用户被永久挡在门外。
///
/// 第二实例风险由调用方规避：ensure_guard 在 spawn 前已确认端口不存活，
/// 且 spawn 后仍以「端口就绪」为唯一成功判据（而非进程是否存活）。
pub fn spawn_daemon(guard: &Path) -> Result<u32, String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let child = Command::new("cmd")
            .args(["/C", &guard.display().to_string(), "daemon"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .spawn()
            .map_err(|e| format!("直接拉起守卫失败: {}", e))?;
        return Ok(child.id());
    }
    #[cfg(not(target_os = "windows"))]
    {
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
