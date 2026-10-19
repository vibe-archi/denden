// Just enough HTTP/1.1 for the daemon's localhost API, the one-shot local pages and the CLI that
// talks to them: one request per connection, Content-Length or chunked bodies.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

pub struct Request {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
    pub fn q(&self, name: &str) -> Option<&str> {
        self.query.get(name).map(String::as_str).filter(|v| !v.is_empty())
    }
}

/// application/x-www-form-urlencoded, like URLSearchParams (later keys win, as Object.fromEntries).
pub fn parse_form(s: &str) -> HashMap<String, String> {
    s.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (url_decode(k), url_decode(v))
        })
        .collect()
}

pub fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() && hex(b[i + 1]).is_some() && hex(b[i + 2]).is_some() => {
                out.push(hex(b[i + 1]).unwrap_or(0) * 16 + hex(b[i + 2]).unwrap_or(0));
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

pub fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|c| {
            if c.is_ascii_alphanumeric() || b"-_.*".contains(&c) {
                (c as char).to_string()
            } else if c == b' ' {
                "+".into()
            } else {
                format!("%{c:02X}")
            }
        })
        .collect()
}

fn parse_head(head: &str) -> Option<(String, String, HashMap<String, String>)> {
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_string();
    let target = first.next()?.to_string();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    Some((method, target, headers))
}

fn split_target(target: &str) -> (String, HashMap<String, String>) {
    match target.split_once('?') {
        Some((p, q)) => (p.to_string(), parse_form(q)),
        None => (target.to_string(), HashMap::new()),
    }
}

pub fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        410 => "Gone",
        _ => "Unknown",
    }
}

pub fn response(code: u16, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut out = format!("HTTP/1.1 {code} {}\r\n", reason(code));
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str(&format!("content-length: {}\r\nconnection: close\r\n\r\n", body.len()));
    let mut out = out.into_bytes();
    out.extend_from_slice(body);
    out
}

pub fn chunked_head(code: u16, content_type: &str) -> Vec<u8> {
    format!("HTTP/1.1 {code} {}\r\ncontent-type: {content_type}\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n", reason(code)).into_bytes()
}

pub fn chunk(data: &[u8]) -> Vec<u8> {
    let mut out = format!("{:x}\r\n", data.len()).into_bytes();
    out.extend_from_slice(data);
    out.extend_from_slice(b"\r\n");
    out
}

// ---------- server side (async, for the daemon) ----------

pub mod server {
    use super::*;
    use tokio::io::{AsyncRead, AsyncReadExt};

    /// Reads one request; None if the client went away or sent garbage.
    pub async fn read_request<R: AsyncRead + Unpin>(r: &mut R, max_body: usize) -> Option<Request> {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        let head_end = loop {
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i;
            }
            if buf.len() > 64 * 1024 {
                return None;
            }
            let n = r.read(&mut tmp).await.ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&tmp[..n]);
        };
        let (method, target, headers) = parse_head(std::str::from_utf8(&buf[..head_end]).ok()?)?;
        let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
        if len > max_body {
            return None;
        }
        let mut body = buf[head_end + 4..].to_vec();
        while body.len() < len {
            let n = r.read(&mut tmp).await.ok()?;
            if n == 0 {
                return None;
            }
            body.extend_from_slice(&tmp[..n]);
        }
        body.truncate(len);
        let (path, query) = split_target(&target);
        Some(Request { method, path, query, headers, body })
    }
}

/// Blocking twin of `server::read_request`, for the CLI's one-shot local pages.
pub fn read_request_sync<R: Read>(r: &mut R, max_body: usize) -> Option<Request> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > 64 * 1024 {
            return None;
        }
        let n = r.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    };
    let (method, target, headers) = parse_head(std::str::from_utf8(&buf[..head_end]).ok()?)?;
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if len > max_body {
        return None;
    }
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = r.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(len);
    let (path, query) = split_target(&target);
    let mut req = Request { method, path, query, headers, body };
    // The local pages compare the full target, query included.
    if let Some((_, q)) = target.split_once('?') {
        req.path = format!("{}?{q}", req.path);
    }
    Some(req)
}

// ---------- client side (blocking, for the CLI) ----------

fn connect(port: u16, timeout: Option<Duration>) -> std::io::Result<TcpStream> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let s = TcpStream::connect_timeout(&addr, Duration::from_secs(5))?;
    s.set_read_timeout(timeout)?;
    s.set_nodelay(true)?;
    Ok(s)
}

fn send_request(mut s: &TcpStream, port: u16, method: &str, path: &str, token: &str, body: Option<&[u8]>) -> std::io::Result<()> {
    let mut req = format!("{method} {path} HTTP/1.1\r\nhost: 127.0.0.1:{port}\r\nx-denden-token: {token}\r\nconnection: close\r\n");
    if let Some(b) = body {
        req.push_str(&format!("content-type: application/json\r\ncontent-length: {}\r\n", b.len()));
    }
    req.push_str("\r\n");
    let mut data = req.into_bytes();
    if let Some(b) = body {
        data.extend_from_slice(b);
    }
    s.write_all(&data)
}

fn read_head<R: BufRead>(r: &mut R) -> std::io::Result<(u16, HashMap<String, String>)> {
    let mut status = String::new();
    r.read_line(&mut status)?;
    let code = status.split(' ').nth(1).and_then(|c| c.parse().ok()).ok_or_else(|| std::io::Error::other("bad HTTP status line"))?;
    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    Ok((code, headers))
}

/// Reads a chunked body piece by piece, calling `on_data` for each chunk; stops when it returns false.
fn read_chunked<R: BufRead>(r: &mut R, mut on_data: impl FnMut(&[u8]) -> bool) -> std::io::Result<()> {
    loop {
        let mut size = String::new();
        if r.read_line(&mut size)? == 0 {
            return Ok(());
        }
        let n = usize::from_str_radix(size.trim().split(';').next().unwrap_or("0"), 16).unwrap_or(0);
        if n == 0 {
            return Ok(());
        }
        let mut data = vec![0u8; n];
        r.read_exact(&mut data)?;
        let mut crlf = [0u8; 2];
        r.read_exact(&mut crlf)?;
        if !on_data(&data) {
            return Ok(());
        }
    }
}

/// One request to the daemon. The connected socket goes to `on_connect` first, so another thread
/// can shut it down to abandon the request (a cancelled MCP call).
pub fn request_with(
    port: u16,
    method: &str,
    path: &str,
    token: &str,
    body: Option<&[u8]>,
    timeout: Option<Duration>,
    on_connect: impl FnOnce(&Arc<TcpStream>),
) -> std::io::Result<(u16, Vec<u8>)> {
    let s = Arc::new(connect(port, timeout)?);
    on_connect(&s);
    send_request(&s, port, method, path, token, body)?;
    let mut r = BufReader::new(&*s);
    let (code, headers) = read_head(&mut r)?;
    let mut out = Vec::new();
    if headers.get("transfer-encoding").is_some_and(|v| v.contains("chunked")) {
        read_chunked(&mut r, |d| {
            out.extend_from_slice(d);
            true
        })?;
    } else if let Some(len) = headers.get("content-length").and_then(|v| v.parse::<usize>().ok()) {
        out.resize(len, 0);
        r.read_exact(&mut out)?;
    } else {
        r.read_to_end(&mut out)?;
    }
    Ok((code, out))
}

/// GET a streaming endpoint and hand over its body as it arrives, until the server closes it.
pub fn stream(port: u16, path: &str, token: &str, mut on_data: impl FnMut(&[u8])) -> std::io::Result<()> {
    let s = connect(port, None)?;
    send_request(&s, port, "GET", path, token, None)?;
    let mut r = BufReader::new(s);
    let (_code, headers) = read_head(&mut r)?;
    if headers.get("transfer-encoding").is_some_and(|v| v.contains("chunked")) {
        read_chunked(&mut r, |d| {
            on_data(d);
            true
        })
    } else {
        let mut buf = [0u8; 4096];
        loop {
            let n = r.read(&mut buf)?;
            if n == 0 {
                return Ok(());
            }
            on_data(&buf[..n]);
        }
    }
}
