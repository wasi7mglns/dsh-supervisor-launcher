//! 窗口导航与显示。
//!
//! 壳的窗口语义（产品形态）：
//!   · 关窗 = 隐藏到托盘（服务继续常驻），见 `main.rs` 的 CloseRequested；
//!   · 「退出管家」= 停全部服务链（见 `crate::domain::guardctl::shutdown_all`）。

use tauri::{Emitter, Manager};

pub(crate) fn go_panel(app: &tauri::AppHandle, force: bool) {
    let url = crate::env::api_base_url(); // http://127.0.0.1:<config.apiPort 或高位段 fallback>/
    // 壳框架(shell.html)的 evt listener 在首帧注册；setup 线程的 emit 可能早于注册被丢弃，
    // 故延时重发数次覆盖竞态（listener 就绪后任一次生效即切面板）。
    // force=true（如用户重新显示窗口）→ 即使 URL 相同也强制重载，保证拿到最新 UI。
    for (i, delay_ms) in [400u64, 1200, 2500].iter().enumerate() {
        let h = app.clone();
        let u = url.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(*delay_ms));
            let _ = h.emit("shell:goto-panel", serde_json::json!({ "url": u, "seq": i, "force": force }));
        });
    }
}

pub(crate) fn show_main(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
        // 每次显示都强制重新导航到面板：WebView 不做陈旧缓存，保证拿最新 UI（force=true）
        go_panel(app, true);
    }
}

// ═══════════════════════════════════════════════════════════════════
// 桌面壳自更新命令（2026-09-11）
//
// 三平台**同一代码路径**：检查 → 下载 → minisign 验签 → 平台安装 → 重启。
// 平台差异（Linux pkexec dpkg -i / macOS .app 替换 / Windows NSIS passive）
// 全部由 tauri-plugin-updater 内部处理，壳侧无平台分支。
//
// ⚠ 2026-09-13（P3 清理）：此处**原有一段没有宿主的文档注释** ——
//   它描述的命令（shell_update_check / shell_update_apply / shell_restart）
//   实际定义在 src/commands/mod.rs（命令层只做校验与委托，见门禁 G3：
//   main.rs 不得定义 #[tauri::command]）。文档留在 windowing.rs 会产生
//   「这里应该有一组命令」的误导 —— 读者会去找不存在的实现。
//   设计要点已上移到上面的块注释，本段只保留**指向**。
