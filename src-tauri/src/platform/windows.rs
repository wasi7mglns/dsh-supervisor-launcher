//! Windows 平台实现（计划任务 schtasks）。
//!
//! 本文件是 Windows 的**全部**平台知识（门禁 G1）。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::service::ServiceControl;
use super::{home_dir, Capabilities, Platform, SVC_NORMAL, SVC_QUICK};

pub const NAME: &str = "windows";
/// 计划任务名（**定义由本文件建立**；内核只做 /ENABLE /DISABLE）。
pub const GUARD_TASK: &str = "DSH-Supervisor";
/// 崩溃自拉的保活任务（内核 `autostart.js` 建立；停止守卫时须先停它）。
pub const WATCHDOG_TASK: &str = "DSH-Supervisor-Watchdog";

/// 安装命令超时（15 分钟：下载 + msiexec + UAC 授权）。
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
            privilege_channel: true, // UAC / msiexec
            node_artifact: "msi",
        }
    }

    fn node_artifact(&self, version: &str) -> Option<super::NodeArtifact> {
        // 官方**没有** win-arm64-msi（files[] 只有 win-arm64-7z / win-arm64-zip）。
        // 故 arm64 Windows 也取 x64 msi —— 依赖系统的 x64 模拟执行。
        // 这是**有意为之的折中**（原生 arm64 需改用 zip 解包，当前未实现），
        // 并在本方法的文档与 --platform-matrix 输出中如实记录。
        Some(super::NodeArtifact {
            tag: "win-x64-msi",
            file: format!("node-v{}-x64.msi", version),
        })
    }

    fn node_candidate_paths(&self) -> Vec<PathBuf> {
        // ⚠ 不得硬编码 C:\Program Files（2026-09-11 审计）：
        //   真实路径随**系统盘符**与**系统语言**变化（中文系统是本地化目录名），
        //   也可能装在 Program Files (x86)。故一律经环境变量推导。
        let exe = "node.exe";
        let mut v: Vec<PathBuf> = Vec::new();
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
        std::env::var("ProgramFiles")
            .map(|b| PathBuf::from(b).join("nodejs").join("node.exe"))
            .unwrap_or_else(|_| PathBuf::from("C:\\Program Files\\nodejs\\node.exe"))
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
        let abs = file.canonicalize().map_err(|e| e.to_string())?;
        let esc = abs.display().to_string().replace('"', "");
        let ps = format!(
            "Start-Process -FilePath msiexec -ArgumentList '/i','{}','/qn','/norestart' -Verb RunAs -Wait",
            esc
        );
        let out = crate::bounded::run(
            Command::new("powershell").args(["-NoProfile", "-Command", &ps]),
            INSTALL_CMD_TIMEOUT,
        )
        .map_err(|e| format!("无法启动 powershell: {}", e))?;
        if !out.success {
            return Err(format!(
                "Windows 安装失败（用户取消 UAC 或 msiexec 报错）: {}",
                out.stderr.trim()
            ));
        }
        Ok(self.node_bin_after_install())
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

    fn is_local_fixed_dir(&self, dir: &Path) -> bool {
        use std::os::windows::ffi::OsStrExt;
        // ⚠ 为什么需要这个判定（真实事故）：
        //   Path::is_file() / canonicalize() 在**断开的映射盘**或 UNC 路径上
        //   会触网并阻塞数十秒，而调用点在引导的**关键路径**上（每步都要探测候选）。
        //   判定本身（GetDriveTypeW）不触网。
        //
        // ⚠ 按**盘符**缓存：PATH 里数十个条目往往集中在同一两个盘符，
        //   原实现对每个条目都调一次 API —— 同一个盘被反复查询，
        //   一旦该盘有问题就重复付出阻塞代价。现每个盘符只查一次。
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
        // Windows 的 UAC 提权**恒可用**（msiexec -Verb RunAs）。
        true
    }
}

impl ServiceControl for Impl {
    fn kind(&self) -> &'static str {
        "schtasks"
    }

    fn definition_path(&self) -> PathBuf {
        // 计划任务不是文件；返回标识串供日志/诊断（`is_file()` 恒 false 是有意为之）。
        PathBuf::from(format!("schtasks://{}", GUARD_TASK))
    }

    /// 建立计划任务（幂等）。
    fn ensure_defined(&self, guard: &Path) -> Result<String, String> {
        if let Ok(o) = crate::bounded::run(
            Command::new("schtasks").args(["/Query", "/TN", GUARD_TASK]),
            SVC_QUICK,
        ) {
            if o.success {
                return Ok(format!("已存在 计划任务 {}", GUARD_TASK));
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
        // ⚠ `/RL HIGHEST` 需要相应权限（2026-09-11 审计）：
        //   在**非提权**会话中创建「以最高权限运行」的计划任务可能被拒（Access is denied）。
        //   而壳默认以普通用户权限运行 —— 若首次尝试失败，退化为普通权限任务，
        //   保证「服务定义一定能建立」（守卫本身不需要管理员权限，它只管理当前用户的 DSH）。
        //   原则：**权限不足时应降级而非彻底失败**，否则用户会卡在「守卫就绪」。
        let base = |rl: Option<&str>| {
            let mut c = Command::new("schtasks");
            c.args(["/Create", "/TN", GUARD_TASK, "/SC", "ONLOGON"]);
            if let Some(level) = rl {
                c.args(["/RL", level]);
            }
            c.args(["/F", "/TR", &wrapper.display().to_string()]);
            c
        };

        let first = crate::bounded::run(&mut base(Some("HIGHEST")), SVC_NORMAL)?;
        if first.success {
            return Ok(format!(
                "已建立 计划任务 {}（最高权限）-> {}",
                GUARD_TASK,
                wrapper.display()
            ));
        }
        // 降级重试（去掉 /RL HIGHEST）
        let second = crate::bounded::run(&mut base(None), SVC_NORMAL)?;
        if second.success {
            return Ok(format!(
                "已建立 计划任务 {}（普通权限，HIGHEST 被拒）-> {}",
                GUARD_TASK,
                wrapper.display()
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
        crate::bounded::run_lossy(
            Command::new("schtasks").args(["/End", "/TN", WATCHDOG_TASK]),
            SVC_NORMAL,
        );
        crate::bounded::run_lossy(
            Command::new("schtasks").args(["/End", "/TN", GUARD_TASK]),
            SVC_NORMAL,
        );
        crate::bounded::run_lossy(
            Command::new("taskkill").args(["/F", "/IM", "dsh-supervisor.exe"]),
            SVC_NORMAL,
        );
        Ok(())
    }

    fn spawn_daemon(&self, guard: &Path) -> Result<u32, String> {
        use std::os::windows::process::CommandExt;
        // ⚠ 引号处理必须正确（2026-09-11 修复）：
        //   npm 全局安装的守卫是 `dsh-supervisor.cmd` 垫片，须经 `cmd /C` 启动。
        //   而旧写法 `.args(["/C", path, "daemon"])` 在**路径含空格**时会被 cmd 拆错 ——
        //   而 `%APPDATA%` 形如 `C:\Users\<用户名>\AppData\Roaming`，
        //   Windows 用户名**可以含空格**（如 "John Smith"），故此风险真实存在。
        //   症状是「守卫启动失败」，且错误信息难以解读（cmd 报路径语法错误）。
        //
        //   正确形态（cmd 的经典引号规则）：整个命令用**外层引号**包住，
        //   路径自身再包一层 —— 即 `cmd /C ""<path>" daemon"`。
        //   用 raw_arg 直接给出该形式，避免 Rust 再次转义。
        let line = format!("\"\"{}\" daemon\"", guard.display());
        let child = Command::new("cmd")
            .arg("/C")
            .raw_arg(line)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .spawn()
            .map_err(|e| format!("直接拉起守卫失败: {}", e))?;
        Ok(child.id())
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
