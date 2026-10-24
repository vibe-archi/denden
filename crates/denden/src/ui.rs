// Showing or collecting an invitation code without it ever passing through the AI conversation:
// a one-shot local page on 127.0.0.1, or the user's own terminal.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use base64::Engine;

use crate::core::random_hex;
use crate::http;

#[cfg(windows)]
fn hide_window(cmd: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000) // CREATE_NO_WINDOW
}
#[cfg(not(windows))]
fn hide_window(cmd: &mut Command) -> &mut Command {
    cmd
}

/// Opens `url` in the system browser. DENDEN_NO_BROWSER prevents it unless `force` (im open: an
/// explicit request). False when no browser could be started.
pub fn open_browser(url: &str, force: bool) -> bool {
    if !force && std::env::var_os("DENDEN_NO_BROWSER").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    let (cmd, args): (&str, Vec<&str>) = if cfg!(windows) {
        ("explorer.exe", vec![url])
    } else if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else {
        ("xdg-open", vec![url])
    };
    let Ok(mut child) = hide_window(Command::new(cmd).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())).spawn() else {
        return false;
    };
    // explorer.exe exits with 1 even when it opened the page; xdg-open and open report failures
    // (no browser, no display) through their exit status, and return once the browser is launched.
    if cfg!(windows) {
        return true;
    }
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(5) {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return false,
        }
    }
    true // still running: it is the browser itself
}

// ---------- the local page (pages/ui.html in the plugin) ----------

const TEMPLATE: &str = include_str!("../../../plugin/skills/im/pages/ui.html");

#[derive(Clone, Copy, PartialEq)]
pub enum Page {
    Invite,
    Join,
}

/// What the page shows: the template's CONFIG, less the language and the deadline.
pub struct PageConfig {
    pub page: Page,
    pub name: String,
    pub label: String,
    /// The invitation's expiry as the CLI prints it; "" for join.
    pub expires: String,
}

/// "fr" when Accept-Language starts with fr, else "en".
pub fn lang(accept: Option<&str>) -> &'static str {
    if accept.unwrap_or("").trim_start().get(..2).is_some_and(|p| p.eq_ignore_ascii_case("fr")) { "fr" } else { "en" }
}

/// The template with LANG, NONCE and CONFIG filled in. CONFIG is JSON with every "<" written as
/// <, so no value can close its script tag.
pub fn render_page(cfg: &PageConfig, lang: &str, nonce: &str, deadline_ms: u64) -> String {
    let config = serde_json::json!({
        "page": if cfg.page == Page::Invite { "invite" } else { "join" },
        "lang": lang,
        "name": cfg.name,
        "label": cfg.label,
        "expires": cfg.expires,
        "deadline": deadline_ms,
    })
    .to_string()
    .replace('<', "\\u003c");
    TEMPLATE.replacen("{{LANG}}", lang, 1).replacen("{{NONCE}}", nonce, 1).replacen("{{CONFIG}}", &config, 1)
}

/// What the page's API call answers: a JSON reply and, once the page has done its job, the value
/// the command waits for (the server closes about a second later). Err: a status and its message.
pub type ApiReply<T> = Result<(serde_json::Value, Option<T>), (u16, String)>;

/// Serves the page at a random URL on 127.0.0.1 until `api` returns a value, or `timeout`.
/// The page may be loaded any number of times: it holds no secret. Its single API action (POST
/// <page>/api/reveal for invite, /api/join for join) needs a JSON body, the x-denden header and
/// a same-origin request, so another site cannot call it.
pub fn serve_page<T>(cfg: &PageConfig, mut api: impl FnMut(&serde_json::Value) -> ApiReply<T>, on_ready: impl FnOnce(&str), timeout: Duration) -> Result<T, String> {
    let token = random_hex(16);
    let mut nonce_bytes = [0u8; 12];
    rand::fill(&mut nonce_bytes);
    let nonce = base64::engine::general_purpose::STANDARD.encode(nonce_bytes);
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let origin = format!("http://127.0.0.1:{}", listener.local_addr().map_err(|e| e.to_string())?.port());
    let host = origin["http://".len()..].to_string();
    let path = format!("/p/{token}");
    let action = format!("{path}/api/{}", if cfg.page == Page::Invite { "reveal" } else { "join" });
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let deadline_ms = crate::core::now_ms() + timeout.as_millis() as u64;
    let deadline = Instant::now() + timeout;
    on_ready(&format!("{origin}{path}"));

    let csp = format!(
        "default-src 'none'; style-src 'unsafe-inline'; img-src data:; script-src 'nonce-{nonce}'; connect-src 'self'; form-action 'none'; base-uri 'none'; frame-ancestors 'none'"
    );
    let mut done: Option<(T, Instant)> = None;
    loop {
        match &done {
            Some((_, at)) if at.elapsed() >= Duration::from_secs(1) => return Ok(done.take().map(|d| d.0).expect("set")),
            None if Instant::now() >= deadline => return Err("nobody answered on the local page in time".into()),
            _ => {}
        }
        let (mut stream, _) = match listener.accept() {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            Err(e) => return Err(e.to_string()),
        };
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
        let send = |stream: &mut std::net::TcpStream, code: u16, content_type: &str, body: &[u8], csp: Option<&str>| {
            let mut headers = vec![
                ("content-type", content_type),
                ("cache-control", "no-store"),
                ("x-content-type-options", "nosniff"),
                ("x-frame-options", "DENY"),
                ("referrer-policy", "no-referrer"),
            ];
            if let Some(c) = csp {
                headers.push(("content-security-policy", c));
            }
            let _ = stream.write_all(&http::response(code, &headers, body));
            let _ = stream.flush();
        };
        let send_json = |stream: &mut std::net::TcpStream, code: u16, v: &serde_json::Value| send(stream, code, "application/json", v.to_string().as_bytes(), None);
        let error = |stream: &mut std::net::TcpStream, code: u16, msg: &str| send_json(stream, code, &serde_json::json!({ "error": msg }));
        let Some(req) = http::read_request_sync(&mut stream, 64 * 1024) else {
            continue;
        };
        // Only addressed as 127.0.0.1:<port> (no DNS rebinding), only this page's paths.
        if req.header("host") != Some(host.as_str()) {
            error(&mut stream, 403, "forbidden");
        } else if req.path == path {
            if req.method == "GET" {
                let html = render_page(cfg, lang(req.header("accept-language")), &nonce, deadline_ms);
                send(&mut stream, 200, "text/html; charset=utf-8", html.as_bytes(), Some(&csp));
            } else {
                error(&mut stream, 405, "method not allowed");
            }
        } else if req.path == action {
            if req.method != "POST" {
                error(&mut stream, 405, "method not allowed");
                continue;
            }
            // A JSON body and a custom header: a page on another site cannot send them without a
            // CORS preflight, which is never answered. And the origin must be this page's.
            // Browsers may send "Origin: null" for a same-origin request (it depends on the
            // referrer policy), so Sec-Fetch-Site: same-origin is accepted then.
            let json_body = req.header("content-type").is_some_and(|c| c.split(';').next().unwrap_or("").trim().eq_ignore_ascii_case("application/json"));
            let o = req.header("origin");
            let same_origin =
                o == Some(origin.as_str()) || (o.is_none_or(|o| o.is_empty() || o == "null") && req.header("sec-fetch-site") == Some("same-origin"));
            if !json_body || req.header("x-denden") != Some("1") || !same_origin {
                error(&mut stream, 403, "forbidden");
                continue;
            }
            let Ok(body) = serde_json::from_slice::<serde_json::Value>(&req.body) else {
                error(&mut stream, 400, "bad request");
                continue;
            };
            match api(&body) {
                Ok((reply, value)) => {
                    send_json(&mut stream, 200, &reply);
                    if let (Some(v), None) = (value, &done) {
                        done = Some((v, Instant::now()));
                    }
                }
                Err((code, msg)) => error(&mut stream, code, &msg),
            }
        } else {
            error(&mut stream, 404, "not found");
        }
    }
}

// ---------- the user's own terminal (only when a human runs the command) ----------

pub fn read_stdin() -> String {
    let mut s = String::new();
    let _ = std::io::stdin().read_to_string(&mut s);
    s
}

/// An invitation code typed or pasted in the user's own terminal. It is not echoed, but its start
/// and end show as it comes, the middle masked (`denden_Ab3****************9fQ2`), so that a paste
/// can be seen. Ctrl+C stops, Backspace and Ctrl+U correct.
pub fn ask_hidden(question: &str) -> String {
    let mut code = String::new();
    let mut shown = 0;
    let draw = |code: &str, shown: &mut usize| {
        let m = masked(code);
        let pad = " ".repeat(shown.saturating_sub(m.chars().count()));
        eprint!("\r{question}{m}{pad}\r{question}{m}");
        let _ = std::io::stderr().flush();
        *shown = m.chars().count();
    };
    draw(&code, &mut shown);
    let restore = raw_mode();
    let mut stdin = std::io::stdin();
    let mut byte = [0u8; 1];
    loop {
        match stdin.read(&mut byte) {
            Ok(1) => {}
            _ => break,
        }
        match byte[0] {
            b'\r' | b'\n' => break,
            0x03 => {
                restore();
                eprintln!();
                std::process::exit(130);
            }
            0x08 | 0x7f => {
                code.pop();
            }
            0x15 => code.clear(),
            0x1b => {
                // An escape sequence (an arrow key, a bracketed paste marker): skipped.
                if stdin.read(&mut byte).is_ok_and(|n| n == 1) && byte[0] == b'[' {
                    while stdin.read(&mut byte).is_ok_and(|n| n == 1) && !(0x40..=0x7e).contains(&byte[0]) {}
                }
            }
            c if c.is_ascii_graphic() => code.push(c as char),
            _ => {}
        }
        draw(&code, &mut shown);
    }
    restore();
    eprintln!();
    code
}

/// Puts `text` in the clipboard of the computer this terminal runs on, with the OSC 52 sequence
/// (Windows Terminal, iTerm2, kitty, WezTerm; inside tmux, with set-clipboard or allow-passthrough
/// on). No answer comes back: false when it was not even tried.
pub fn osc52(text: &str) -> bool {
    if cfg!(windows) && std::env::var_os("WT_SESSION").is_none() {
        return false; // the old Windows console would print the sequence
    }
    let seq = format!("\x1b]52;c;{}\x07", base64::engine::general_purpose::STANDARD.encode(text));
    let mut out = std::io::stdout().lock();
    if std::env::var_os("TMUX").is_some() {
        let _ = write!(out, "\x1bPtmux;{}\x1b\\", seq.replace('\x1b', "\x1b\x1b"));
    }
    let _ = write!(out, "{seq}");
    let _ = out.flush();
    true
}

/// The start and the end of a code, the middle as stars: enough to see it arrived, not to use it.
pub fn masked(code: &str) -> String {
    let c: Vec<char> = code.chars().collect();
    let (head, tail) = (10, 4);
    if c.len() <= head {
        return code.to_string();
    }
    let start: String = c[..head].iter().collect();
    if c.len() <= head + tail {
        return format!("{start}{}", "*".repeat(c.len() - head));
    }
    let end: String = c[c.len() - tail..].iter().collect();
    format!("{start}{}{end}", "*".repeat((c.len() - head - tail).min(16)))
}

/// No echo, no line editing, no signal on Ctrl+C (handled by the caller): one byte at a time.
#[cfg(windows)]
fn raw_mode() -> impl FnOnce() {
    use windows_sys::Win32::System::Console::{ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE, SetConsoleMode};
    let h = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    let mut mode = 0u32;
    let ok = unsafe { GetConsoleMode(h, &mut mode) } != 0;
    if ok {
        unsafe { SetConsoleMode(h, mode & !(ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT)) };
    }
    move || {
        if ok {
            unsafe { SetConsoleMode(h, mode) };
        }
    }
}

#[cfg(unix)]
fn raw_mode() -> impl FnOnce() {
    let mut t: libc::termios = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::tcgetattr(0, &mut t) } == 0;
    let saved = t;
    if ok {
        t.c_lflag &= !(libc::ECHO | libc::ICANON | libc::ISIG);
        t.c_cc[libc::VMIN] = 1;
        t.c_cc[libc::VTIME] = 0;
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &t) };
    }
    move || {
        if ok {
            unsafe { libc::tcsetattr(0, libc::TCSANOW, &saved) };
        }
    }
}

/// http://127.0.0.1:<port>/p/<32 hex>: the only URLs `open` opens.
pub fn is_local_page(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://127.0.0.1:") else { return false };
    let Some((port, token)) = rest.split_once("/p/") else { return false };
    !port.is_empty() && port.len() <= 5 && port.bytes().all(|b| b.is_ascii_digit()) && token.len() == 32 && token.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_shows_its_ends_only() {
        assert_eq!(masked(""), "");
        assert_eq!(masked("denden_"), "denden_");
        assert_eq!(masked("denden_Ab3x9"), "denden_Ab3**");
        let code = format!("denden_{}9fQ2", "x".repeat(50));
        assert_eq!(masked(&code), format!("denden_xxx{}9fQ2", "*".repeat(16)));
    }

    #[test]
    fn page_renders_without_stray_text() {
        let cfg = PageConfig { page: Page::Invite, name: String::new(), label: "test-chan".into(), expires: "15:40".into() };
        let html = render_page(&cfg, "en", "nonce123", 0);
        // The header comment stays a comment: nothing but markup before <html>.
        let head = &html[..html.find("<html").expect("an html element")];
        let outside = head.replace("<!doctype html>", "");
        let outside = outside.trim();
        assert!(outside.starts_with("<!--") && outside.ends_with("-->"), "text outside the comment: {outside}");
        assert_eq!(outside.matches("<!--").count(), 1);
        assert!(!html.contains("{{"), "a placeholder left");
        assert!(html.contains("\"label\":\"test-chan\""));
    }

    #[test]
    fn local_pages_only() {
        assert!(is_local_page(&format!("http://127.0.0.1:8123/p/{}", "a1".repeat(16))));
        assert!(!is_local_page(&format!("http://127.0.0.1:8123/p/{}/api/reveal", "a1".repeat(16))));
        assert!(!is_local_page(&format!("http://localhost:8123/p/{}", "a1".repeat(16))));
        assert!(!is_local_page("https://example.com"));
    }
}
