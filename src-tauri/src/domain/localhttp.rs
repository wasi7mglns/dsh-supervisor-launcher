//! 本地 HTTP 客户端（与内核对齐的**最小**实现）。
//!
//! 用途：壳与内核之间的**本机回环**交互（握手、会话态、停止握手）。
//! 为什么不引 reqwest/ureq：这些请求极简（无 TLS / 无重定向 / 无连接复用），
//! 而壳是**安装器** —— 少一个依赖就少一份供应链与体积负担。
//!
//! ⚠ 不变量 B1（有界）：
//!   · 连接必须用 `connect_timeout` —— 防火墙 DROP 时 `connect` 会等到
//!     OS 的 SYN 重试耗尽（Windows 默认可达 20+ 秒）；
//!   · 读写必须设超时 —— 守卫挂起时不得让壳无限阻塞。
//!
//! ⚠ 不变量 B2（不阻塞 UI）：经 [`spawn_local_post`] 派发到独立线程 ——
//!   托盘回调里的网络 I/O 一旦阻塞，整个界面（含重绘）都会冻结。
use std::net::TcpStream;

pub(crate) fn post_local(port: u16, path: &str) {
    let _ = post_local_timeout(port, path, std::time::Duration::from_secs(60));
}

/// 把本地 API 调用派发到独立线程（**绝不阻塞 UI 线程**）。
///
/// 用于托盘菜单等由 UI 线程派发的回调：这些回调里的网络 I/O 一旦阻塞，
/// 整个界面（含重绘）都会被冻结 —— 实测观感是「点击无反应」。
///
/// 退出流程同样经此派发：即使守卫无响应，菜单也立即响应，
/// 用户不会觉得「程序关不掉」。
pub(crate) fn spawn_local_post(port: u16, path: &'static str) {
    std::thread::spawn(move || post_local(port, path));
}

/// 同 post_local，但带读写超时（防止守卫挂起时壳无限阻塞）。返回响应体（解码 utf8 尽力）。
/// 与本地守卫建立连接，**带连接超时**。
///
/// ⚠ 必须用 `connect_timeout` 而非 `connect`（2026-09-11 审计）：`TcpStream::connect`
///   **没有超时** —— 若端口被防火墙 DROP（而非 REJECT），连接会一直等到操作系统的
///   SYN 重试耗尽，Windows 上默认可达 20+ 秒。而本函数被 `guard_ready`（引导页每次
///   500ms 轮询一次、最多 40 次）与托盘动作调用，等同于反复长时间阻塞。
///   回环地址正常时是微秒级，但**不能依赖「正常时很快」来省略上限**。
pub(crate) fn connect_local(port: u16, timeout: std::time::Duration) -> Option<TcpStream> {
    use std::net::ToSocketAddrs;
    let addr = format!("127.0.0.1:{}", port);
    let sa = addr.to_socket_addrs().ok()?.next()?;
    TcpStream::connect_timeout(&sa, timeout).ok()
}

/// 本地 HTTP 请求的连接预算（回环地址，正常为微秒级；此处仅作兜底上限）。
const LOCAL_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(800);

pub(crate) fn post_local_timeout(port: u16, path: &str, timeout: std::time::Duration) -> Option<String> {
    let mut stream = connect_local(port, LOCAL_CONNECT_TIMEOUT)?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let req = format!(
        "POST {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        path, port
    );
    std::io::Write::write_all(&mut stream, req.as_bytes()).ok()?;
    let mut buf = Vec::new();
    let _ = std::io::Read::read_to_end(&mut stream, &mut buf);
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// 本地 HTTP GET（返回 (状态码, 全文)）——守卫就绪探针用。
pub(crate) fn http_get_local(port: u16, path: &str, timeout: std::time::Duration) -> Option<(u16, String)> {
    let mut stream = connect_local(port, LOCAL_CONNECT_TIMEOUT)?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let req = format!("GET {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n", path, port);
    std::io::Write::write_all(&mut stream, req.as_bytes()).ok()?;
    let mut buf = Vec::new();
    let _ = std::io::Read::read_to_end(&mut stream, &mut buf);
    let s = String::from_utf8_lossy(&buf).into_owned();
    let code = s.split_whitespace().nth(1).and_then(|c| c.parse::<u16>().ok()).unwrap_or(0);
    Some((code, s))
}

/// 读取会话态（GET /session/status 的最小解析：找 "sessionState":"xxx"）。
pub(crate) fn get_session_state(port: u16) -> Option<String> {
    let mut stream = connect_local(port, LOCAL_CONNECT_TIMEOUT)?;
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(3)));
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(3)));
    let req = format!("GET /session/status HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n", port);
    std::io::Write::write_all(&mut stream, req.as_bytes()).ok()?;
    let mut buf = Vec::new();
    let _ = std::io::Read::read_to_end(&mut stream, &mut buf);
    let s = String::from_utf8_lossy(&buf);
    let key = "\"sessionState\":\"";
    let i = s.find(key)? + key.len();
    let rest = &s[i..];
    let v: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    if v.is_empty() { None } else { Some(v) }
}
