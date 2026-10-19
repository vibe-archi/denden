// denden: peer-to-peer messaging between AI conversations, on the Hyperswarm DHT (via peeroxide).
//
// One background daemon per machine keeps the channels' connections; this binary is also the CLI
// the conversations call (through the skill /denden:im), the hooks of Claude Code and Codex, and an
// MCP server for Codex's hooks. Channel secrets and invitation codes never appear in its output:
// they are shown or entered on a one-shot local page, or in the user's own terminal.

mod binary;
mod check;
mod codex;
mod conv;
mod core;
mod daemon;
mod http;
mod mcp;
mod net;
mod orphan;
mod output;
mod proto;
mod store;
mod time;
mod ui;
mod wake;

use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::core::*;
use crate::output::{alert, info, question};

fn help() -> String {
    format!(
        "denden - messages between AI conversations, peer to peer (Hyperswarm, no server)

Usage, from a conversation (the skill passes --session for Claude; Codex sets CODEX_THREAD_ID):
  denden                              this conversation's denden, channel, mode and unread count
  denden create <channel> [as <name>] create a channel on this machine and attach this conversation
  denden join <channel> [as <name>]   attach this conversation to a channel (--move: take the name
                                      from another conversation here; asks for the invitation code
                                      on a local page when this machine is not in the channel yet)
  denden invite                       a single-use invitation code for a new machine, on a local page
  denden open <url>                   open a denden local page in the system browser
  denden who                          the denden of the channel: machine, role, state
  denden send <to> <text>             send without waiting for a reply (<to>: name, a,b,c, or all)
  denden ask <to> <text>              send and ask for a reply
  denden history [<name>] [-n <N>]    this denden's messages
  denden role \"<text>\"                the role shown in who
  denden auto on|off                  handle requests without waiting for the user's go-ahead
  denden wakes <n>|off                wake-ups per hour for this conversation
  denden keepalive on|off             keep this machine's daemon up without attached conversations
  denden detach                       detach this conversation (the denden stays, its messages wait)
  denden delete <name>                delete a denden of this machine (the name stays bound to it)
  denden block|unblock <machine>      ignore a machine, here only
  denden leave <channel>              this machine leaves the channel
  denden check                        a full report of the installation and its protections

In your own terminal, outside any AI: denden join <channel> (the code is typed hidden).
Internal (agents and hooks): inbox, reply <id> <text>, watch, hook-prompt, hook-stop, mcp, stop.
Data: {}",
        home().display()
    )
}

// ---------- small helpers ----------

/// Exit codes: 1 an error, 3 a decision the user has to make (the question is printed).
const NEEDS_ANSWER: i32 = 3;

fn fail(msg: &str) -> ! {
    println!("{}", alert(msg));
    std::process::exit(1)
}

fn ask_user(q: &str) -> ! {
    println!("{}", question(q));
    std::process::exit(NEEDS_ANSWER)
}

type Opts = HashMap<String, String>;

fn flag(opts: &Opts, k: &str) -> bool {
    opts.contains_key(k)
}

fn vs<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}
fn vu(v: &Value, k: &str) -> u64 {
    v.get(k).and_then(Value::as_u64).unwrap_or(0)
}
fn arr(v: &Value, k: &str) -> Vec<Value> {
    v.get(k).and_then(Value::as_array).cloned().unwrap_or_default()
}

/// Writes a line to stdout and flushes it. When that fails, whoever read us is gone (a closed
/// pipe): exit quietly, before anything is marked read.
fn emit(text: &str) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    if writeln!(out, "{text}").and_then(|_| out.flush()).is_err() {
        std::process::exit(0);
    }
}

/// `<channel> as <name>` (or `--as <name>`): the channel and the name, if given.
fn channel_and_name(pos: &[String], opts: &Opts) -> (String, String) {
    let label = pos.first().map(|l| l.to_lowercase()).unwrap_or_default();
    let name = match (pos.get(1).map(String::as_str), pos.get(2)) {
        (Some("as"), Some(n)) => n.to_lowercase(),
        (Some(n), None) if n != "as" => n.to_lowercase(),
        _ => opts.get("as").map(|n| n.to_lowercase()).unwrap_or_default(),
    };
    (label, name.trim_start_matches('@').to_string())
}

// ---------- which conversation ----------

struct Here {
    conv: Option<String>,
    folder: String,
    cwd: String,
    title: String,
}

fn here(opts: &Opts) -> Here {
    let cwd_p = std::env::current_dir().unwrap_or_default();
    let (folder, cwd) = (folder_key(&cwd_p), cwd_p.display().to_string());
    let mut title = String::new();
    let conv = if let Some(s) = opts.get("session").filter(|s| !s.is_empty() && !s.contains("CLAUDE_SESSION_ID")) {
        Some(format!("claude:{s}"))
    } else if let Some(c) = std::env::var("DENDEN_CONV").ok().filter(|c| conv::valid_key(c)) {
        Some(c)
    } else if let Some(t) = codex::thread_from_env() {
        // A thread Codex spawned (a guardian review) stands for the conversation it belongs to.
        let home_c = codex::codex_home();
        let id = codex::user_thread(&home_c, &t).unwrap_or(t);
        title = codex::title(&home_c, &id).unwrap_or_default();
        Some(format!("codex:{id}"))
    } else {
        None
    };
    Here { conv: conv.filter(|c| conv::valid_key(c)), folder, cwd, title }
}

fn need_conv(h: &Here) -> String {
    match &h.conv {
        Some(c) => c.clone(),
        None => fail("run this from a Claude or Codex conversation (Claude: the skill passes --session)"),
    }
}

/// The fields every attach-like request carries.
fn conv_fields(h: &Here) -> Value {
    let mut v = json!({ "conv": h.conv, "folder": h.folder, "cwd": h.cwd, "title": h.title });
    if h.conv.as_deref().is_some_and(|c| c.starts_with("codex:")) {
        v["codex"] = json!(codex::find_codex().map(|c| c.argv));
        v["codexHome"] = json!(codex::codex_home().display().to_string());
    }
    v
}

// ---------- daemon management ----------

/// Windows: keep our standard handles out of the processes we start (they are inheritable when they
/// came from the parent, and CreateProcess passes every inheritable handle on).
fn no_inherit_std_handles() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation};
        use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
        for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let h = GetStdHandle(which);
            if !h.is_null() && h != INVALID_HANDLE_VALUE {
                SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

#[cfg(windows)]
fn hide_window(cmd: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000) // CREATE_NO_WINDOW
}
#[cfg(not(windows))]
fn hide_window(cmd: &mut Command) -> &mut Command {
    cmd
}

/// Runs in a short-lived middle process. Once it exits the daemon has no living parent, so killing the
/// caller's process tree (a Claude Code monitor expiring on `watch`, say) leaves the daemon running.
fn launch_daemon() -> ! {
    let log = std::fs::OpenOptions::new().create(true).append(true).open(home().join("daemon.log"));
    let Ok(log) = log else { std::process::exit(1) };
    // We run from the verified copy under DENDEN_HOME/bin (binary.rs), and the daemon is that same file.
    // On Linux through /proc/self/exe, which still works if the copy is removed meanwhile.
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("denden"));
    let make = || {
        #[cfg(target_os = "linux")]
        let mut c = {
            use std::os::unix::process::CommandExt;
            let mut c = Command::new("/proc/self/exe");
            c.arg0(&exe);
            c
        };
        #[cfg(not(target_os = "linux"))]
        let mut c = Command::new(&exe);
        // The daemon outlives whoever started it: it must not hold that folder (a project, the
        // plugin) as its current directory, which Windows would keep locked.
        c.arg("__daemon").stdin(Stdio::null()).current_dir(neutral_dir());
        if let (Ok(o), Ok(e)) = (log.try_clone(), log.try_clone()) {
            c.stdout(o).stderr(e);
        }
        c
    };
    no_inherit_std_handles();
    #[cfg(windows)]
    let child = {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        make()
            .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB)
            .spawn()
            .or_else(|_| make().creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP).spawn())
    };
    #[cfg(unix)]
    let child = {
        use std::os::unix::process::CommandExt;
        let mut c = make();
        unsafe {
            c.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        c.spawn()
    };
    match child {
        Ok(c) => {
            print!("{}", c.id());
            std::process::exit(0)
        }
        Err(_) => std::process::exit(1),
    }
}

fn daemon_log(line: &str) {
    let _ = append_private(&home().join("daemon.log"), &format!("{} {line}\n", time::iso(now_ms())));
}

fn restart_marker() -> PathBuf {
    home().join("restart.json")
}

/// Whether another command is restarting the daemon right now.
fn restart_pending() -> bool {
    read_json(&restart_marker()).is_some_and(|m| {
        let pid = vu(&m, "pid") as u32;
        pid != std::process::id() && is_alive(pid) && now_ms().saturating_sub(vu(&m, "t")) < 60_000
    })
}

fn wait_for_restart() -> bool {
    if !restart_pending() {
        return false;
    }
    let t0 = Instant::now();
    while restart_pending() && t0.elapsed() < Duration::from_secs(60) {
        sleep_ms(150);
    }
    daemon_up(&home())
}

/// Starts the machine's daemon unless one answers, restarting it first when it comes from an older
/// build (see binary.rs).
pub(crate) fn try_ensure_daemon() -> Result<(), String> {
    let dir = home();
    if daemon_up(&dir) {
        return match read_json(&dir.join("daemon.json")).filter(binary::is_older) {
            Some(rec) => restart_daemon(&rec),
            None => Ok(()),
        };
    }
    // A daemon that runs but cannot be reached, from inside a sandbox: starting another one is
    // no use.
    if daemon_info(&dir).is_some_and(|i| is_alive(i.pid)) {
        if let Some(why) = sandbox_hint() {
            return Err(why);
        }
    }
    if wait_for_restart() {
        return Ok(());
    }
    start_daemon()
}

fn ensure_daemon() {
    if let Err(e) = try_ensure_daemon() {
        fail(&e);
    }
}

/// Replaces a daemon from an older build with one started from this build's copy. Nothing is lost:
/// everything is on disk, peers queue what they send meanwhile, and listeners reconnect.
fn restart_daemon(rec: &Value) -> Result<(), String> {
    if wait_for_restart() {
        return Ok(());
    }
    let pid = vu(rec, "pid") as u32;
    let marker = restart_marker();
    let _ = write_json(&marker, &json!({ "pid": std::process::id(), "t": now_ms(), "replacing": pid }));
    daemon_log(&format!("denden {} built {} restarts the daemon (pid {pid}, built {}): it comes from an older build", binary::VERSION, time::iso(binary::BUILD_TIME), time::iso(vu(rec, "buildTime"))));
    if daemon_info(&home()).is_some_and(|i| i.pid == pid) {
        let _ = api_request(&home(), "POST", "/stop", Some(&json!({})), 5000);
    }
    let t0 = Instant::now();
    while is_alive(pid) && t0.elapsed() < Duration::from_secs(10) {
        sleep_ms(100);
    }
    let result = if daemon_up(&home()) { Ok(()) } else { start_daemon() };
    if read_json(&marker).and_then(|m| m.get("pid").and_then(Value::as_u64)) == Some(std::process::id() as u64) {
        let _ = std::fs::remove_file(&marker);
    }
    result
}

fn start_daemon() -> Result<(), String> {
    let log_file = home().join("daemon.log");
    // Our stdout and stderr are often a caller's pipes (a hook, an agent's shell tool): neither the
    // launcher nor the daemon may get them, or the caller would wait for the daemon to exit.
    no_inherit_std_handles();
    let mut pid: u32 = 0;
    for _ in 0..2 {
        let exe = match binary::ensure_copy() {
            Ok(copy) => copy,
            Err(e) => {
                let own = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("denden"));
                daemon_log(&format!("{e}; starting the daemon from {} instead", own.display()));
                own
            }
        };
        let out = hide_window(Command::new(exe).arg("__launch").current_dir(neutral_dir()).stdin(Stdio::null()).stderr(Stdio::null())).output();
        pid = out.ok().and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok()).unwrap_or(0);
        if pid != 0 {
            break;
        }
    }
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(20) && is_alive(pid) {
        sleep_ms(150);
        if daemon_up(&home()) {
            return Ok(());
        }
    }
    if daemon_up(&home()) {
        return Ok(());
    }
    if let Some(why) = sandbox_hint() {
        return Err(why);
    }
    // The log stays out of the conversation: it says what each machine did, which is no business of
    // the AI's. The user can read it.
    Err(format!("The daemon failed to start · its log is in {} · /denden:im check for a report", log_file.display()))
}

/// Inside the Codex sandbox, denden can reach neither the network nor its data folder, not even a
/// daemon already running here. Codex marks its commands with CODEX_SANDBOX_NETWORK_DISABLED, but
/// it leaves it set outside the sandbox too: it only explains a daemon that cannot be reached.
fn sandbox_hint() -> Option<String> {
    let codex = std::env::var("CODEX_SANDBOX").is_ok_and(|v| !v.is_empty()) || std::env::var("CODEX_SANDBOX_NETWORK_DISABLED").is_ok_and(|v| v == "1");
    codex.then(|| {
        "denden cannot reach its daemon from the Codex sandbox, which blocks the network and its data folder · run this command outside the sandbox (escalated permissions). If Codex answers that this session refuses such requests, switch /permissions to \"Ask for approval\"".to_string()
    })
}

/// One request to the daemon (started if needed). The JSON answer, errors included: an "error"
/// field, and "need" when the user has a decision to make.
fn call(method: &str, path: &str, body: Option<&Value>, timeout_ms: u64) -> Value {
    ensure_daemon();
    match call_up(method, path, body, timeout_ms) {
        Ok(v) => v,
        Err(e) => fail(&e),
    }
}

fn call_up(method: &str, path: &str, body: Option<&Value>, timeout_ms: u64) -> Result<Value, String> {
    let info = daemon_info(&home()).ok_or("the daemon is not running")?;
    let data = body.map(|b| serde_json::to_vec(b).unwrap_or_default());
    let (_, buf) = http::request_with(info.port, method, path, &info.token, data.as_deref(), Some(Duration::from_millis(timeout_ms)), |_| {}).map_err(|e| e.to_string())?;
    serde_json::from_slice(&buf).map_err(|_| "bad answer from the daemon".to_string())
}

/// The answer, or its error printed as an alert (and exit).
fn ok(v: Value) -> Value {
    if let Some(e) = v.get("error").and_then(Value::as_str) {
        fail(e);
    }
    v
}

fn q(path: &str, params: &[(&str, &str)]) -> String {
    let qs: Vec<String> = params.iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| format!("{k}={}", http::url_encode(v))).collect();
    if qs.is_empty() { path.to_string() } else { format!("{path}?{}", qs.join("&")) }
}

// ---------- once per machine ----------

/// The Claude rule that keeps the agent out of ~/.denden: a plugin cannot install it, so it is
/// suggested once, on the first create or join made from Claude.
fn deny_rule_note(h: &Here) {
    if !h.conv.as_deref().is_some_and(|c| c.starts_with("claude:")) {
        return;
    }
    let mut settings = read_json(&settings_file()).unwrap_or(json!({}));
    if truthy(settings.get("denyNoted")) {
        return;
    }
    if !check::claude_deny_rule() {
        println!("{}", info("Recommended once: add \"Read(~/.denden/**)\" to permissions.deny in ~/.claude/settings.json, so the AI never reads denden's keys · details: /denden:im check"));
    }
    settings["denyNoted"] = json!(true);
    let _ = write_json(&settings_file(), &settings);
}

// ---------- commands ----------

fn cmd_status(_pos: &[String], opts: &Opts) {
    let h = here(opts);
    let Some(conv) = &h.conv else {
        let channels = store::list_channels();
        if channels.is_empty() {
            return println!("{}", info("This machine is in no channel yet · /denden:im create <channel>"));
        }
        let labels: Vec<String> = channels.into_iter().map(|(_, l)| l).collect();
        return println!("{}", info(&format!("This machine ({}) is in: {}", machine_name(), labels.join(", "))));
    };
    let st = ok(call("GET", &q("/status", &[("conv", conv)]), None, 10000));
    let c = &st["conv"];
    if !truthy(c.get("attached")) {
        return println!("{}", info("This conversation is not attached to a denden · /denden:im create <channel> as <name>, or /denden:im join <channel> as <name>"));
    }
    println!("{}", info(&status_line(c)));
}

fn status_line(c: &Value) -> String {
    let online = arr(c, "online").len();
    format!("{} in {} · auto {} · {online} online · {} unread", vs(c, "denden"), vs(c, "label"), if truthy(c.get("auto")) { "on" } else { "off" }, vu(c, "unread"))
}

fn attached_lines(r: &Value, moved_on_request: bool) -> Vec<String> {
    let (d, label) = (vs(r, "denden"), vs(r, "label"));
    let unread = vu(r, "unread");
    let mut lines = Vec::new();
    if !r["moved"].is_null() && !moved_on_request {
        lines.push(info(&format!("{d} taken over here · the previous conversation was no longer listening")));
    }
    if !r["moved"].is_null() {
        let extra = if unread > 0 { format!(" · {unread} unread · history: /denden:im history") } else { " · history: /denden:im history".to_string() };
        lines.push(info(&format!("This conversation is now {d} in {label}{extra}")));
    } else {
        let online: Vec<String> = arr(r, "online").iter().filter_map(Value::as_str).map(str::to_string).collect();
        let who = if online.is_empty() { "nobody else online yet".to_string() } else { format!("online: {}", online.join(", ")) };
        let extra = if unread > 0 { format!(" · {unread} unread") } else { String::new() };
        lines.push(info(&format!("This conversation is {d} in {label} · {who}{extra}")));
    }
    lines
}

/// What an attach request answered: done, or a decision for the user (exit 3), or an error.
fn handle_attach(r: Value, h: &Here, label: &str, moved_on_request: bool) -> Option<Value> {
    match vs(&r, "need") {
        "" => {}
        "name" => {
            let example = Some(vs(&r, "suggest")).filter(|s| !s.is_empty()).unwrap_or("coord");
            ask_user(&format!("Which name for the denden of this conversation? Example: {example}"));
        }
        "other-name" | "move" => ask_user(vs(&r, "error")),
        "code" | "admission" => return Some(r),
        _ => fail(vs(&r, "error")),
    }
    if let Some(e) = r.get("error").and_then(Value::as_str) {
        fail(e);
    }
    let _ = (h, label);
    for l in attached_lines(&r, moved_on_request) {
        println!("{l}");
    }
    None
}

fn cmd_create(pos: &[String], opts: &Opts) {
    let h = here(opts);
    let (label, name) = channel_and_name(pos, opts);
    if label.is_empty() {
        fail("usage: /denden:im create <channel> [as <name>]");
    }
    let mut body = conv_fields(&h);
    body["label"] = json!(label);
    body["name"] = json!(name);
    let r = call("POST", "/create", Some(&body), 30000);
    if vs(&r, "need") == "label" {
        fail(&format!("This machine already has a channel called {label} · to attach this conversation to it: /denden:im join {label} as <name>"));
    }
    let r = ok(r);
    println!("{}", info(&format!("Channel {label} created on {}", vs(&r, "machine"))));
    deny_rule_note(&h);
    match r.get("attach") {
        Some(a) => {
            handle_attach(a.clone(), &h, &label, false);
        }
        None if h.conv.is_some() => ask_user("Which name for the denden of this conversation? Example: coord"),
        None => {}
    }
}

/// Waits until this machine is admitted to `label` (a member let it in), up to `secs`. Err: why
/// not yet, as far as this machine can tell.
fn wait_admission(label: &str, secs: u64) -> Result<(), String> {
    let t0 = Instant::now();
    loop {
        let st = call("GET", &q("/status", &[("label", label)]), None, 10000);
        let c = &st["channel"];
        if truthy(c.get("member")) {
            return Ok(());
        }
        if t0.elapsed() > Duration::from_secs(secs) {
            return Err(match c.get("refused").and_then(Value::as_str) {
                Some("not-trusted") => format!("A machine of {label} refused this one: the invitation was already used or has expired · ask for a new one (/denden:im invite on a machine of the channel), then run this join again"),
                Some("blocked") => format!("A machine of {label} has blocked this one"),
                _ if vu(c, "online") == 0 => format!(
                    "No machine of {label} reached yet{}: the inviting machine must be online, and both networks must let denden through (UDP) · this machine keeps trying in the background: run this join again in a minute",
                    if truthy(c.get("announced")) { "" } else { " (this machine is still announcing itself on the network)" }
                ),
                _ => format!("Connected to {label}, waiting for the certificate of the inviting machine · run this join again in a moment"),
            });
        }
        sleep_ms(500);
    }
}

/// Joins with an invitation code; Ok(label used) once the daemon has the channel.
fn join_with_code(code: &str, label: &str) -> Result<Value, String> {
    let r = call("POST", "/join", Some(&json!({ "code": code, "label": label })), 20000);
    if let Some(e) = r.get("error").and_then(Value::as_str) {
        return Err(e.to_string());
    }
    Ok(r)
}

fn cmd_join(pos: &[String], opts: &Opts) {
    let h = here(opts);
    let (label, name) = channel_and_name(pos, opts);
    if label.is_empty() || !valid_name(&label) {
        fail("usage: /denden:im join <channel> [as <name>]");
    }
    // In the user's own terminal: this machine joins, with the code typed hidden.
    let Some(_) = &h.conv else { return terminal_join(&label) };
    let mut body = conv_fields(&h);
    body["label"] = json!(label);
    body["name"] = json!(name);
    body["move"] = json!(flag(opts, "move"));
    let r = call("POST", "/attach", Some(&body), 20000);
    let Some(r) = handle_attach(r, &h, &label, flag(opts, "move")) else { return deny_rule_note(&h) };
    if vs(&r, "need") == "admission" {
        if let Err(why) = wait_admission(&label, 90) {
            fail(&why);
        }
    } else if no_display() {
        // A server without a desktop: a page on its 127.0.0.1 is out of the user's reach.
        println!("{}", info(&format!("{} is not in {label} yet, and has no browser for the local page", machine_name())));
        ask_user(&format!("In a separate terminal on {}, outside the AI, run {} join {label} and type the invitation code there. Tell me when it is done.", machine_name(), own_command()));
    } else {
        // Not in the channel yet: the invitation code, on a local page (never in the conversation).
        let no_browser = flag(opts, "no-browser");
        let cfg = ui::PageConfig { page: ui::Page::Join, name: name.clone(), label: label.clone(), expires: String::new() };
        let mut done = false;
        let joined = ui::serve_page(
            &cfg,
            |b| {
                if done {
                    return Err((410, "already joined".into()));
                }
                let code: String = b.get("code").and_then(Value::as_str).unwrap_or("").chars().filter(|c| !c.is_whitespace()).collect();
                match join_with_code(&code, &label) {
                    Ok(v) => {
                        done = true;
                        Ok((json!({ "ok": true, "label": label }), Some(v)))
                    }
                    Err(e) => Err((400, e)),
                }
            },
            |url| show_url(url, &format!("{} is not in {label} yet", machine_name()), "paste the invitation code", no_browser),
            Duration::from_millis(proto::INVITE_PAGE),
        );
        match joined {
            Ok(j) => {
                if !truthy(j.get("already")) {
                    println!("{}", info(&format!("{} joined {label}, invited by {} [{}]", machine_name(), vs(&j, "inviter"), vs(&j, "inviterFp"))));
                }
            }
            Err(_) => fail(&format!("The page closed without an invitation code · run /denden:im join {label} again when you have it")),
        }
        if let Err(why) = wait_admission(&label, 90) {
            fail(&why);
        }
    }
    let r = call("POST", "/attach", Some(&body), 20000);
    handle_attach(r, &h, &label, flag(opts, "move"));
    deny_rule_note(&h);
}

/// A Linux machine without a desktop (a server over SSH): no browser can show a local page there.
fn no_display() -> bool {
    cfg!(all(unix, not(target_os = "macos"))) && ["DISPLAY", "WAYLAND_DISPLAY"].iter().all(|v| std::env::var_os(v).is_none_or(|x| x.is_empty()))
}

/// How the user runs denden in their own terminal: the plugin's launcher (skills/im/denden, two
/// levels above bin/linux-x64/denden) when there is one, else this binary.
fn own_command() -> String {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("denden"));
    let launcher = exe.parent().and_then(|p| p.parent()).and_then(|p| p.parent()).map(|d| d.join("denden"));
    let path = launcher.filter(|l| l.is_file() && *l != exe).unwrap_or(exe).display().to_string();
    if path.contains(' ') { format!("\"{path}\"") } else { path }
}

/// `denden join <channel>` in the user's own terminal: the machine joins; any conversation of it
/// can then attach without a code.
fn terminal_join(label: &str) {
    if store::find_channel(label).is_some() {
        println!("This machine is already in {label}. Attach a conversation to it with /denden:im join {label} as <name>.");
        return;
    }
    if !std::io::stdin().is_terminal() {
        fail(&format!("run this in your own terminal (outside the AI): denden join {label}"));
    }
    let code = ui::ask_hidden("Invitation code: ");
    match join_with_code(&code, label) {
        Ok(j) if truthy(j.get("already")) => println!("This machine is already in this channel (here: {}).", vs(&j, "label")),
        Ok(j) => {
            println!("Joining {label}, invited by {} [{}]...", vs(&j, "inviter"), vs(&j, "inviterFp"));
            match wait_admission(label, 90) {
                Ok(()) => println!("{} joined {label}. Attach a conversation with /denden:im join {label} as <name>.", machine_name()),
                Err(why) => println!("{why}"),
            }
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

/// Prints the local page's URL, opened in the browser unless `no_browser` (or DENDEN_NO_BROWSER, or
/// no browser could be started).
fn show_url(url: &str, why: &str, what: &str, no_browser: bool) {
    if !no_browser && ui::open_browser(url, false) {
        println!("{}", info(&format!("{why} · local page opened in your browser: {what} there")));
    } else {
        println!("{}", info(&format!("{why} · local page: {url} · open it in the built-in browser and {what} there")));
    }
}

fn cmd_invite(pos: &[String], opts: &Opts) {
    let h = here(opts);
    let label = pos.first().cloned().unwrap_or_default();
    if flag(opts, "print") && !std::io::stdout().is_terminal() {
        fail("--print only works in your own terminal: the code must not land in an AI conversation");
    }
    let r = ok(call("POST", "/invite", Some(&json!({ "conv": h.conv, "label": label })), 10000));
    let code = vs(&r, "code").to_string();
    let (label, until) = (vs(&r, "label").to_string(), time::fmt_hm(vu(&r, "expires")));
    if flag(opts, "print") {
        println!("{code}");
        return eprintln!("(single use, valid until {until})");
    }
    // The page never holds the code: its copy button asks for it once (POST <page>/api/reveal).
    let cfg = ui::PageConfig { page: ui::Page::Invite, name: String::new(), label: label.clone(), expires: until.clone() };
    let mut revealed = false;
    let no_browser = flag(opts, "no-browser");
    let r = ui::serve_page(
        &cfg,
        |_| {
            if revealed {
                return Err((410, "already revealed".into()));
            }
            revealed = true;
            Ok((json!({ "code": code }), Some(())))
        },
        |url| {
            if !no_browser && ui::open_browser(url, false) {
                println!("{}", info("Invitation opened in your browser · click to copy the code · single use, valid 20 min"));
            } else {
                println!("{}", info(&format!("Invitation page: {url} · open it in the built-in browser, then click to copy the code · single use, valid 20 min")));
            }
        },
        Duration::from_millis(proto::INVITE_PAGE),
    );
    match r {
        Ok(()) => println!("{}", info(&format!("Invitation for {label} copied · single use, valid until {until}"))),
        Err(_) => println!("{}", info("The invitation page closed after 15 min without the code being copied · /denden:im invite again")),
    }
}

// Opens a denden local page (printed by invite or join with --no-browser) in the system browser,
// even with DENDEN_NO_BROWSER: the user asked for it. Nothing else: no other URL, no file.
fn cmd_open(pos: &[String], _opts: &Opts) {
    let url = pos.first().map(String::as_str).unwrap_or("");
    if !ui::is_local_page(url) {
        fail("open only opens denden local pages (http://127.0.0.1:<port>/p/<token>)");
    }
    if ui::open_browser(url, true) {
        return println!("{}", info("Page opened in your browser"));
    }
    fail(&format!("could not open a browser; open {url} yourself"));
}

fn cmd_who(pos: &[String], opts: &Opts) {
    let h = here(opts);
    let label = pos.first().cloned().unwrap_or_default();
    let r = ok(call("GET", &q("/who", &[("conv", h.conv.as_deref().unwrap_or("")), ("label", &label)]), None, 10000));
    let rows = arr(&r, "denden");
    let mut lines = vec![info(&format!("{} · {} denden on {} machine(s)", vs(&r, "label"), rows.len(), vu(&r, "machines")))];
    if !rows.is_empty() {
        lines.push("| denden | machine | role | state |".into());
        lines.push("|---|---|---|---|".into());
        for row in rows {
            let app = match vs(&row, "app") {
                "claude" => " · Claude",
                "codex" => " · Codex",
                _ => "",
            };
            let you = if truthy(row.get("you")) { " (you)" } else { "" };
            let role = vs(&row, "role").replace('|', "/");
            lines.push(format!("| {}{you} | {}{app} | {role} | {} |", vs(&row, "denden"), vs(&row, "machine"), vs(&row, "state")));
        }
    }
    println!("{}", lines.join("\n"));
}

fn message_text(pos: &[String], opts: &Opts) -> String {
    let text = if let Some(f) = opts.get("file") {
        std::fs::read_to_string(f).unwrap_or_else(|e| fail(&e.to_string()))
    } else if pos.is_empty() || (pos.len() == 1 && pos[0] == "-") {
        if std::io::stdin().is_terminal() {
            fail("no message given");
        }
        ui::read_stdin()
    } else {
        pos.join(" ")
    };
    let text = text.trim_end().to_string();
    if text.is_empty() {
        fail("empty message");
    }
    text
}

fn sent_lines(r: &Value, text: &str) -> Vec<String> {
    let to = match r.get("to") {
        Some(Value::String(_)) => format!("all ({} denden)", vu(r, "count")),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "),
        _ => String::new(),
    };
    let kind = if !r["re"].is_null() {
        " · reply"
    } else if truthy(r.get("ask")) {
        " · reply expected"
    } else {
        ""
    };
    let mut lines = vec![info(&format!("→ {to}{kind} : {text}"))];
    let names = |v: &Value| {
        let n = arr(v, "names").iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>();
        if n.is_empty() { vs(v, "machine").to_string() } else { n.join(", ") }
    };
    for qd in arr(r, "queued") {
        lines.push(info(&format!("{} offline: queued, delivered when it comes back (up to 7 days)", names(&qd))));
    }
    for rf in arr(r, "refused") {
        lines.push(alert(&format!("{} refused the message: {}", names(&rf), vs(&rf, "error"))));
    }
    lines
}

fn send_common(h: &Here, body: Value, text: &str) {
    let mut body = body;
    body["conv"] = json!(need_conv(h));
    body["text"] = json!(text);
    let r = ok(call("POST", "/send", Some(&body), 30000));
    println!("{}", sent_lines(&r, text).join("\n"));
}

fn cmd_send(pos: &[String], opts: &Opts, ask: bool) {
    let h = here(opts);
    let Some(to) = pos.first() else { fail(&format!("usage: /denden:im {} <to> <text>   (<to>: a name, a,b,c, or all)", if ask { "ask" } else { "send" })) };
    let text = message_text(&pos[1..], opts);
    send_common(&h, json!({ "to": to, "ask": ask, "force": flag(opts, "force") }), &text);
}

fn cmd_reply(pos: &[String], opts: &Opts) {
    let h = here(opts);
    let Some(id) = pos.first().map(|i| i.trim_start_matches('#').to_string()) else { fail("usage: denden reply <id> <text>") };
    let text = message_text(&pos[1..], opts);
    send_common(&h, json!({ "re": id, "force": flag(opts, "force") }), &text);
}

/// Items handed to a conversation, as lines: a header when there are several, a question when some
/// arrived late (they wait for the user's go-ahead, whatever the mode).
fn handover_lines(items: &[Value], me: &str, auto: bool) -> Vec<String> {
    let msgs = items.iter().filter(|i| vs(i, "kind") == "msg").count();
    let late = items.iter().filter(|i| truthy(i.get("late"))).count();
    let mut lines = Vec::new();
    if late > 0 {
        let n = if late == 1 { "1 message".to_string() } else { format!("{late} messages") };
        lines.push(info(&format!("{n} arrived while you were away")));
    } else if msgs > 1 {
        lines.push(info(&format!("{msgs} new messages for {me} · auto {}", if auto { "on" } else { "off" })));
    }
    lines.extend(items.iter().map(|i| output::item(i, me)));
    if late > 0 {
        lines.push(question("Which ones should I handle? All, some, or none?"));
    }
    lines
}

fn cmd_inbox(_pos: &[String], opts: &Opts) {
    let h = here(opts);
    let conv = need_conv(&h);
    let peek = if flag(opts, "peek") { "1" } else { "" };
    let r = ok(call("GET", &q("/inbox", &[("conv", &conv), ("peek", peek)]), None, 10000));
    let items = arr(&r, "items");
    if items.is_empty() {
        return println!("{}", info("No new messages"));
    }
    let me = ok(call("GET", &q("/status", &[("conv", &conv)]), None, 10000))["conv"]["denden"].as_str().unwrap_or("").to_string();
    println!("{}", handover_lines(&items, &me, truthy(r.get("auto"))).join("\n"));
}

fn cmd_history(pos: &[String], opts: &Opts) {
    let h = here(opts);
    let conv = need_conv(&h);
    let n = opts.get("n").cloned().unwrap_or_default();
    let with = pos.first().cloned().unwrap_or_default();
    let r = ok(call("GET", &q("/history", &[("conv", &conv), ("with", &with), ("n", &n)]), None, 10000));
    let list = arr(&r, "messages");
    if list.is_empty() {
        return println!("{}", info("No messages yet"));
    }
    let me = vs(&r, "denden").to_string();
    let lines: Vec<String> = list.iter().map(|m| if vs(m, "dir") == "out" { output::sent(m) } else { output::item(m, &me) }).collect();
    println!("{}", lines.join("\n"));
}

fn set(h: &Here, body: Value) -> Value {
    let mut body = body;
    body["conv"] = json!(need_conv(h));
    ok(call("POST", "/set", Some(&body), 10000))
}

fn on_off(pos: &[String], usage: &str) -> bool {
    match pos.first().map(|s| s.to_lowercase()).as_deref() {
        Some("on") => true,
        Some("off") => false,
        _ => fail(usage),
    }
}

fn cmd_auto(pos: &[String], opts: &Opts) {
    let h = here(opts);
    let on = on_off(pos, "usage: /denden:im auto on|off");
    let r = set(&h, json!({ "auto": on }));
    let d = vs(&r, "denden");
    println!("{}", info(&if on { format!("{d} now handles requests without waiting for your go-ahead") } else { format!("{d} asks you before acting on a request") }));
}

fn cmd_wakes(pos: &[String], opts: &Opts) {
    let h = here(opts);
    let n: u64 = match pos.first().map(|s| s.to_lowercase()).as_deref() {
        Some("off") => 0,
        Some(v) => v.parse().ok().filter(|n| *n > 0).unwrap_or_else(|| fail("usage: /denden:im wakes <n>|off")),
        None => fail("usage: /denden:im wakes <n>|off"),
    };
    let r = set(&h, json!({ "wakes": n }));
    let d = vs(&r, "denden");
    println!("{}", info(&if n == 0 { format!("No wake-up limit for {d}") } else { format!("Up to {n} wake-ups per hour for {d}") }));
}

fn cmd_role(pos: &[String], opts: &Opts) {
    let h = here(opts);
    let role = pos.join(" ").trim().trim_matches('"').to_string();
    let r = set(&h, json!({ "role": role }));
    println!("{}", info(&if role.is_empty() { format!("Role of {} removed", vs(&r, "denden")) } else { format!("Role of {}: {}", vs(&r, "denden"), vs(&r, "role")) }));
}

fn cmd_keepalive(pos: &[String], _opts: &Opts) {
    let on = on_off(pos, "usage: /denden:im keepalive on|off");
    ok(call("POST", "/keepalive", Some(&json!({ "on": on })), 10000));
    let m = machine_name();
    println!("{}", info(&if on { format!("The daemon of {m} stays up even without an attached conversation") } else { format!("The daemon of {m} stops when no conversation listens any more") }));
}

fn cmd_detach(_pos: &[String], opts: &Opts) {
    let h = here(opts);
    let r = ok(call("POST", "/detach", Some(&json!({ "conv": need_conv(&h) })), 10000));
    println!("{}", info(&format!("This conversation is no longer attached to {} · its messages wait for the next conversation that takes it", vs(&r, "denden"))));
}

fn cmd_delete(pos: &[String], opts: &Opts) {
    let h = here(opts);
    let Some(name) = pos.first() else { fail("usage: /denden:im delete <name>") };
    let r = ok(call("POST", "/delete", Some(&json!({ "conv": h.conv, "name": name })), 10000));
    println!("{}", info(&format!("{} deleted from {} · the name stays bound to this machine", vs(&r, "denden"), vs(&r, "label"))));
}

fn cmd_block(pos: &[String], opts: &Opts, on: bool) {
    let h = here(opts);
    if pos.is_empty() {
        fail(&format!("usage: /denden:im {} <machine>", if on { "block" } else { "unblock" }));
    }
    let r = ok(call("POST", if on { "/block" } else { "/unblock" }, Some(&json!({ "conv": h.conv, "machine": pos.join(" ") })), 10000));
    let (m, l) = (vs(&r, "machine"), vs(&r, "label"));
    println!("{}", info(&if on { format!("{m} is blocked here, in {l}: its messages and connections are refused") } else { format!("{m} is no longer blocked in {l}") }));
}

fn cmd_leave(pos: &[String], opts: &Opts) {
    let h = here(opts);
    let label = pos.first().cloned().unwrap_or_default();
    let target = if label.is_empty() { ok(call("GET", &q("/status", &[("conv", h.conv.as_deref().unwrap_or(""))]), None, 10000))["conv"]["label"].as_str().unwrap_or("").to_string() } else { label };
    if target.is_empty() {
        fail("usage: /denden:im leave <channel>");
    }
    ok(call("POST", "/leave", Some(&json!({ "label": target })), 20000));
    println!("{}", info(&format!("This machine left {target}")));
}

fn cmd_check(_pos: &[String], opts: &Opts) {
    let report = check::security_report();
    if flag(opts, "json") {
        return println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
    }
    println!("{}", check::format_report(&report));
}

fn cmd_stop(_pos: &[String], _opts: &Opts) {
    if !daemon_up(&home()) {
        return println!("the daemon was not running");
    }
    let pid = daemon_info(&home()).map(|i| i.pid).unwrap_or(0);
    let _ = call_up("POST", "/stop", Some(&json!({})), 5000);
    for _ in 0..50 {
        if !is_alive(pid) {
            break;
        }
        sleep_ms(100);
    }
    println!("the daemon is stopped");
}

// ---------- listening (Claude) ----------

/// Messages from a listener carry their seq: written out first, then marked read.
fn cmd_watch(_pos: &[String], opts: &Opts) {
    let h = here(opts);
    let conv = need_conv(&h);
    // Runs until killed, or for --timeout seconds: a Claude Code Monitor kills its command when it
    // expires, but under Git Bash the process it kills is not ours, so a watcher started with a
    // timeout just under the Monitor's ends by itself first.
    orphan::exit_when_orphaned();
    static OUTPUT: std::sync::Mutex<()> = std::sync::Mutex::new(());
    if let Some(secs) = opts.get("timeout").and_then(|t| t.trim().parse::<f64>().ok()).filter(|t| *t > 0.0 && t.is_finite()) {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs_f64(secs.min(1e9)));
            let mut _held = None;
            for _ in 0..50 {
                if let Ok(guard) = OUTPUT.try_lock() {
                    _held = Some(guard);
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            std::process::exit(0);
        });
    }
    let st = ok(call("GET", &q("/status", &[("conv", &conv)]), None, 10000));
    let me = st["conv"]["denden"].as_str().unwrap_or("").to_string();
    if me.is_empty() {
        fail("this conversation is not attached to a denden: nothing to listen to");
    }
    let path = q("/watch", &[("conv", &conv)]);
    loop {
        ensure_daemon();
        if let Some(info) = daemon_info(&home()) {
            let mut buf: Vec<u8> = Vec::new();
            let mut end = false;
            let _ = http::stream(info.port, &path, &info.token, |data| {
                buf.extend_from_slice(data);
                while let Some(i) = buf.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = buf.drain(..=i).collect();
                    let Ok(it) = serde_json::from_slice::<Value>(&line[..line.len() - 1]) else { continue };
                    if vs(&it, "kind") == "ping" {
                        continue;
                    }
                    let _one = OUTPUT.lock();
                    emit(&output::item(&it, &me));
                    if truthy(it.get("detached")) {
                        end = true;
                        std::process::exit(0);
                    }
                    if let Some(seq) = it.get("seq").and_then(Value::as_u64) {
                        let _ = call_up("POST", "/read", Some(&json!({ "conv": conv, "seqs": [seq] })), 5000);
                    }
                }
            });
            if end {
                return;
            }
        }
        // The daemon went away (an upgrade): resume when it is back, unless the denden is gone.
        sleep_ms(1000);
        if let Ok(st) = call_up("GET", &q("/status", &[("conv", &conv)]), None, 5000) {
            if !truthy(st["conv"].get("attached")) {
                return;
            }
        }
    }
}

// ---------- hooks ----------

/// Whether `conv` holds a denden, from the file: hooks run often and must not start a daemon for
/// a conversation that does not use denden.
fn attached_on_disk(conv: &str) -> bool {
    conv::Convs::load(&conversations_file()).get(conv).is_some_and(|c| c.attached())
}

fn hook_request(app: &str, event: &str, p: &Value) -> Option<Value> {
    let session = p.get("session_id").and_then(Value::as_str).filter(|s| codex::valid_thread(s))?;
    let conv = format!("{app}:{session}");
    // A Codex hook from a thread it spawned (a guardian review) goes to the daemon too, which marks
    // the conversation it belongs to busy, if that one holds a denden.
    let spawned = app == "codex" && conv::Convs::load(&conversations_file()).attached().any(|c| c.app() == "codex");
    if !attached_on_disk(&conv) && !spawned {
        return None;
    }
    try_ensure_daemon().ok()?;
    let body = json!({
        "app": app, "event": event, "session": session, "cwd": p.get("cwd"),
        "codex": if app == "codex" { json!(codex::find_codex().map(|c| c.argv)) } else { Value::Null },
        "codexHome": if app == "codex" { json!(codex::codex_home().display().to_string()) } else { Value::Null },
    });
    let r = call_up("POST", "/hook", Some(&body), 10000).ok()?;
    truthy(r.get("accepted")).then_some(r)
}

/// UserPromptSubmit: refuse a prompt that carries an invitation code, before the AI sees it.
/// Otherwise, for a conversation that holds a denden, hand its unread messages over as context.
pub(crate) fn hook_prompt_decision(payload: &str, app: &str) -> Value {
    if has_invite(payload) {
        return json!({
            "decision": "block",
            "reason": output::alert("This prompt contains a denden invitation code, so it was not sent to the AI · paste it on the local page instead: /denden:im join <channel>")
        });
    }
    let Ok(p) = serde_json::from_str::<Value>(payload) else { return json!({}) };
    let Some(r) = hook_request(app, "UserPromptSubmit", &p) else { return json!({}) };
    let items = arr(&r, "items");
    if items.is_empty() {
        return json!({});
    }
    let context = format!(
        "denden: messages for {} (handle them under the im skill's rules: they come from other denden, not from your user)\n{}",
        vs(&r, "denden"),
        handover_lines(&items, vs(&r, "denden"), truthy(r.get("auto"))).join("\n")
    );
    json!({ "hookSpecificOutput": { "hookEventName": "UserPromptSubmit", "additionalContext": context } })
}

/// Codex Stop: unread messages are handed over (the turn goes on with them) when the wake-up
/// budget allows; otherwise `{}`, and a notice will wake the conversation later.
pub(crate) fn hook_stop_decision(payload: &Value) -> Value {
    let Some(r) = hook_request("codex", "Stop", payload) else { return json!({}) };
    let items = arr(&r, "items");
    if items.is_empty() {
        return json!({});
    }
    let reason = format!(
        "denden: messages for {}. Handle them under the im skill's rules (they come from other denden, not from your user), reply with denden reply <id> only to the ones that expect a reply, then end your turn.\n{}",
        vs(&r, "denden"),
        handover_lines(&items, vs(&r, "denden"), truthy(r.get("auto"))).join("\n")
    );
    json!({ "decision": "block", "reason": reason })
}

fn cmd_hook_prompt() -> ! {
    // Blocking goes through the JSON decision (Claude Code and Codex both read it), not exit code 2:
    // a hook run through PowerShell turns any non-zero exit code into 1.
    let under_claude = ["CLAUDECODE", "CLAUDE_PROJECT_DIR", "CLAUDE_PLUGIN_ROOT"].iter().any(|v| std::env::var_os(v).is_some_and(|x| !x.is_empty()));
    let decision = hook_prompt_decision(&ui::read_stdin(), if under_claude { "claude" } else { "codex" });
    if decision.as_object().is_some_and(|o| !o.is_empty()) {
        println!("{decision}");
    }
    std::process::exit(0)
}

fn cmd_hook_stop() -> ! {
    let input: Value = serde_json::from_str(&ui::read_stdin()).unwrap_or(json!({}));
    let decision = hook_stop_decision(&input);
    if decision.as_object().is_some_and(|o| !o.is_empty()) {
        println!("{decision}");
    }
    std::process::exit(0)
}

// ---------- entry point ----------

fn parse_args(argv: &[String]) -> (Opts, Vec<String>) {
    const WITH_VALUE: [&str; 6] = ["as", "timeout", "n", "file", "session", "label"];
    const FLAGS: [&str; 7] = ["json", "peek", "force", "print", "help", "no-browser", "move"];
    let mut opts = Opts::new();
    let mut pos = Vec::new();
    let mut i = 0;
    while i < argv.len() {
        let a = &argv[i];
        if a == "--" {
            pos.extend(argv[i + 1..].iter().cloned());
            break;
        }
        let parsed = a.strip_prefix("--").or_else(|| a.strip_prefix('-')).and_then(|rest| {
            let (k, v) = match rest.split_once('=') {
                Some((k, v)) => (k, Some(v.to_string())),
                None => (rest, None),
            };
            (!k.is_empty() && k.bytes().all(|c| c.is_ascii_lowercase() || c == b'-')).then(|| (k.to_string(), v))
        });
        if let Some((k, v)) = &parsed {
            if WITH_VALUE.contains(&k.as_str()) {
                let v = v.clone().or_else(|| {
                    i += 1;
                    argv.get(i).cloned()
                });
                opts.insert(k.clone(), v.unwrap_or_default());
                i += 1;
                continue;
            }
            if FLAGS.contains(&k.as_str()) {
                opts.insert(k.clone(), "1".into());
                i += 1;
                continue;
            }
        }
        if a == "-h" {
            opts.insert("help".into(), "1".into());
            i += 1;
            continue;
        }
        pos.push(a.clone());
        i += 1;
    }
    (opts, pos)
}

fn main() {
    pin_im_home();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    // `denden --session <id>` alone is the status: no command name, options only.
    let cmd = argv.first().filter(|a| !a.starts_with('-')).cloned().unwrap_or_default();
    let rest = if cmd.is_empty() { &argv[..] } else { &argv[1..] };
    match cmd.as_str() {
        "hook-prompt" => cmd_hook_prompt(),
        "hook-stop" => cmd_hook_stop(),
        "mcp" => mcp::serve(),
        "__launch" => launch_daemon(),
        "__daemon" => {
            if let Err(e) = daemon::run() {
                eprintln!("{} fatal: {e}", time::iso(now_ms()));
                std::process::exit(1);
            }
            std::process::exit(0);
        }
        _ => {}
    }
    let (opts, pos) = parse_args(rest);
    if let Err(e) = ensure_dir(&channels_dir()) {
        let code = match e.kind() {
            std::io::ErrorKind::PermissionDenied => Some("EACCES"),
            std::io::ErrorKind::ReadOnlyFilesystem => Some("EROFS"),
            _ => None,
        };
        match code {
            Some(code) => fail(&format!(
                "cannot write {} ({code}): this is probably an agent sandbox (Codex). denden needs its data folder and the network: run this command outside the sandbox (escalated permissions)",
                home().display()
            )),
            None => fail(&e.to_string()),
        }
    }
    if flag(&opts, "help") || cmd == "help" {
        println!("{}", help());
        std::process::exit(0);
    }
    match cmd.as_str() {
        "" | "status" => cmd_status(&pos, &opts),
        "create" => cmd_create(&pos, &opts),
        "join" => cmd_join(&pos, &opts),
        "invite" => cmd_invite(&pos, &opts),
        "open" => cmd_open(&pos, &opts),
        "who" => cmd_who(&pos, &opts),
        "send" => cmd_send(&pos, &opts, false),
        "ask" => cmd_send(&pos, &opts, true),
        "reply" => cmd_reply(&pos, &opts),
        "inbox" => cmd_inbox(&pos, &opts),
        "history" => cmd_history(&pos, &opts),
        "role" => cmd_role(&pos, &opts),
        "auto" => cmd_auto(&pos, &opts),
        "wakes" => cmd_wakes(&pos, &opts),
        "keepalive" => cmd_keepalive(&pos, &opts),
        "detach" => cmd_detach(&pos, &opts),
        "delete" => cmd_delete(&pos, &opts),
        "block" => cmd_block(&pos, &opts, true),
        "unblock" => cmd_block(&pos, &opts, false),
        "leave" => cmd_leave(&pos, &opts),
        "check" => cmd_check(&pos, &opts),
        "watch" => cmd_watch(&pos, &opts),
        "stop" => cmd_stop(&pos, &opts),
        _ => {
            println!("{}", help());
            std::process::exit(1);
        }
    }
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn channel_then_name() {
        let o = Opts::new();
        assert_eq!(channel_and_name(&strs(&["vibedoc-chan", "as", "coord"]), &o), ("vibedoc-chan".into(), "coord".into()));
        assert_eq!(channel_and_name(&strs(&["Vibedoc-Chan", "Coord"]), &o), ("vibedoc-chan".into(), "coord".into()));
        assert_eq!(channel_and_name(&strs(&["vibedoc-chan"]), &o), ("vibedoc-chan".into(), String::new()));
        let mut with = Opts::new();
        with.insert("as".into(), "@front".into());
        assert_eq!(channel_and_name(&strs(&["vibedoc-chan"]), &with), ("vibedoc-chan".into(), "front".into()));
    }

    #[test]
    fn late_messages_ask_the_user() {
        let items = vec![json!({ "kind": "msg", "id": "a", "from": "coord", "to": ["db"], "text": "Deploy", "late": true, "ago": "2 h ago" })];
        let lines = handover_lines(&items, "db", true);
        assert_eq!(lines[0], "> 🐌» (( 1 message arrived while you were away ))");
        assert!(lines[1].contains("late, sent 2 h ago"));
        assert_eq!(lines[2], "🐌»  **Which ones should I handle? All, some, or none?**");
    }
}
