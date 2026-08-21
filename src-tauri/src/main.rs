#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// dsh-supervisor-gui：桌面壳。
// 窗口加载守卫托管的面板（http://127.0.0.1:3100/）；
// 托盘常驻：关窗 = 隐藏到托盘，守卫状态随时可达；菜单动作直发本地 API。
// 对本地 API 的调用用裸 TCP 写 HTTP，不引入任何额外依赖。

use tauri::{
    menu::{Menu, MenuItem},
    tray::{TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// 向守卫本地 API 发一个无 body 的 POST（仅回环，无鉴权边界内）。
fn post_local(port: u16, path: &str) {
    let addr = format!("127.0.0.1:{}", port);
    if let Ok(mut stream) = TcpStream::connect(&addr) {
        let req = format!(
            "POST {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            path, port
        );
        let _ = stream.write_all(req.as_bytes());
        let mut buf = Vec::new();
        let _ = stream.read_to_end(&mut buf);
    }
}

fn guard_alive(port: u16) -> bool {
    let addr = format!("127.0.0.1:{}", port);
    match addr.parse() {
    Ok(a) => TcpStream::connect_timeout(&a, Duration::from_millis(400)).is_ok(),
        Err(_) => false,
    }
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            // 守卫 API 端口固定默认 3100；如用户改配，托盘控制以面板为准
            let port: u16 = std::env::var("DSH_SUPERVISOR_TRAY_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(3100);

            // 守卫没起就先拉起（GUI 先于服务打开的常见场景），失败静默交由用户排查
            if !guard_alive(port) {
                let _ = std::process::Command::new("systemctl")
                    .args(["--user", "start", "dsh-supervisor.service"])
                    .status();
            }

            let show = MenuItem::with_id(app, "show", "显示面板", true, None::<&str>)?;
            let start = MenuItem::with_id(app, "start", "启动 DSH", true, None::<&str>)?;
            let stop = MenuItem::with_id(app, "stop", "停止 DSH", true, None::<&str>)?;
            let restart = MenuItem::with_id(app, "restart", "重启一次", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出托盘", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &start, &stop, &restart, &quit])?;

            TrayIconBuilder::with_id("dsh-supervisor-tray")
                .icon(app.default_window_icon().expect("no default icon").clone())
                .tooltip("dsh-supervisor")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(move |app, event| {
                    match event.id.as_ref() {
                        "show" => show_main(app),
                        "start" => post_local(port, "/start"),
                        "stop" => post_local(port, "/stop"),
                        "restart" => post_local(port, "/restart"),
                        "quit" => app.exit(0),
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { .. } = event {
                        show_main(tray.app_handle());
                    }
                })
                .build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            // 关窗 = 隐藏到托盘常驻；真正退出走托盘菜单「退出托盘」
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running dsh-supervisor-gui");
}
