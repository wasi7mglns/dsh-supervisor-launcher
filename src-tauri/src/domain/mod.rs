//! 业务层（**平台无关**）—— 2026-09-11 从 `main.rs` 拆出。
//!
//! 分层与依赖方向（硬约束）：
//!
//! ```text
//! commands ──▶ domain ──▶ platform ──▶ infra
//! ```
//!
//! · `domain` **不得**出现平台分支（门禁 G1）—— 平台差异一律经 `platform` 层；
//! · `domain` **不得**依赖 `commands`（命令层只做校验与委托）。
//!
//! 拆出这些模块的直接收益：`main.rs` 从「什么都有」变成「只做组装」，
//! 且每个模块可独立测试（原先它们与 Tauri 的 `AppHandle` 纠缠在一起）。
pub(crate) mod cli;
pub(crate) mod coreloc;
pub(crate) mod guardctl;
pub(crate) mod localhttp;
pub(crate) mod windowing;
