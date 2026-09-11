//! 有界子进程执行（**公共设施**，2026-09-11）。
//!
//! == 为什么需要它（同一根因的第二次出现） ==
//!
//! 「环境探测卡死」的根因是「无界阻塞调用 + 被 await」。审计发现**同一模式散布在多处**：
//!
//!   · 当时的 service.rs 与 main.rs —— systemctl / loginctl / launchctl / schtasks 全用 .output()（无界）；
//!   · main.rs    —— start_guard_service / stop_guard_service / taskkill 同样无界；
//!   · node.rs    —— pkexec / osascript / powershell 同样无界。
//!
//! 而这些调用**几乎都在引导的关键路径上**（建立服务定义 → 启动守卫 → 进入面板）。
//! systemctl 在 dbus 会话异常、systemd 无响应时会长时间挂起 —— 此时
//! guard_start 永不返回，引导页永久停在「正在启动守卫…」。
//!
//! 故把「有界执行」提取为公共设施，**所有**外部命令一律经它执行，
//! 而不是在每个调用点各写一遍（那正是缺陷能够分散潜伏的原因）。
//!
//! == 实现要点 ==
//!
//! · 输出重定向到**临时文件**而非管道：若用 Stdio::piped() 且不读取，
//!   冗长输出填满 OS 管道缓冲区（约 64KB）后子进程会阻塞，反而制造死锁。
//! · 轮询 try_wait + 超时 kill：std 无跨平台的 wait-with-timeout，
//!   而子进程一旦挂起，同步 wait 就是无界的 —— 必须自己轮询。
//! · Windows 加 CREATE_NO_WINDOW：GUI 进程调控制台程序不弹黑框。

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct Output {
    pub success: bool,
    pub code: Option<String>,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    /// 成功时取 stdout（已 trim）；失败时把 stderr 作为错误返回。
    pub fn ok_or_stderr(self, what: &str) -> Result<String, String> {
        if self.success {
            Ok(self.stdout.trim().to_string())
        } else {
            let detail = if self.stderr.trim().is_empty() { self.stdout.trim() } else { self.stderr.trim() };
            Err(format!("{} 失败（退出码 {}）：{}", what, self.code.unwrap_or_else(|| "killed".into()), detail))
        }
    }
}

/// 给外部命令加上「不弹控制台窗口」标志（POSIX 平台无需处理）。
pub fn prepare(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

/// 有界执行子进程：超出 timeout 即 kill 并返回错误（**绝不无限阻塞调用方**）。
pub fn run(cmd: &mut Command, timeout: Duration) -> Result<Output, String> {
    let dir = std::env::temp_dir();
    let stamp = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let out_path = dir.join(format!("dsh-cmd-out-{}.log", stamp));
    let err_path = dir.join(format!("dsh-cmd-err-{}.log", stamp));

    let out_file = std::fs::File::create(&out_path).map_err(|e| format!("创建临时日志失败: {}", e))?;
    let err_file = std::fs::File::create(&err_path).map_err(|e| format!("创建临时日志失败: {}", e))?;
    cmd.stdout(Stdio::from(out_file));
    cmd.stderr(Stdio::from(err_file));
    cmd.stdin(Stdio::null());
    prepare(cmd);

    let mut child = cmd.spawn().map_err(|e| format!("启动失败: {}", e))?;

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    let err = read_log(&err_path);
                    let out = read_log(&out_path);
                    cleanup(&out_path, &err_path);
                    let detail = if err.trim().is_empty() { out } else { err };
                    return Err(format!(
                        "执行超时（超过 {} 秒未返回，已终止）{}",
                        timeout.as_secs(),
                        if detail.trim().is_empty() { String::new() } else { format!("：{}", tail(&detail, 200)) }
                    ));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                cleanup(&out_path, &err_path);
                return Err(format!("等待子进程失败: {}", e));
            }
        }
    };

    let stdout = read_log(&out_path);
    let stderr = read_log(&err_path);
    cleanup(&out_path, &err_path);
    Ok(Output {
        success: status.success(),
        code: status.code().map(|c| c.to_string()),
        stdout,
        stderr,
    })
}

/// 执行并返回 stdout，失败即 Err（供「只关心成败」的调用点使用）。
pub fn run_checked(cmd: &mut Command, timeout: Duration, what: &str) -> Result<String, String> {
    run(cmd, timeout)?.ok_or_stderr(what)
}

/// 执行但**忽略结果**（用于尽力而为的动作，如 daemon-reload）。仍然有界。
pub fn run_lossy(cmd: &mut Command, timeout: Duration) {
    let _ = run(cmd, timeout);
}

fn read_log(p: &std::path::Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

fn cleanup(a: &std::path::Path, b: &std::path::Path) {
    let _ = std::fs::remove_file(a);
    let _ = std::fs::remove_file(b);
}

fn tail(s: &str, n: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= n {
        return t.to_string();
    }
    let skip = t.chars().count() - n;
    t.chars().skip(skip).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_run_success() {
        let mut c = Command::new(if cfg!(windows) { "cmd" } else { "echo" });
        if cfg!(windows) {
            c.args(["/C", "echo hello"]);
        } else {
            c.arg("hello");
        }
        let out = run(&mut c, Duration::from_secs(5)).expect("run");
        assert!(out.success, "stderr={}", out.stderr);
        assert!(out.stdout.contains("hello"), "stdout={}", out.stdout);
    }

    #[test]
    fn bounded_run_kills_on_timeout() {
        // 睡眠远超上限：必须被 kill 并返回 Err，而不是无限阻塞
        let mut c = Command::new(if cfg!(windows) { "cmd" } else { "sleep" });
        if cfg!(windows) {
            c.args(["/C", "ping -n 30 127.0.0.1 >NUL"]);
        } else {
            c.arg("30");
        }
        let started = Instant::now();
        let r = run(&mut c, Duration::from_millis(600));
        let el = started.elapsed();
        assert!(r.is_err(), "超时未返回 Err");
        assert!(el < Duration::from_secs(10), "耗时过长: {:?}", el);
    }

    #[test]
    fn missing_binary_is_error_not_panic() {
        let mut c = Command::new("dsh-no-such-binary-xyz");
        assert!(run(&mut c, Duration::from_secs(2)).is_err());
    }
}
