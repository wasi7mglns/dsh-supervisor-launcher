//! 未支持平台的显式实现（**绝不静默成功**）。
//!
//! 设计原则（与内核 `platform/os/service.js` 的 `CapabilityError` 同规）：
//!
//! 反面教材（本项目已发生过两次）：
//!
//! 本文件让「不支持」成为**编译期存在、运行期可见**的事实。

use std::path::{Path, PathBuf};

use super::service::ServiceControl;
use super::{Capabilities, Platform};

pub const NAME: &str = "unsupported";

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
            native_service: false,
            privilege_channel: false,
            node_artifact: "unknown",
        }
    }
}

impl ServiceControl for Impl {
    fn kind(&self) -> &'static str {
        "none"
    }

    fn definition_path(&self) -> PathBuf {
        PathBuf::from("unsupported")
    }

    fn ensure_defined(&self, _guard: &Path) -> Result<String, String> {
        Err(format!("当前平台（{}）不支持服务定义", std::env::consts::OS))
    }

    fn start(&self) -> Result<(), String> {
        Err(format!("当前平台（{}）不支持守卫服务管理", std::env::consts::OS))
    }

    fn stop(&self) -> Result<(), String> {
        Err(format!("当前平台（{}）不支持守卫服务管理", std::env::consts::OS))
    }

    fn spawn_daemon(&self, guard: &Path) -> Result<u32, String> {
        // spawn 是**平台无关**的兜底能力（进程启动本身处处可用），
        // 故这里不像服务管理那样直接拒绝 —— 但必须在文档与自检中如实反映
        // 「本平台没有原生服务管理器，只有 spawn 兜底」。
        use std::process::{Command, Stdio};
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

// ⚠ 2026-09-12（P1 修复）：**这里必须收束 ServiceControl，另起 Platform 的 impl**。
//
//   缺陷：原实现把 Platform 的 11 个必需方法（node_artifact / node_candidate_paths /
//     node_bin_after_install / is_usable_executable / core_extra_candidates /
//     is_local_fixed_dir / install_node / has_privilege_channel / node_exe_name /
//     npm_exe_name / core_exe_names）写在了 ServiceControl 的 impl 块内 ——
//     而 ServiceControl 只有 6 个方法（kind/definition_path/ensure_defined/start/stop/spawn_daemon），
//     于是：(a) Platform 的 impl 缺 11 个必需方法；(b) 这 11 个方法在 ServiceControl 里
//     属「不属于该 trait 的项」。两者都是编译错误。
//
//   为何从未暴露：platform/mod.rs 用
//       #[cfg(not(any(target_os = "linux", "macos", "windows")))]
//     把本模块整体排除 —— 三大平台构建根本不编译它。
//     于是「未知平台显式 Unsupported、绝不静默成功」的承诺在源码层不成立：
//     任何移植/新增 target 的那一刻就会编译失败。
//
//   修法：在此收束 ServiceControl，另起 Platform 的 impl。
//   门禁：tests/platform_unsupported_structure_test.rs（U-a/U-b/U-c/U-d）。
impl Platform for Impl {
    fn node_artifact(&self, _version: &str) -> Option<super::NodeArtifact> {
        // 未知平台：**显式返回 None**（无可用制品），而不是猜一个。
        None
    }

    fn node_candidate_paths(&self) -> Vec<PathBuf> {
        // 只给通用 Unix 落点（未知平台多半是类 Unix）；探测失败由调用方如实报告。
        vec![
            PathBuf::from("/usr/local/bin/node"),
            PathBuf::from("/usr/bin/node"),
        ]
    }

    fn node_bin_after_install(&self) -> PathBuf {
        PathBuf::from("/usr/local/bin/node")
    }

    fn is_usable_executable(&self, cand: &Path) -> bool {
        cand.is_file()
    }

    fn core_extra_candidates(&self, _names: &[&str], _pkg: Option<&str>) -> Vec<PathBuf> {
        Vec::new()
    }

    fn is_local_fixed_dir(&self, _dir: &Path) -> bool {
        // Unix：无「网络盘 / 可移动盘」概念上的 is_file() 触网风险，
        // 本地文件系统调用不会因路径本身而阻塞数十秒。
        true
    }

    fn install_node(&self, _file: &Path) -> Result<PathBuf, String> {
        Err(format!(
            "当前平台（{}）不支持自动安装 Node —— 请手动安装后重启桌面壳",
            std::env::consts::OS
        ))
    }

    fn has_privilege_channel(&self) -> bool {
        false
    }

    // ── 可执行文件名的平台差异（P2/G1）──
    // 未知平台按 POSIX 形态给出（保守：至少不引入 Windows 专有扩展名）。
    fn node_exe_name(&self) -> &'static str { "node" }
    fn npm_exe_name(&self) -> &'static str { "npm" }
    fn core_exe_names(&self) -> &'static [&'static str] { &["dsh-supervisor"] }
}