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
}