//! 结构化错误模型（2026-09-11）。
//!
//! == 为什么需要（用户指正：「壳是乱的，没有架构」）==
//!
//! 立项时审计实测：`Result<_, String>` **42 处**，自定义 Error 枚举 **0 个**。
//! 后果：
//!   · 前端只能拿到一句可能判错的自然语言，无法按 `kind` 给不同建议；
//!   · 「探测超时」与「配置错误」在类型上无法区分 —— 用户看到的提示往往是错的；
//!   · 无法程序化处理（重试 / 降级 / 上报 都只能靠字符串匹配）。
//!
//! == 设计 ==
//!
//! · 用 `#[serde(tag = "kind")]` 序列化 → 前端 `switch (e.kind)` 即可分支；
//! · `Probe` **必须带 stage 与 elapsed_ms** —— 「卡住时看得见」是硬要求
//!   （「环境探测卡死」事故的直接教训：没有阶段信息就只能猜）；
//! · `Unsupported` 让「未实现的能力」成为**类型上可见**的事实，
//!   而不是一句 `Err("不支持".into())`（与平台层的 P1 不变量同规）。
//!
//! == 接入位置（现状）==
//!
//! **IPC 边界**：`commands/` 的 `Result` 命令都返回 [`ShellResult`] ——
//! 前端因此拿到结构化对象，并按 `kind` 分支、显示后端给的 `hint`
//! （见 `bootstrap/js/10-ui.js` 的 `errText()`）。
//!
//! ⚠ 上述「显示后端给的 `hint`」在 2026-09-12 之前**并不成立**：
//!   `hint()` 只是 Rust 方法，**从未进入 JSON**，前端 `e.hint` 恒为 `undefined` ——
//!   即注释声称的能力当时并不存在（典型「注释声称、代码没有」）。
//!   现由手工 `Serialize` 把 `hint` 并入输出（见本文件下方 impl），并有测试锁定：
//!   `every_kind_serializes_a_hint` / `serialization_keeps_original_field_names`。
//!
//! ⚠ 有一处**例外**：`guard_start` 的 JSON 响应体里 `error` 仍是字符串（`e.to_string()`）。
//!   因为该字段被前端当字符串拼接；塞入对象会显示 `[object Object]`。
//!
//! **内部函数**仍多用 `Result<_, String>`：它们经 `From<String>` 在 IPC 边界自动升级。
//! 这是**有意的渐进迁移** —— 内部签名是否结构化不影响前端收益，不为改造而改造。

use serde::Serialize;

/// 壳的结构化错误。
///
/// ⚠ `hint` 是**序列化字段**（P2 修复，2026-09-12）。
///
///   此前 `hint()` 只是一个 Rust 方法，**从未进入 JSON** ——
///   而前端 `10-ui.js::errText()` 写着 `e.hint ? (detail + '；' + e.hint) : detail`，
///   于是 `e.hint` 恒为 `undefined`：
///     · 「可尝试切换镜像源」「请按系统提示完成授权」这类**可操作建议永远到不了用户**；
///     · 而 `error.rs` 头部注释却声称「前端按 kind 分支、显示后端给的 hint」——
///       又一处「注释声称、代码没有」。
///
///   修法：把 `hint` 作为字段并入序列化输出（`#[serde(serialize_with)]`），
///   使 `#[serde(tag = "kind")]` 的结构体里多出一个 `hint: String` 键。
#[derive(Debug, Clone)]
pub enum ShellError {
    /// 探测失败：**必须带阶段与耗时**（「卡住时看得见」）。
    Probe {
        /// 失败发生在哪一步（如 `enumerate` / `version` / `path-scan`）。
        stage: String,
        cause: String,
        elapsed_ms: u128,
    },
    /// 网络请求失败。
    Network { url: String, cause: String },
    /// 安装失败（Node 或内核）。
    Install { platform: String, cause: String },
    /// 服务（systemd / launchd / schtasks）操作失败。
    Service { action: String, cause: String },
    /// 与内核的契约文件读写失败。
    Contract { file: String, cause: String },
    /// IPC 边界自身的问题（参数非法等）。
    Ipc { cause: String },
    /// **显式不支持**（替代静默成功 / 裸字符串）。
    Unsupported { capability: String, platform: String },
}

/// 手工 `Serialize`：派生表示 + 一个 `hint` 字段（P2 修复）。
///
/// ⚠ 为什么不直接 `#[derive(Serialize)]`：`hint()` 是**方法**，派生不会带上它。
///   而前端 `errText()` 消费的正是 `e.hint` —— 不序列化等于那句建议永远发不出去。
///
/// ⚠ 字段格式必须与原先**逐字一致**（`kind` 用 kebab-case 变体名，字段名保持 snake_case），
///   否则前端已有的 `e.stage` / `e.cause` / `e.elapsed_ms` 读取会失效。
impl Serialize for ShellError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut v = match self {
            ShellError::Probe {
                stage,
                cause,
                elapsed_ms,
            } => serde_json::json!({
                "kind": "probe",
                "stage": stage,
                "cause": cause,
                "elapsed_ms": elapsed_ms,
            }),
            ShellError::Network { url, cause } => serde_json::json!({
                "kind": "network",
                "url": url,
                "cause": cause,
            }),
            ShellError::Install { platform, cause } => serde_json::json!({
                "kind": "install",
                "platform": platform,
                "cause": cause,
            }),
            ShellError::Service { action, cause } => serde_json::json!({
                "kind": "service",
                "action": action,
                "cause": cause,
            }),
            ShellError::Contract { file, cause } => serde_json::json!({
                "kind": "contract",
                "file": file,
                "cause": cause,
            }),
            ShellError::Ipc { cause } => serde_json::json!({
                "kind": "ipc",
                "cause": cause,
            }),
            ShellError::Unsupported {
                capability,
                platform,
            } => serde_json::json!({
                "kind": "unsupported",
                "capability": capability,
                "platform": platform,
            }),
        };
        // 这里就是「漏接线」的那一步：把可操作建议并入输出。
        if let Some(o) = v.as_object_mut() {
            o.insert(
                "hint".to_string(),
                serde_json::Value::String(self.hint().to_string()),
            );
        }
        serde::Serialize::serialize(&v, serializer)
    }
}

impl ShellError {
    /// 探测失败（带阶段与耗时）。
    pub fn probe(stage: &str, cause: impl Into<String>, elapsed_ms: u128) -> Self {
        ShellError::Probe {
            stage: stage.to_string(),
            cause: cause.into(),
            elapsed_ms,
        }
    }

    /// 网络失败。
    pub fn network(url: &str, cause: impl Into<String>) -> Self {
        ShellError::Network {
            url: url.to_string(),
            cause: cause.into(),
        }
    }

    /// IPC 参数非法。
    pub fn ipc(cause: impl Into<String>) -> Self {
        ShellError::Ipc {
            cause: cause.into(),
        }
    }

    /// 后端给前端的**可操作建议**。
    ///
    /// 单独暴露（而非让前端拼字符串）—— 建议文案属于知识，应随 kind 一起演进，
    /// 且三平台/多语言时只需改一处。
    pub fn hint(&self) -> &'static str {
        match self {
            ShellError::Probe { .. } => "探测超时。可检查网络与代理设置后重试；诊断信息含失败阶段与耗时。",
            ShellError::Network { .. } => "网络请求失败。可尝试切换镜像源（设定 → 镜像源）后重试。",
            ShellError::Install { .. } => "安装失败。若为权限问题，请按系统提示完成授权后重试。",
            ShellError::Service { .. } => "服务操作失败。可尝试手动启动一次桌面壳以重建服务定义。",
            ShellError::Contract { .. } => "配置读取失败。可删除该文件后重启（将自动重建）。",
            ShellError::Ipc { .. } => "内部调用参数错误（通常为前后端版本不一致）。请更新到最新版本。",
            ShellError::Unsupported { .. } => "当前平台不支持该能力。请参考文档中的平台能力矩阵。",
        }
    }
}

/// 便于既有 `Result<_, String>` 代码用 `?` 直接升级（渐进迁移）。
impl From<String> for ShellError {
    fn from(s: String) -> Self {
        ShellError::Ipc { cause: s }
    }
}

impl From<&str> for ShellError {
    fn from(s: &str) -> Self {
        ShellError::Ipc {
            cause: s.to_string(),
        }
    }
}

/// 兼容既有 `Result<T, String>` 签名：把结构化错误压平为**人类可读**字符串
/// （含 kind 与建议），供暂时未迁移的调用点使用。
impl From<ShellError> for String {
    fn from(e: ShellError) -> Self {
        format!("{}（{}）：{}", e.kind_label(), e.hint(), e)
    }
}

impl std::fmt::Display for ShellError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShellError::Probe {
                stage,
                cause,
                elapsed_ms,
            } => write!(f, "探测失败于 {}（{}ms）：{}", stage, elapsed_ms, cause),
            ShellError::Network { url, cause } => write!(f, "网络失败 {}：{}", url, cause),
            ShellError::Install { platform, cause } => {
                write!(f, "{} 安装失败：{}", platform, cause)
            }
            ShellError::Service { action, cause } => {
                write!(f, "服务操作 {} 失败：{}", action, cause)
            }
            ShellError::Contract { file, cause } => {
                write!(f, "契约读写失败 {}：{}", file, cause)
            }
            ShellError::Ipc { cause } => write!(f, "IPC 错误：{}", cause),
            ShellError::Unsupported {
                capability,
                platform,
            } => write!(f, "{} 不支持 {}", platform, capability),
        }
    }
}

impl std::error::Error for ShellError {}

impl ShellError {
    /// `kind` 的稳定字符串（与 serde 的 tag 一致）。
    pub fn kind_label(&self) -> &'static str {
        match self {
            ShellError::Probe { .. } => "probe",
            ShellError::Network { .. } => "network",
            ShellError::Install { .. } => "install",
            ShellError::Service { .. } => "service",
            ShellError::Contract { .. } => "contract",
            ShellError::Ipc { .. } => "ipc",
            ShellError::Unsupported { .. } => "unsupported",
        }
    }
}

/// 壳的统一 Result 别名。
pub type ShellResult<T> = Result<T, ShellError>;

#[cfg(test)]
mod tests {
    use super::*;

    /// 探测错误**必须**带阶段与耗时 —— 这是「卡住时看得见」的机器保证。
    #[test]
    fn probe_error_carries_stage_and_elapsed() {
        let e = ShellError::probe("enumerate", "read_dir 超时", 25000);
        let j = serde_json::to_value(&e).unwrap();
        assert_eq!(j["kind"], "probe");
        assert_eq!(j["stage"], "enumerate");
        assert_eq!(j["elapsed_ms"], 25000);
    }

    /// 每个变体都必须能序列化出稳定 `kind`（前端据此分支）。
    #[test]
    fn all_kinds_are_stable() {
        let cases: Vec<(ShellError, &str)> = vec![
            (ShellError::probe("s", "c", 1), "probe"),
            (ShellError::network("u", "c"), "network"),
            (
                ShellError::Install {
                    platform: "linux".into(),
                    cause: "c".into(),
                },
                "install",
            ),
            (
                ShellError::Service {
                    action: "start".into(),
                    cause: "c".into(),
                },
                "service",
            ),
            (
                ShellError::Contract {
                    file: "f".into(),
                    cause: "c".into(),
                },
                "contract",
            ),
            (ShellError::ipc("c"), "ipc"),
            (
                ShellError::Unsupported {
                    capability: "x".into(),
                    platform: "linux".into(),
                },
                "unsupported",
            ),
        ];
        for (e, want) in cases {
            let j = serde_json::to_value(&e).unwrap();
            assert_eq!(j["kind"], want, "变体 {} 的 kind 不稳定", want);
            assert_eq!(e.kind_label(), want);
        }
    }

    /// 每个变体都有**非空建议**（前端不能拿到空提示）。
    #[test]
    fn every_kind_has_a_hint() {
        let all = [
            ShellError::probe("s", "c", 1),
            ShellError::network("u", "c"),
            ShellError::Install {
                platform: "p".into(),
                cause: "c".into(),
            },
            ShellError::Service {
                action: "a".into(),
                cause: "c".into(),
            },
            ShellError::Contract {
                file: "f".into(),
                cause: "c".into(),
            },
            ShellError::ipc("c"),
            ShellError::Unsupported {
                capability: "x".into(),
                platform: "p".into(),
            },
        ];
        for e in all {
            assert!(!e.hint().is_empty(), "{} 缺少建议", e.kind_label());
        }
    }

    /// **`hint` 必须真的进入 JSON**（P2 回归，2026-09-12）。
    ///
    /// 上面那条只检查了 **Rust 方法**非空 —— 而本次缺陷正是：
    ///   方法存在、前端也读 `e.hint`，但 `hint()` **从未被序列化** → `e.hint` 恒 undefined。
    ///   于是「可尝试切换镜像源」这类建议永远到不了用户，
    ///   而文件头注释却声称前端会显示它。
    ///
    /// 故本测试断言的是**序列化输出**，并同时校验「JSON 里的值与方法返回一致」
    /// （防两处各写一份而漂移）。
    #[test]
    fn every_kind_serializes_a_hint() {
        let all = [
            ShellError::probe("s", "c", 1),
            ShellError::network("u", "c"),
            ShellError::Install {
                platform: "p".into(),
                cause: "c".into(),
            },
            ShellError::Service {
                action: "a".into(),
                cause: "c".into(),
            },
            ShellError::Contract {
                file: "f".into(),
                cause: "c".into(),
            },
            ShellError::ipc("c"),
            ShellError::Unsupported {
                capability: "x".into(),
                platform: "p".into(),
            },
        ];
        for e in all {
            let j = serde_json::to_value(&e).unwrap();
            let h = j.get("hint").and_then(|x| x.as_str()).unwrap_or("");
            assert!(
                !h.is_empty(),
                "{} 的 JSON 里没有 hint —— 前端 e.hint 会是 undefined",
                e.kind_label()
            );
            assert_eq!(h, e.hint(), "{} 的 JSON hint 与方法返回不一致", e.kind_label());
        }
    }

    /// 手工 `Serialize` 不得改动原有字段名/值（前端已按它们读取）。
    #[test]
    fn serialization_keeps_original_field_names() {
        let p = serde_json::to_value(ShellError::probe("path-scan", "卡住", 999)).unwrap();
        assert_eq!(p["kind"], "probe");
        assert_eq!(p["stage"], "path-scan");
        assert_eq!(p["cause"], "卡住");
        assert_eq!(p["elapsed_ms"], 999);

        let c = serde_json::to_value(ShellError::Contract {
            file: "a.json".into(),
            cause: "bad".into(),
        })
        .unwrap();
        assert_eq!(c["file"], "a.json");
        assert_eq!(c["cause"], "bad");

        let u = serde_json::to_value(ShellError::Unsupported {
            capability: "cap".into(),
            platform: "plat".into(),
        })
        .unwrap();
        assert_eq!(u["capability"], "cap");
        assert_eq!(u["platform"], "plat");
    }
}