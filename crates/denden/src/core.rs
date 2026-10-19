
// Shared pieces of denden: paths and private files, keys and fingerprints, channel secrets and what
// is derived from them, the daemon's local API, and the outgoing secret scanner.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The labels of every derivation from a channel secret (unchanged since agent-im: renaming them
/// would only risk a cryptographic slip).
pub const PROTO: &str = "agent-im/v2";
pub const MAX_TEXT: usize = 32 * 1024;

pub fn home() -> PathBuf {
    match std::env::var_os("DENDEN_HOME") {
        Some(h) if !h.is_empty() => PathBuf::from(h),
        _ => home_dir().join(".denden"),
    }
}

pub fn home_dir() -> PathBuf {
    home_dir_opt().unwrap_or_else(|| PathBuf::from("."))
}

fn home_dir_opt() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// A current directory that holds no project or plugin folder: the user's home, else the root of
/// the current drive. On Windows a process keeps its current directory locked (it cannot be renamed
/// or deleted), so long-lived processes (`im mcp`, the daemon) move there.
pub fn neutral_dir() -> PathBuf {
    if let Some(h) = home_dir_opt().filter(|h| h.is_dir()) {
        return h;
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    cwd.ancestors().last().filter(|r| !r.as_os_str().is_empty()).map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(std::path::MAIN_SEPARATOR_STR))
}

/// An DENDEN_HOME given as a relative path is relative to the directory we were started in: pin it to
/// an absolute path before anything changes directory, for us and for the processes we start.
pub fn pin_im_home() {
    let Some(h) = std::env::var_os("DENDEN_HOME").filter(|h| !h.is_empty()) else { return };
    let p = PathBuf::from(&h);
    if p.is_absolute() {
        return;
    }
    if let Ok(abs) = std::path::absolute(&p) {
        // SAFETY: called first thing in main, before any other thread exists.
        unsafe { std::env::set_var("DENDEN_HOME", abs) };
    }
}

pub fn channels_dir() -> PathBuf {
    home().join("channels")
}
/// Conversation -> channel, denden, mode and wake-up budget.
pub fn conversations_file() -> PathBuf {
    home().join("conversations.json")
}
/// Machine-wide settings (keepalive).
pub fn settings_file() -> PathBuf {
    home().join("settings.json")
}

/// This machine's name as other members see it: its host name, lowercased and cleaned to a valid
/// name (`LAPTOP-AK` gives `laptop-ak`).
pub fn machine_name() -> String {
    clean_name(&host_name()).unwrap_or_else(|| "machine".into())
}

/// `raw` turned into a valid name, or None when nothing usable is left.
pub fn clean_name(raw: &str) -> Option<String> {
    let mut out = String::new();
    for c in raw.trim().to_lowercase().chars() {
        let c = if c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c) { c } else { '-' };
        if !(c == '-' && out.ends_with('-')) {
            out.push(c);
        }
    }
    let out: String = out.trim_start_matches(['-', '.', '_']).chars().take(32).collect();
    let out = out.trim_end_matches(['-', '.', '_']).to_string();
    valid_name(&out).then_some(out)
}

fn host_name() -> String {
    for var in ["COMPUTERNAME", "HOSTNAME"] {
        if let Some(h) = std::env::var_os(var).filter(|h| !h.is_empty()) {
            return h.to_string_lossy().to_string();
        }
    }
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        if unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) } == 0 {
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            return String::from_utf8_lossy(&buf[..end]).split('.').next().unwrap_or("").to_string();
        }
    }
    String::new()
}

/// path.resolve() of a folder, lowercased on Windows.
pub fn folder_key(p: &Path) -> String {
    let abs = if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(p) };
    let mut parts: Vec<String> = Vec::new();
    let mut prefix = String::new();
    for c in abs.components() {
        match c {
            std::path::Component::Prefix(pr) => prefix = pr.as_os_str().to_string_lossy().to_string(),
            std::path::Component::RootDir => {}
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                parts.pop();
            }
            std::path::Component::Normal(n) => parts.push(n.to_string_lossy().to_string()),
        }
    }
    let sep = if cfg!(windows) { "\\" } else { "/" };
    let s = format!("{prefix}{sep}{}", parts.join(sep));
    if cfg!(windows) { s.to_lowercase() } else { s }
}

/// Discovery topics and transport keys change every epoch (one UTC day). DENDEN_EPOCH_SECONDS is for tests.
pub fn epoch_ms() -> u64 {
    static EPOCH: OnceLock<u64> = OnceLock::new();
    *EPOCH.get_or_init(|| {
        let secs = std::env::var("DENDEN_EPOCH_SECONDS")
            .ok()
            .and_then(|s| s.trim().parse::<f64>().ok())
            .filter(|s| *s != 0.0 && s.is_finite())
            .unwrap_or(86400.0);
        (secs.max(30.0) * 1000.0) as u64
    })
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_millis() as u64
}

pub fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

// ---------- names ----------

/// /^[a-z0-9][a-z0-9._-]{0,31}$/
pub fn valid_name(n: &str) -> bool {
    let b = n.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(c))
}

pub fn is_broadcast(n: &str) -> bool {
    matches!(n, "all" | "*" | "everyone")
}

pub fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

// ---------- bytes ----------

/// Length-prefixed concatenation, so ("ab", "c") and ("a", "bc") never sign or MAC the same bytes.
pub fn frame(parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for p in parts {
        out.extend_from_slice(&(p.len() as u32).to_be_bytes());
        out.extend_from_slice(p);
    }
    out
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

pub fn hmac(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut m = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("any key length");
    m.update(data);
    m.finalize().into_bytes().into()
}

pub fn b64u(b: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(b)
}

/// Lenient base64url decoding: either alphabet, padding and junk ignored.
pub fn unb64u(s: &str) -> Vec<u8> {
    let mut clean: String = s
        .chars()
        .filter_map(|c| match c {
            '+' => Some('-'),
            '/' => Some('_'),
            c if c.is_ascii_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect();
    if clean.len() % 4 == 1 {
        clean.pop();
    }
    let engine = base64::engine::GeneralPurpose::new(
        &base64::alphabet::URL_SAFE,
        base64::engine::GeneralPurposeConfig::new()
            .with_encode_padding(false)
            .with_decode_allow_trailing_bits(true)
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent),
    );
    engine.decode(clean).unwrap_or_default()
}

pub fn random_hex(n: usize) -> String {
    let mut b = vec![0u8; n];
    rand::fill(&mut b[..]);
    hex::encode(b)
}

pub fn safe_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// JavaScript's String(value), for fields peers may send with the wrong type.
pub fn js_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Array(a) => a.iter().map(|x| if x.is_null() { String::new() } else { js_string(x) }).collect::<Vec<_>>().join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}

/// JavaScript truthiness of a JSON value (undefined is None).
pub fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

/// Text length the way JavaScript counts it (UTF-16 code units), for the same MAX_TEXT limit.
pub fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}

// ---------- private files (0700 dirs, 0600 files; on Windows the profile ACLs already apply) ----------

pub fn ensure_dir(dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

pub fn read_json(file: &Path) -> Option<Value> {
    let text = fs::read_to_string(file).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_private(file: &Path, data: &[u8]) -> std::io::Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(file)?.write_all(data)
}

pub fn write_json(file: &Path, value: &Value) -> std::io::Result<()> {
    let data = serde_json::to_string_pretty(value).unwrap_or_default();
    let tmp = PathBuf::from(format!("{}.{}.tmp", file.display(), std::process::id()));
    match write_private(&tmp, data.as_bytes()).and_then(|_| fs::rename(&tmp, file)) {
        Ok(()) => Ok(()),
        Err(_) => {
            let _ = fs::remove_file(&tmp);
            write_private(file, data.as_bytes())
        }
    }
}

pub fn append_private(file: &Path, data: &str) -> std::io::Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(file)?.write_all(data.as_bytes())
}

// ---------- processes ----------

#[cfg(windows)]
pub fn is_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, GetLastError, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    if pid == 0 {
        return false;
    }
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return GetLastError() == ERROR_ACCESS_DENIED;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code);
        CloseHandle(h);
        ok != 0 && code == STILL_ACTIVE as u32
    }
}

#[cfg(unix)]
pub fn is_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let alive = unsafe { libc::kill(pid as libc::pid_t, 0) } == 0;
    alive || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

// ---------- the daemon's localhost API (port and random token in daemon.json) ----------

pub struct DaemonInfo {
    pub pid: u32,
    pub port: u16,
    pub token: String,
}

pub fn daemon_info(dir: &Path) -> Option<DaemonInfo> {
    let v = read_json(&dir.join("daemon.json"))?;
    Some(DaemonInfo {
        pid: v.get("pid")?.as_u64()? as u32,
        port: v.get("port")?.as_u64()? as u16,
        token: v.get("token")?.as_str()?.to_string(),
    })
}

/// One request to the daemon of the DENDEN_HOME `dir`.
pub fn api_request(dir: &Path, method: &str, path: &str, body: Option<&Value>, timeout_ms: u64) -> Result<Value, String> {
    api_request_with(dir, method, path, body, timeout_ms, |_| {})
}

/// `api_request`, handing the connected socket to `on_connect` (see `http::request_with`).
pub fn api_request_with(
    dir: &Path,
    method: &str,
    path: &str,
    body: Option<&Value>,
    timeout_ms: u64,
    on_connect: impl FnOnce(&std::sync::Arc<std::net::TcpStream>),
) -> Result<Value, String> {
    let info = daemon_info(dir).ok_or_else(|| "daemon not running".to_string())?;
    let data = body.map(|b| serde_json::to_vec(b).unwrap_or_default());
    let timeout = (timeout_ms > 0).then(|| Duration::from_millis(timeout_ms));
    let (code, buf) = crate::http::request_with(info.port, method, path, &info.token, data.as_deref(), timeout, on_connect).map_err(|e| {
        if e.kind() == std::io::ErrorKind::TimedOut || e.kind() == std::io::ErrorKind::WouldBlock {
            "the daemon did not answer in time".to_string()
        } else {
            e.to_string()
        }
    })?;
    let text = String::from_utf8_lossy(&buf).to_string();
    let json: Value = serde_json::from_str(&text)
        .map_err(|_| format!("bad daemon response: {}", text.chars().take(200).collect::<String>()))?;
    if code >= 400 {
        return Err(json.get("error").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("HTTP {code}")));
    }
    Ok(json)
}

pub fn daemon_up(dir: &Path) -> bool {
    match daemon_info(dir) {
        Some(info) if is_alive(info.pid) => api_request(dir, "GET", "/status", None, 3000).is_ok(),
        _ => false,
    }
}

// ---------- identity keys: Ed25519 from the 32-byte seed kept in config.json ----------

pub struct Identity {
    pub sk: SigningKey,
    pub pub_key: [u8; 32],
}

pub fn identity_keys(seed_hex: &str) -> Option<Identity> {
    let seed: [u8; 32] = hex::decode(seed_hex).ok()?.try_into().ok()?;
    let sk = SigningKey::from_bytes(&seed);
    let pub_key = sk.verifying_key().to_bytes();
    Some(Identity { sk, pub_key })
}

pub fn sign(sk: &SigningKey, data: &[u8]) -> [u8; 64] {
    sk.sign(data).to_bytes()
}

pub fn verify_sig(pub_key: &[u8], sig: &[u8], data: &[u8]) -> bool {
    let (Ok(pk), Ok(sig)) = (<[u8; 32]>::try_from(pub_key), Signature::from_slice(sig)) else {
        return false;
    };
    VerifyingKey::from_bytes(&pk).is_ok_and(|vk| vk.verify(data, &sig).is_ok())
}

/// Fingerprints: 16 hex chars (64 bits) to verify a key by eye, the first 8 in compact listings.
pub fn fingerprint(pub_key: &[u8]) -> String {
    hex::encode(sha256(pub_key))[..16].to_string()
}
pub fn fingerprint_hex(key_hex: &str) -> String {
    fingerprint(&hex::decode(key_hex).unwrap_or_default())
}
pub fn fmt_fp(fp: &str) -> String {
    fp.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).to_string()).collect::<Vec<_>>().join("-")
}
pub fn short_fp(pub_key: &[u8]) -> String {
    fingerprint(pub_key)[..8].to_string()
}
pub fn norm_fp(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_ascii_hexdigit()).collect()
}

// ---------- channels: a random 256-bit secret; everything public is derived from it ----------

pub fn new_channel_secret() -> String {
    random_hex(32)
}

pub struct ChannelKeys {
    pub id: [u8; 32],
    pub auth: [u8; 32],
    secret: Vec<u8>,
}

impl ChannelKeys {
    pub fn new(secret_hex: &str) -> Self {
        let s = hex::decode(secret_hex).unwrap_or_default();
        Self {
            id: hmac(&s, format!("{PROTO}/id").as_bytes()),
            auth: hmac(&s, format!("{PROTO}/auth").as_bytes()),
            secret: s,
        }
    }
    /// What the DHT sees; changes every epoch.
    pub fn topic(&self, epoch: u64) -> [u8; 32] {
        hmac(&self.secret, format!("{PROTO}/topic/{epoch}").as_bytes())
    }
}

pub fn default_label(secret_hex: &str) -> String {
    let s = hex::decode(secret_hex).unwrap_or_default();
    format!("chan-{}", &hex::encode(hmac(&s, format!("{PROTO}/label").as_bytes()))[..4])
}

/// Per-epoch transport key seed: the DHT never sees the long-term identity key.
pub fn transport_seed(seed_hex: &str, epoch: u64) -> [u8; 32] {
    hmac(&hex::decode(seed_hex).unwrap_or_default(), format!("{PROTO}/transport/{epoch}").as_bytes())
}

// ---------- outgoing secret scanner ----------

pub const INVITE_RE: &str = r"denden_[A-Za-z0-9_-]{40,}";

fn secret_patterns() -> &'static Vec<(&'static str, regex_lite::Regex)> {
    static P: OnceLock<Vec<(&'static str, regex_lite::Regex)>> = OnceLock::new();
    P.get_or_init(|| {
        [
            ("an Anthropic API key", r"sk-ant-[A-Za-z0-9_-]{20,}"),
            ("an OpenAI API key", r"\bsk-(?:proj-|svcacct-)?[A-Za-z0-9_-]{32,}"),
            ("an AWS access key", r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b"),
            ("a GitHub token", r"\b(?:gh[pousr]_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{40,})"),
            ("a Slack token", r"\bxox[abprs]-[A-Za-z0-9-]{10,}"),
            ("a Google API key", r"\bAIza[0-9A-Za-z_-]{35}\b"),
            ("a Stripe live key", r"\b[sr]k_live_[0-9A-Za-z]{20,}"),
            ("a private key", r"-----BEGIN [A-Z ]*PRIVATE KEY-----"),
            ("a JSON Web Token", r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}"),
            ("credentials inside a URL", r"(?i)\b[a-z][a-z0-9+.-]*://[^\s:/@]+:[^\s@/]+@"),
            (
                "a password or secret value",
                r#"(?i)\b(?:password|passwd|pwd|secret|api[_-]?key|access[_-]?key|auth[_-]?token|client[_-]?secret)\b["']?\s*[:=]\s*(?:["'][^"'\s]{8,}["']|[A-Za-z0-9_\-+/=]{16,})"#,
            ),
            ("a denden invitation code", INVITE_RE),
        ]
        .into_iter()
        .map(|(what, re)| (what, regex_lite::Regex::new(re).expect("valid pattern")))
        .collect()
    })
}

pub fn find_secrets(text: &str) -> Vec<&'static str> {
    secret_patterns().iter().filter(|(_, re)| re.is_match(text)).map(|(what, _)| *what).collect()
}

pub fn has_invite(text: &str) -> bool {
    static RE: OnceLock<regex_lite::Regex> = OnceLock::new();
    RE.get_or_init(|| regex_lite::Regex::new(INVITE_RE).expect("valid")).is_match(text)
}
