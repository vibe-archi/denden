// Codex: which conversation is which (thread ids, rollouts, titles), finding the codex binary, and
// running `codex queue --thread <id> --message <text>`, which wakes an idle conversation: the
// notice shows up as a user message and Codex starts a turn by itself. The notice never carries
// message text. When to wake is wake.rs's business; the daemon runs the command.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
#[cfg(test)]
use serde_json::json;

use crate::core::home_dir;

/// Thread ids go into argv as the value of --thread: a session UUID, never anything that could read
/// as an option.
pub fn valid_thread(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty() && b.len() <= 128 && b[0].is_ascii_alphanumeric() && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-' || *c == b'_')
}

// ---------- which conversation ----------
//
// Codex runs hooks in every thread, including the ones it spawns for itself: guardian reviews of
// escalated commands, subagents, ephemeral threads. Only a user conversation holds a denden and is
// woken. Codex gives the id of the conversation running a command to that command as
// CODEX_THREAD_ID; a spawned thread stands for the conversation it belongs to (its rollout's
// parent_thread_id), and a thread without a user rollout stands for nothing.

/// CODEX_HOME, else ~/.codex.
pub fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home_dir().join(".codex"))
}

/// The Codex conversation running this command (Codex sets CODEX_THREAD_ID in the agent's shell
/// commands, in or out of its sandbox).
pub fn thread_from_env() -> Option<String> {
    std::env::var("CODEX_THREAD_ID").ok().map(|t| t.trim().to_string()).filter(|t| valid_thread(t))
}

/// `sessions/**/rollout-<time>-<id>.jsonl` under a Codex home.
fn rollout(home: &Path, id: &str) -> Option<PathBuf> {
    let suffix = format!("-{id}.jsonl");
    let mut dirs = vec![(home.join("sessions"), 0)];
    while let Some((dir, depth)) = dirs.pop() {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                if depth < 6 {
                    dirs.push((p, depth + 1));
                }
            } else if name.starts_with("rollout-") && name.ends_with(&suffix) {
                return Some(p);
            }
        }
    }
    None
}

#[derive(Debug, PartialEq)]
pub enum Kind {
    /// A conversation of the user's.
    User,
    /// A thread Codex spawned (guardian review, subagent), with the thread it belongs to.
    Sub(String),
    /// No rollout (an ephemeral thread), or not a user conversation.
    Unknown,
}

/// What a thread is, from the first line (session_meta) of its rollout.
pub fn classify(home: &Path, id: &str) -> Kind {
    if !valid_thread(id) {
        return Kind::Unknown;
    }
    let Some(file) = rollout(home, id) else { return Kind::Unknown };
    let first = std::fs::File::open(&file).ok().and_then(|f| {
        use std::io::BufRead;
        let mut line = String::new();
        std::io::BufReader::new(f).read_line(&mut line).ok()?;
        serde_json::from_str::<Value>(&line).ok()
    });
    let Some(first) = first else { return Kind::Unknown };
    let meta = first.get("payload").filter(|p| p.is_object()).unwrap_or(&first);
    let field = |k: &str| meta.get(k).and_then(Value::as_str).filter(|s| !s.is_empty());
    match (field("parent_thread_id"), field("thread_source")) {
        (Some(parent), _) => Kind::Sub(parent.to_string()),
        (None, Some("user")) => Kind::User,
        _ => Kind::Unknown,
    }
}

/// The user conversation a thread belongs to: itself, or the parent of a spawned thread.
pub fn user_thread(home: &Path, id: &str) -> Option<String> {
    let mut id = id.to_string();
    for _ in 0..5 {
        match classify(home, &id) {
            Kind::User => return Some(id),
            Kind::Sub(parent) => id = parent,
            Kind::Unknown => return None,
        }
    }
    None
}

/// The conversation's title in Codex (session_index.jsonl, its latest entry).
pub fn title(home: &Path, id: &str) -> Option<String> {
    let text = std::fs::read_to_string(home.join("session_index.jsonl")).ok()?;
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v.get("id").and_then(Value::as_str) == Some(id))
        .filter_map(|v| v.get("thread_name").and_then(Value::as_str).map(str::to_string))
        .last()
}

/// Codex's error when the thread does not exist for it (no rollout, unknown thread).
pub fn thread_gone(error: &str) -> bool {
    let e = error.to_lowercase();
    e.contains("no rollout found") || e.contains("thread not found")
}

// ---------- the codex binary ----------

/// How to run codex: the program and the arguments that come before ours (a script for node).
#[derive(Clone, Debug, PartialEq)]
pub struct CodexBin {
    pub argv: Vec<String>,
    pub how: &'static str,
}

fn parse_version(name: &str) -> [u64; 3] {
    let core = name.split(['-', '+']).next().unwrap_or("");
    let mut out = [0u64; 3];
    for (slot, part) in out.iter_mut().zip(core.split('.')) {
        *slot = part.trim().parse().unwrap_or(0);
    }
    out
}

fn mtime(p: &Path) -> std::time::SystemTime {
    std::fs::metadata(p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH)
}

fn exe_name() -> &'static str {
    if cfg!(windows) { "codex.exe" } else { "codex" }
}

/// The newest `releases/<version>-<target>/bin/codex[.exe]` of the desktop app's app-server daemon.
fn from_app_server() -> Option<PathBuf> {
    let home = std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home_dir().join(".codex"));
    let releases = home.join("packages").join("app-server-daemon").join("releases");
    std::fs::read_dir(releases)
        .ok()?
        .flatten()
        .map(|e| e.path().join("bin").join(exe_name()))
        .filter(|p| p.is_file())
        .max_by_key(|p| {
            let dir = p.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default();
            (parse_version(&dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()), mtime(&dir))
        })
}

/// The native codex.exe behind npm's `codex.cmd` shim, or node with the package's script: either
/// runs with plain argv, where the .cmd would go through cmd.exe and its parsing of our arguments.
#[cfg(windows)]
fn from_npm_shim(dir: &Path) -> Option<Vec<String>> {
    let (arch, triple) = if cfg!(target_arch = "aarch64") { ("arm64", "aarch64-pc-windows-msvc") } else { ("x64", "x86_64-pc-windows-msvc") };
    let pkg = dir.join("node_modules").join("@openai").join("codex");
    let platform = format!("codex-win32-{arch}");
    let vendors = [
        pkg.join("node_modules").join("@openai").join(&platform).join("vendor"),
        dir.join("node_modules").join("@openai").join(&platform).join("vendor"),
        pkg.join("vendor"),
    ];
    for v in &vendors {
        for sub in ["bin", "codex"] {
            let exe = v.join(triple).join(sub).join("codex.exe");
            if exe.is_file() {
                return Some(vec![exe.display().to_string()]);
            }
        }
    }
    let script = pkg.join("bin").join("codex.js");
    if !script.is_file() {
        return None;
    }
    let node = Some(dir.join("node.exe")).filter(|n| n.is_file()).or_else(|| on_path("node.exe"))?;
    Some(vec![node.display().to_string(), script.display().to_string()])
}

#[cfg(windows)]
fn on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(name)).find(|p| p.is_file())
}

fn from_path() -> Option<Vec<String>> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        #[cfg(windows)]
        {
            let exe = dir.join("codex.exe");
            if exe.is_file() {
                return Some(vec![exe.display().to_string()]);
            }
            if ["codex.cmd", "codex.ps1", "codex"].iter().any(|n| dir.join(n).is_file()) {
                if let Some(argv) = from_npm_shim(&dir) {
                    return Some(argv);
                }
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let p = dir.join("codex");
            if std::fs::metadata(&p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0) {
                return Some(vec![p.display().to_string()]);
            }
        }
    }
    None
}

/// The desktop app's own copies, `%LOCALAPPDATA%\OpenAI\Codex\bin\<hash>\codex.exe`: the newest.
fn from_desktop() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let base = PathBuf::from(std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty())?).join("OpenAI").join("Codex").join("bin");
    std::fs::read_dir(base).ok()?.flatten().map(|e| e.path().join("codex.exe")).filter(|p| p.is_file()).max_by_key(|p| mtime(p))
}

/// Finds codex: DENDEN_CODEX_BIN, the desktop app-server's newest release, `codex` on PATH, the desktop
/// app's own binaries.
pub fn find_codex() -> Option<CodexBin> {
    if let Some(p) = std::env::var_os("DENDEN_CODEX_BIN").filter(|v| !v.is_empty()) {
        return Some(CodexBin { argv: vec![PathBuf::from(p).display().to_string()], how: "DENDEN_CODEX_BIN" });
    }
    if let Some(p) = from_app_server() {
        return Some(CodexBin { argv: vec![p.display().to_string()], how: "Codex app-server" });
    }
    if let Some(argv) = from_path() {
        return Some(CodexBin { argv, how: "PATH" });
    }
    from_desktop().map(|p| CodexBin { argv: vec![p.display().to_string()], how: "Codex desktop app" })
}

#[cfg(windows)]
fn no_window(c: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
}
#[cfg(not(windows))]
fn no_window(_: &mut std::process::Command) {}

/// Runs codex with `args` after its argv prefix: no shell, stdin closed, killed after `timeout`.
/// Ok on exit status 0; otherwise the reason (exit code and the first line of its output).
pub fn run(argv: &[String], args: &[&str], timeout: Duration) -> Result<(), String> {
    let Some((program, prefix)) = argv.split_first() else { return Err("no codex binary".into()) };
    let mut c = std::process::Command::new(program);
    c.args(prefix)
        .args(args)
        .current_dir(crate::core::neutral_dir())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    no_window(&mut c);
    let mut child = c.spawn().map_err(|e| format!("cannot run {program}: {e}"))?;
    // Read both pipes on threads, so a chatty codex cannot fill one and stall.
    let pipe = |r: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut s = String::new();
            if let Some(mut r) = r {
                let _ = r.read_to_string(&mut s);
            }
            s
        })
    };
    let out = pipe(child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>));
    let err = pipe(child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>));
    let t0 = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if t0.elapsed() < timeout => std::thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("no answer within {} s", timeout.as_secs()));
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    let text = format!("{}\n{}", err.join().unwrap_or_default(), out.join().unwrap_or_default());
    if status.success() {
        return Ok(());
    }
    let first = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(200).collect::<String>();
    Err(format!("exit {}{}", status.code().map(|c| c.to_string()).unwrap_or_else(|| "by signal".into()), if first.is_empty() { String::new() } else { format!(": {first}") }))
}

/// The fixed text queued as a user message: a count and validated names only, never message text.
/// The skill tells the agent that a line starting with 🐌» is denden's, not the user's.
pub fn notice(n: usize, senders: &[String], late: usize) -> String {
    let mut names: Vec<String> = senders.iter().take(5).cloned().collect();
    if senders.len() > 5 {
        names.push(format!("{} more", senders.len() - 5));
    }
    let what = if n == 1 { "1 new message".to_string() } else { format!("{n} new messages") };
    let late = if late > 0 { format!(" · {late} late") } else { String::new() };
    format!("> 🐌» (( {what} from {}{late} · denden inbox ))", names.join(", "))
}

pub fn argv_of(v: &Value) -> Option<Vec<String>> {
    let a: Vec<String> = v.as_array()?.iter().filter_map(|s| s.as_str().map(str::to_string)).collect();
    // The program must be an absolute path: it comes from a hook, through the local API.
    (!a.is_empty() && a.len() <= 3 && Path::new(&a[0]).is_absolute()).then_some(a)
}

/// The `codex` field of a hook request, checked like a saved one.
pub fn argv_from_request(v: Option<&Value>) -> Option<Vec<String>> {
    v.and_then(argv_of)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "019e1c2a-1111-7222-8333-444455556666";

    #[test]
    fn thread_ids() {
        assert!(valid_thread(T));
        assert!(!valid_thread("--message"));
        assert!(!valid_thread(""));
        assert!(!valid_thread("a b"));
        assert!(!valid_thread(&"a".repeat(129)));
    }

    #[test]
    fn rollouts_classify_threads() {
        let home = std::env::temp_dir().join(format!("denden-codex-test-{}", std::process::id()));
        let day = home.join("sessions").join("2026").join("10").join("17");
        std::fs::create_dir_all(&day).unwrap();
        let write = |id: &str, meta: Value| {
            std::fs::write(day.join(format!("rollout-2026-10-17T00-00-00-{id}.jsonl")), format!("{}\n", json!({ "type": "session_meta", "payload": meta }))).unwrap();
        };
        let user = "01a1496e-0000-7000-8000-000000000001";
        let guardian = "01a1496f-0000-7000-8000-000000000002";
        write(user, json!({ "id": user, "thread_source": "user", "source": "vscode" }));
        write(guardian, json!({ "id": guardian, "parent_thread_id": user, "thread_source": "guardian_review", "source": { "subagent": { "other": "guardian" } } }));
        assert_eq!(classify(&home, user), Kind::User);
        assert_eq!(classify(&home, guardian), Kind::Sub(user.to_string()));
        assert_eq!(classify(&home, "01a14975-0000-7000-8000-000000000003"), Kind::Unknown, "ephemeral: no rollout");
        assert_eq!(user_thread(&home, guardian).as_deref(), Some(user));
        std::fs::write(home.join("session_index.jsonl"), format!("{}\n{}\n", json!({ "id": user, "thread_name": "old" }), json!({ "id": user, "thread_name": "Queue test" }))).unwrap();
        assert_eq!(title(&home, user).as_deref(), Some("Queue test"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn notice_has_no_text() {
        assert_eq!(notice(2, &["coord".into(), "back".into()], 0), "> 🐌» (( 2 new messages from coord, back · denden inbox ))");
        assert_eq!(notice(1, &["coord".into()], 1), "> 🐌» (( 1 new message from coord · 1 late · denden inbox ))");
    }
}
