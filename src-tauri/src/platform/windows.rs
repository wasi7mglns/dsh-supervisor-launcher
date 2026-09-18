//! Windows 平台实现（计划任务 schtasks）。
//!
//! 本文件是 Windows 的**全部**平台知识（门禁 G1）。

use std::path::{Path, PathBuf};
use std::process::Command;

use super::service::ServiceControl;
use super::{home_dir, Capabilities, LaunchSpec, Platform, SVC_NORMAL, SVC_QUICK};

pub const NAME: &str = "windows";
/// 计划任务名（**定义由本文件建立**；内核不再管理，见 D6）。
pub const GUARD_TASK: &str = "DSH-Supervisor";
/// 崩溃自拉的保活任务（**由壳建立**；停止守卫时须先停它）。
/// 2026-09-15：所有者从内核 `autostart.js` 收归壳（KERNEL-DAEMON-CONTRACT D6）。
pub const WATCHDOG_TASK: &str = "DSH-Supervisor-Watchdog";

/// Windows 看护脚本（PowerShell，纯文本免 cmd 转义）。
/// 语义：API 不可达且无守卫 node 进程 → 启动稳定入口 `<壳> --run-guard`；
/// GUI 壳缺失则**独立**拉起（两个判断必须相互独立，否则「壳崩、守卫活」时壳永远不回来）。
///
/// 看护脚本本身**不含** node/guard 路径 —— 检测在 `--run-guard` 内完成。
fn watchdog_script(_spec: &LaunchSpec) -> String {
    let port = crate::env::api_port();
    let shell = std::env::current_exe()
        .map(|p| ps_quote(&p.display().to_string()))
        .unwrap_or_else(|_| "''".into());
    [
        "$ErrorActionPreference = \"SilentlyContinue\"".to_string(),
        format!("$port = {};", port),
        format!("$shell = {};", shell),
        "$up = Test-NetConnection -ComputerName 127.0.0.1 -Port $port -InformationLevel Quiet -WarningAction SilentlyContinue".to_string(),
        "if (-not $up) {".to_string(),
        "  $p = @(Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'node.exe' -and $_.CommandLine -like '*dsh-supervisor*' })".to_string(),
        "  if (-not $p) { Start-Process -FilePath $shell -ArgumentList '--run-guard' -WindowStyle Hidden }".to_string(),
        "}".to_string(),
        "$g = @(Get-CimInstance Win32_Process | Where-Object { $_.Name -like 'dsh-supervisor*' -and $_.CommandLine -notlike '*--run-guard*' })".to_string(),
        "if (-not $g -and (Test-Path $shell)) { Start-Process -FilePath $shell -WindowStyle Hidden }".to_string(),
        "exit 0".to_string(),
    ].join("\r\n")
}

/// PowerShell 单引号字符串（内部单引号翻倍；反斜杠为字面量，无需转义）。
fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// 归档解包超时（15 分钟；下载已完成，余量给解包与慢盘）。
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
            native_service: true, // 计划任务
            privilege_channel: true, // 仅壳自更新（替换安装包）用；Node 安装已改为用户级、零权限
            node_artifact: "zip",
        }
    }

    fn core_platform_tag(&self) -> Option<&'static str> {
        match std::env::consts::ARCH {
            "x86_64" => Some("win-x64"),
            "aarch64" => Some("win-arm64"),
            _ => None,
        }
    }

    fn node_artifact(&self, version: &str) -> Option<super::NodeArtifact> {
        // 用户级安装：官方对 x64/arm64 都提供 zip，解包到 <状态根>/node，**无需 UAC**
        //   （2026-09-18 权限模型重写）。原 MSI + UAC 路径在「提升到管理员账户」时
        //   常读不到当前用户 profile 下的安装包 → msiexec 1619；zip 路径彻底消除该问题，
        //   且 arm64 用**原生**制品（原实现只能退 x64 msi 靠模拟）。
        let arch = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            _ => return None,
        };
        let tag = if arch == "arm64" { "win-arm64-zip" } else { "win-x64-zip" };
        Some(super::NodeArtifact {
            tag,
            file: format!("node-v{}-win-{}.zip", version, arch),
        })
    }

    fn node_candidate_paths(&self) -> Vec<PathBuf> {
        // 不得硬编码 C:\Program Files（2026-09-11 审计）：
        //   真实路径随**系统盘符**与**系统语言**变化（中文系统是本地化目录名），
        //   也可能装在 Program Files (x86)。故一律经环境变量推导。
        let exe = "node.exe";
        // 用户级安装（<状态根>/node）**最先**：壳自己装的，优先于系统其它 Node。
        let mut v: Vec<PathBuf> = vec![self.node_bin_after_install()];
        let env_dir = |var: &str, rest: &[&str]| -> Option<PathBuf> {
            std::env::var(var).ok().map(|base| {
                let mut p = PathBuf::from(base);
                for seg in rest {
                    p = p.join(seg);
                }
                p
            })
        };
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(p) = env_dir(var, &["nodejs", exe]) {
                v.push(p);
            }
        }
        if let Some(p) = env_dir("ProgramData", &["chocolatey", "bin", exe]) {
            v.push(p);
        }
        if let Some(p) = env_dir("LOCALAPPDATA", &["Programs", "nodejs", exe]) {
            v.push(p);
        }
        if let Some(p) = env_dir("LOCALAPPDATA", &["Volta", "bin", exe]) {
            v.push(p);
        }
        if let Some(p) = env_dir("USERPROFILE", &["scoop", "apps", "nodejs", "current", exe]) {
            v.push(p);
        }
        // nvm-windows 有三种布局：LOCALAPPDATA\nvm、APPDATA\nvm、%NVM_HOME%
        for base in ["LOCALAPPDATA", "APPDATA"] {
            if let Ok(b) = std::env::var(base) {
                if let Some(p) = super::latest_versioned_node(&PathBuf::from(&b).join("nvm"), &[exe]) {
                    v.push(p);
                }
            }
        }
        if let Ok(nh) = std::env::var("NVM_HOME") {
            if let Some(p) = super::latest_versioned_node(&PathBuf::from(&nh), &[exe]) {
                v.push(p);
            }
            v.push(PathBuf::from(&nh).join(exe));
        }
        if let Ok(link) = std::env::var("NVM_SYMLINK") {
            v.push(PathBuf::from(&link).join(exe));
        }
        v
    }

    fn node_bin_after_install(&self) -> PathBuf {
        // 用户级安装落点（零权限）；不再指向 %ProgramFiles%\nodejs（那需要管理员）。
        crate::env::node_install_root().join(self.node_exe_name())
    }

    fn is_usable_executable(&self, cand: &Path) -> bool {
        // 过滤两类**伪可执行**：
        //   · \WindowsApps\ 下的应用执行别名存根 —— 执行它会挂起或唤起 Store；
        //   · 0 字节文件。
        if !cand.is_file() {
            return false;
        }
        let low = cand.to_string_lossy().to_ascii_lowercase();
        if low.contains("\\windowsapps\\") {
            return false;
        }
        !std::fs::metadata(cand).map(|m| m.len() == 0).unwrap_or(true)
    }

    fn install_node(&self, file: &Path) -> Result<PathBuf, String> {
        // 用户级解包（zip），**完全不需要管理员/UAC**（2026-09-18 权限模型重写）。
        //   原 MSI + Start-Process -Verb RunAs 的两个致命问题：
        //     ① UAC 提升到管理员账户后常读不到当前用户 profile 下的 .msi → msiexec 1619；
        //     ② canonicalize() 在 Windows 返回 \\?\ 前缀路径，msiexec 不认。
        //   zip 解包两问题都不存在（本进程直接写自己的状态目录）。
        let root = crate::env::node_install_root();
        let staging = root.with_file_name("node.extract");
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
        // Expand-Archive 是 Windows 内置（PS 5+）；单引号内再转义单引号。
        let ps = format!(
            "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
            file.display().to_string().replace('\'', "''"),
            staging.display().to_string().replace('\'', "''")
        );
        let out = crate::bounded::run(
            Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &ps]),
            INSTALL_CMD_TIMEOUT,
        )
        .map_err(|e| format!("无法启动 powershell: {}", e))?;
        if !out.success {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(format!("解包 Node 归档失败: {}", out.stderr.trim()));
        }
        let r = super::commit_user_node(&staging, &root, &[self.node_exe_name()]);
        if r.is_err() {
            let _ = std::fs::remove_dir_all(&staging);
        }
        r
    }

    fn core_extra_candidates(&self, names: &[&str], pkg: Option<&str>) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = Vec::new();
        let Ok(appdata) = std::env::var("APPDATA") else {
            return v;
        };
        let npm_root = PathBuf::from(&appdata).join("npm");
        for name in names {
            // npm 生成的 .cmd 垫片（在 npm 根目录下）
            v.push(npm_root.join(name));
        }
        // 真实包内脚本：.cmd 垫片无法被 package_dir_of 解析（父目录不是 bin/），
        // 且执行它取版本在部分环境下会失败。直接给出包内真实路径优先命中，
        // 既能正确读 package.json 取版本，也能让 global_prefix_for 正常推导前缀。
        if let Some(p) = pkg {
            v.push(
                npm_root
                    .join("node_modules")
                    .join(p)
                    .join("bin")
                    .join("dsh-supervisor"),
            );
        }
        v
    }

    /// Windows：npm 全局垫片直接在 prefix 下（`<name>.cmd`），真实脚本在
    ///   `<prefix>\node_modules\<pkg>\bin\<name>`（后者可被 package_dir_of 正确解析版本）。
    fn core_bin_candidates_in_prefix(
        &self,
        prefix: &std::path::Path,
        names: &[&str],
        pkg: Option<&str>,
    ) -> Vec<std::path::PathBuf> {
        let mut v: Vec<std::path::PathBuf> = Vec::new();
        for name in names {
            v.push(prefix.join(format!("{}.cmd", name)));
            v.push(prefix.join(name));
        }
        if let Some(p) = pkg {
            for name in names {
                v.push(prefix.join("node_modules").join(p).join("bin").join(name));
            }
        }
        v
    }

    /// Windows 状态根惯例：%LOCALAPPDATA%\dsh-supervisor。
    fn state_root_default(&self) -> PathBuf {
        let base = std::env::var("LOCALAPPDATA")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join("AppData").join("Local"));
        base.join("dsh-supervisor")
    }

    fn is_local_fixed_dir(&self, dir: &Path) -> bool {
        use std::os::windows::ffi::OsStrExt;
// 先做本地固定盘判定（不触网），再访问文件系统；按盘符缓存，每盘只查一次。
        let w: Vec<u16> = dir.as_os_str().encode_wide().collect();
        // UNC（以两个反斜杠开头，ASCII 92）→ 跳过（纯字面判定，不触网）
        if w.len() >= 2 && w[0] == 92 && w[1] == 92 {
            return false;
        }
        // 无盘符（相对路径等）→ 保守放行
        if w.len() < 2 || w[1] != 58 {
            return true;
        }
        drive_is_fixed(w[0])
    }

    fn has_privilege_channel(&self) -> bool {
        // Windows 恒有 UAC 提权通道（**仅壳自更新用**；Node 安装已用户级、零权限）。
        true
    }

    // ── 可执行文件名的平台差异（P2/G1：原为平台层之外的 cfg!() 宏）──
    fn node_exe_name(&self) -> &'static str { "node.exe" }
    /// Windows 上 npm 是 `.cmd`；Node 的 spawn/execFileSync **不做 PATHEXT 解析** ——
    /// 与内核侧 `platform/os/exec-path.js::npmBin()` 同一事实（P1-C）。
    fn npm_exe_name(&self) -> &'static str { "npm.cmd" }
    /// Windows 内核候选：`.cmd` 垫片必须在内 —— PATH 解析只认扩展名形态。
    fn core_exe_names(&self) -> &'static [&'static str] {
        &["dsh-supervisor.exe", "dsh-supervisor.cmd", "dsh-supervisor"]
    }
}

/// Windows 看护任务的固有实现（**不属于** ServiceControl 契约：它是壳的私有辅助，
/// 由 `ensure_defined` 调用；放进 trait impl 内会触发 E0407）。
impl Impl {
    /// 壳拥有的 Windows 看护任务（D6/H5）：写 watchdog.ps1 + schtasks MINUTE。
    /// 幂等：每次 ensure_defined 都重写脚本并 `/Create /F`（覆盖语义），
    ///   故不会因守卫任务「已是最新」而被跳过。
    fn watchdog_status(&self, spec: &LaunchSpec) -> String {
        match self.ensure_watchdog(spec) {
            Ok(s) => format!("；{}", s),
            Err(e) => format!("；看护未建立（{}）", e),
        }
    }

    fn ensure_watchdog(&self, spec: &LaunchSpec) -> Result<String, String> {
        let dir = crate::env::supervisor_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建状态目录失败: {}", e))?;
        let ps1 = dir.join("watchdog.ps1");
        let body = watchdog_script(spec);
        let tmp = ps1.with_extension("ps1.tmp");
        std::fs::write(&tmp, body).map_err(|e| format!("写看护脚本失败: {}", e))?;
        std::fs::rename(&tmp, &ps1).map_err(|e| format!("落盘看护脚本失败: {}", e))?;
        let tr = format!(
            "powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"{}\"",
            ps1.display()
        );
        let r = crate::bounded::run(
            Command::new("schtasks").args([
                "/Create", "/TN", WATCHDOG_TASK, "/SC", "MINUTE", "/MO", "5", "/RL", "HIGHEST", "/F", "/TR", &tr,
            ]),
            SVC_NORMAL,
        )?;
        if r.success {
            Ok(format!("看护任务 {} 已建立", WATCHDOG_TASK))
        } else {
            Err(format!("schtasks 看护任务创建失败: {}", r.stderr.trim()))
        }
    }

}

impl ServiceControl for Impl {
    fn kind(&self) -> &'static str {
        "schtasks"
    }

    fn definition_path(&self) -> PathBuf {
        // 计划任务不是文件；返回标识串供日志/诊断。
        // 正因如此，**不能**用 `definition_path().is_file()` 判断「定义是否存在」
        //   （恒 false，会让 --service-plan 自检误报）—— 见下面的 is_defined 覆写。
        PathBuf::from(format!("schtasks://{}", GUARD_TASK))
    }

    /// Windows 的真实判定：`schtasks /Query` 成功即计划任务存在。
    ///
/// 覆写默认判定：以 `schtasks /Query` 成功为准（标识串用 is_file() 恒 false）。
    fn is_defined(&self) -> bool {
        matches!(
            crate::bounded::run(
                Command::new("schtasks").args(["/Query", "/TN", GUARD_TASK]),
                SVC_QUICK,
            ),
            Ok(o) if o.success
        )
    }

    /// 建立计划任务（幂等，且**包装脚本过时时自愈**）。
    ///
    /// 2026-09-12（P2）：原实现「`/Query` 成功 → 直接返回」= **只创建、永不更新**。
    ///   与 Linux unit / macOS plist 同病：模板演进后老用户永远跑旧定义。
    ///
    ///   Windows 与另两平台的区别：计划任务**本身**无法直接比对内容，
    ///   但它的动作指向我们写的 `.cmd` 包装脚本（可比对）；
    ///   故判据改为「任务存在 **且** 包装脚本内容一致」才提前返回，
    ///   否则用 `/Create /F` 强制重建（`/F` 本就是覆盖语义）。
    fn ensure_defined(&self, spec: &LaunchSpec) -> Result<String, String> {
        let task_exists = matches!(
            crate::bounded::run(
                Command::new("schtasks").args(["/Query", "/TN", GUARD_TASK]),
                SVC_QUICK,
            ),
            Ok(o) if o.success
        );
        // 稳定入口（2026-09-15 架构修正）：计划任务只指向 `<壳> --run-guard`。
        //   定义中**不含** node/guard 路径 —— 检测由 --run-guard 在每次启动时完成。
        //   旧实现把 node/guard 写进现场生成的 .cmd/.ps1（编码/前缀/垫片轮番咬），已整体删除。
        let (shell, args) = spec.service_command();
        let action = super::service_exec_line(shell, args);
        // 动作内容记录（P2 自愈）：计划任务无法回读动作串，故本地留一份用于比对。
        let record = crate::env::supervisor_dir().join("guard-task.action");
        if let Some(dir) = record.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("创建状态目录失败: {}", e))?;
        }
        let record_current = std::fs::read_to_string(&record).ok().as_deref() == Some(action.as_str());
        if task_exists && record_current {
            // 即使守卫任务已是最新，也必须确保**壳拥有的看护任务**存在（幂等）。
            let wd = self.watchdog_status(spec);
            return Ok(format!("已存在且为最新 计划任务 {}{}", GUARD_TASK, wd));
        }
        let is_update = task_exists;
        std::fs::write(&record, &action).map_err(|e| format!("写入动作记录失败: {}", e))?;
        // 看护任务（DSH-Supervisor-Watchdog）的所有者 = 壳（D6/H5）：随服务定义一并（重）建立。
        let wd_note = self.watchdog_status(spec);
        // `/TR` 值**自带引号**（写在值内部，非给 Rust args 加引号）：路径含空格时不被截断。
        let tr_action = action;
        let base = |rl: Option<&str>| {
            let mut c = Command::new("schtasks");
            c.args(["/Create", "/TN", GUARD_TASK, "/SC", "ONLOGON"]);
            if let Some(level) = rl {
                c.args(["/RL", level]);
            }
            c.args(["/F", "/TR", &tr_action]);
            c
        };
        let verb = if is_update { "已更新" } else { "已建立" };
        let first = crate::bounded::run(&mut base(Some("HIGHEST")), SVC_NORMAL)?;
        if first.success {
            return Ok(format!(
                "{} 计划任务 {}（最高权限）-> {}{}",
                verb, GUARD_TASK, tr_action, wd_note
            ));
        }
        // 降级重试（去掉 /RL HIGHEST）
        let second = crate::bounded::run(&mut base(None), SVC_NORMAL)?;
        if second.success {
            return Ok(format!(
                "{} 计划任务 {}（普通权限，HIGHEST 被拒）-> {}{}",
                verb, GUARD_TASK, tr_action, wd_note
            ));
        }
        Err(format!(
            "schtasks /Create 失败（含降级重试）: 首次={} / 降级={}",
            first.stderr.trim(),
            second.stderr.trim()
        ))
    }

    fn start(&self) -> Result<(), String> {
        crate::bounded::run_checked(
            Command::new("schtasks").args(["/Run", "/TN", GUARD_TASK]),
            SVC_NORMAL,
            "schtasks /Run",
        )
        .map(|_| ())
    }

    fn stop(&self) -> Result<(), String> {
        // Windows：先停 watchdog 保活任务，再终止守卫进程（否则 watchdog 会立刻重新拉起）。
        // 全部有界：退出流程也要能在服务管理器无响应时走完，否则用户会觉得「程序关不掉」。
        // ⚠ 2026-09-18 修（严重缺陷：退出管家后自动重启）：
        //   原只用 /End —— 那只结束**本次运行实例**，而 DSH-Supervisor-Watchdog 是
        //   /SC MINUTE /MO 5 的**计划**（watchdog.ps1 在无 dsh-supervisor* GUI 进程时
        //   `Start-Process <壳>`，在守卫端口 down 时 `Start-Process <壳> --run-guard`）。
        //   /End 不禁用计划 ⇒ ≤5 分钟后看护再次触发，把守卫与桌面壳一起拉回来。
        //   故看护任务必须 **/Delete 计划**；下次启动 ensure_defined 会重建（幂等）。
        crate::bounded::run_lossy(
            Command::new("schtasks").args(["/Delete", "/TN", WATCHDOG_TASK, "/F"]),
            SVC_NORMAL,
        );
        crate::bounded::run_lossy(
            Command::new("schtasks").args(["/End", "/TN", GUARD_TASK]),
            SVC_NORMAL,
        );
        // 守卫是 `node.exe`（**不是** dsh-supervisor.exe）——旧的按镜像名 taskkill 根本杀不掉它，
        //   会残留进程/锁，导致后续启动被 guard.lock 拒绝（「永远拉不起来」）。
        //   按**命令行**精确匹配 dsh-supervisor 的 node 进程再杀，绝不误杀 DSH 自身的 node。
        crate::bounded::run_lossy(
            Command::new("powershell").args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                "Get-CimInstance Win32_Process -Filter \"Name='node.exe'\" | Where-Object { $_.CommandLine -like '*dsh-supervisor*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }",
            ]),
            SVC_NORMAL,
        );
        Ok(())
    }

}

/// 盘符是否为固定磁盘（结果按盘符缓存，每个盘符最多查询一次）。
#[cfg(target_os = "windows")]
fn drive_is_fixed(letter: u16) -> bool {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<u16, bool>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(m) = cache.lock() {
        if let Some(v) = m.get(&letter) {
            return *v;
        }
    }
    let mut root = [0u16; 4];
    root[0] = letter;
    root[1] = 58; // 冒号
    root[2] = 92; // 反斜杠
    root[3] = 0;
    extern "system" {
        fn GetDriveTypeW(lp_root_path_name: *const u16) -> u32;
    }
    // DRIVE_FIXED = 3；其余（REMOTE=4 / NO_ROOT_DIR=1 / UNKNOWN=0）一律跳过
    let fixed = unsafe { GetDriveTypeW(root.as_ptr()) } == 3;
    if let Ok(mut m) = cache.lock() {
        m.insert(letter, fixed);
    }
    fixed
}
