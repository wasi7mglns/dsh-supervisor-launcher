//! 服务控制契约（**定义 + 启停** 同一对象）。
//!
//! == 本契约修复的分层违规 ==
//!
//! 改造前：
//!   · 顶层 `service.rs`（**已删除**）—— 服务**定义**（`ensure_defined`，含 4 份 `#[cfg]`）
//!   · `main.rs`      —— 服务**启停**（`start_guard_service` / `stop_guard_service`，另 8 份 `#[cfg]`）
//!
//! 同一概念的 per-OS 知识分居两层，是「加一个平台要改两处不同层」的根因。
//! 现统一到本 trait：**定义与启停永远是同一个对象**，平台实现无法只改一半。
//!
//! == 不变量 ==
//!
//! · **P1 显式性**：不支持的能力返回 `Err(Unsupported …)`，**绝不静默成功**。
//! · **B1 有界性**：所有外部命令经 [`crate::bounded`]（超时即 kill）。
//! · **幂等性**：`ensure_defined` 对已存在的定义直接返回成功。

use std::path::{Path, PathBuf};

pub trait ServiceControl: Send + Sync {
    /// 服务管理器种类（`systemd` / `launchagent` / `schtasks` / `none`）。
    fn kind(&self) -> &'static str;

    /// 服务定义文件的规范路径。
    ///
    /// Windows 的计划任务没有文件，返回标识串（`schtasks://DSH-Supervisor`）供日志。
    fn definition_path(&self) -> PathBuf;

    /// 建立服务定义（幂等）。
    ///
    /// 返回人类可读的状态描述。**注意**：`enable` 失败不应返回 Err ——
    /// 定义已写入时仍可在 `start` 阶段拉起（并另有 `spawn_daemon` 兜底），
    /// 把「可继续」误判为「彻底失败」会让用户卡在引导页。
    fn ensure_defined(&self, guard: &Path) -> Result<String, String>;

    /// 请求服务管理器启动（不直接 spawn）。
    fn start(&self) -> Result<(), String>;

    /// 请求服务管理器停止（守卫的所有者动作）。
    fn stop(&self) -> Result<(), String>;

    /// 服务管理器不可用时的**直接 spawn 兜底**。
    ///
    /// 设计取舍：项目原约束为「壳绝不直接 spawn 守卫」（避免游离于服务管理器的
    /// 第二实例），但该约束不能凌驾于**可用性**之上 —— 容器、无 user systemd
    /// session、launchctl 被策略拦截、schtasks 被组策略禁止等场景下服务管理器
    /// 根本无法使用，若无兜底用户被永久挡在门外。
    ///
    /// 第二实例风险由调用方规避：spawn 前已确认端口不存活，
    /// 且 spawn 后仍以「端口就绪」为唯一成功判据（而非进程是否存活）。
    fn spawn_daemon(&self, guard: &Path) -> Result<u32, String>;
}