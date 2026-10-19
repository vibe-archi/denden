// The full report `check` prints on request: for each AI host on this machine (Claude Code, Codex),
// which protections are in place and how to add the missing ones; the binary; the daemon. Read-only.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::binary::file_sha256;
use crate::core::{daemon_info, home, home_dir, is_alive, read_json, truthy};

fn read_text(p: &Path) -> String {
    fs::read_to_string(p).unwrap_or_default()
}

fn tilde(p: &Path) -> String {
    let s = p.display().to_string();
    let h = home_dir().display().to_string();
    match s.strip_prefix(&h) {
        Some(rest) => format!("~{}", rest.replace('\\', "/")),
        None => s,
    }
}

/// Which agent runs this command: Claude Code sets CLAUDECODE; Codex sets CODEX_* variables
/// (CODEX_HOME and CODEX_MANAGED_* also exist outside Codex, so they do not count).
pub fn detect_host() -> Option<&'static str> {
    if std::env::var_os("CLAUDECODE").is_some_and(|v| !v.is_empty()) {
        return Some("claude");
    }
    let codex = std::env::vars_os().any(|(k, _)| {
        let k = k.to_string_lossy();
        k.starts_with("CODEX_") && !k.starts_with("CODEX_HOME") && !k.starts_with("CODEX_MANAGED_")
    });
    codex.then_some("codex")
}

fn item(status: &str, what: String, fix: Option<String>) -> Value {
    match fix {
        Some(f) => json!({ "status": status, "what": what, "fix": f }),
        None => json!({ "status": status, "what": what }),
    }
}

/// Claude Code's settings files that apply here: the user's, then the project's.
fn claude_settings() -> Vec<Value> {
    let cwd = std::env::current_dir().unwrap_or_default();
    [home_dir().join(".claude").join("settings.json"), cwd.join(".claude").join("settings.json"), cwd.join(".claude").join("settings.local.json")]
        .iter()
        .filter_map(|f| read_json(f))
        .filter(|v| v.is_object())
        .collect()
}

fn list_in(sets: &[Value], get: &dyn Fn(&Value) -> Option<Value>) -> Vec<Value> {
    sets.iter().filter_map(get).flat_map(|v| v.as_array().cloned().unwrap_or_default()).collect()
}

/// Whether a Claude deny rule keeps the agent from reading ~/.denden.
pub fn claude_deny_rule() -> bool {
    let sets = claude_settings();
    list_in(&sets, &|s| s.get("permissions").and_then(|p| p.get("deny")).cloned()).iter().filter_map(Value::as_str).any(|r| r.starts_with("Read(") && r.contains(".denden"))
}

fn claude_report(cmd: &str) -> Option<Value> {
    let dir = home_dir().join(".claude");
    if !dir.exists() {
        return None;
    }
    let user_file = dir.join("settings.json");
    let sets = claude_settings();
    let hook_commands = |event: &str| -> Vec<String> {
        list_in(&sets, &|s| s.get("hooks").and_then(|h| h.get(event)).cloned())
            .iter()
            .flat_map(|g| g.get("hooks").and_then(Value::as_array).cloned().unwrap_or_default())
            .map(|h| h.get("command").and_then(Value::as_str).unwrap_or("").to_string())
            .collect()
    };
    let mode = sets
        .iter()
        .filter_map(|s| s.get("permissions").and_then(|p| p.get("defaultMode")).and_then(Value::as_str))
        .filter(|m| !m.is_empty())
        .last()
        .unwrap_or("default")
        .to_string();
    let allow = list_in(&sets, &|s| s.get("permissions").and_then(|p| p.get("allow")).cloned());
    let sends_unasked = ["auto", "bypassPermissions"].contains(&mode.as_str()) || allow.iter().filter_map(Value::as_str).any(|r| r == "Bash" || r.starts_with("Bash(*"));
    let sandboxed = sets.iter().any(|s| truthy(s.get("sandbox").and_then(|x| x.get("enabled"))));
    let plugin = sets.iter().any(|s| s.get("enabledPlugins").and_then(Value::as_object).is_some_and(|m| m.iter().any(|(id, on)| truthy(Some(on)) && id.starts_with("denden@"))));
    let manual_hook = hook_commands("UserPromptSubmit").iter().any(|c| c.contains("hook-prompt"));
    let h = tilde(&home());
    let uf = tilde(&user_file);
    let mut items = vec![
        if claude_deny_rule() {
            item("ok", format!("the agent cannot read {h} (channel secrets and keys)"), None)
        } else {
            item("missing", format!("the agent can read {h} (channel secrets and keys)"), Some(format!("add \"Read({h}/**)\" to permissions.deny in {uf}")))
        },
        if plugin || manual_hook {
            item("ok", "an invitation code pasted in the chat is blocked before the AI sees it".into(), None)
        } else {
            item("missing", "an invitation code pasted in the chat would reach the AI".into(), Some(format!("install the denden plugin (see the README), or add a UserPromptSubmit hook in {uf} running: {cmd} hook-prompt")))
        },
    ];
    if plugin && manual_hook {
        items.push(item("advice", "the hook-prompt hook is declared twice (denden plugin and settings)".into(), Some(format!("remove it from {uf}: the plugin already provides it"))));
    }
    items.push(if sends_unasked {
        item(
            "advice",
            format!("messages may be sent without asking you (permission mode \"{mode}\")"),
            Some("for sensitive work, use the default permission mode in that session, so each message asks first".into()),
        )
    } else {
        item("ok", "you approve each message denden sends".into(), None)
    });
    items.push(if sandboxed {
        item("ok", format!("shell commands are sandboxed, so they cannot read {h} either"), None)
    } else {
        item("advice", format!("shell commands (cat, type...) could still read {h}"), Some("enable the Claude Code sandbox (sandbox.enabled): the Read deny rule then applies to shell commands too".into()))
    });
    Some(json!({ "id": "claude", "name": "Claude Code", "where": uf, "items": items }))
}

fn codex_report(cmd: &str) -> Option<Value> {
    let dir = std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home_dir().join(".codex"));
    if !dir.exists() {
        return None;
    }
    let hooks_file = dir.join("hooks.json");
    let config = read_text(&dir.join("config.toml"));
    let cwd = std::env::current_dir().unwrap_or_default();
    let raw = [read_text(&hooks_file), config.clone(), read_text(&cwd.join(".codex").join("hooks.json"))].join("\n");
    let re = |p: &str| regex_lite::Regex::new(p).expect("valid pattern");
    let runs_unasked = re(r#"approval_policy\s*=\s*"never""#).is_match(&raw);
    // The plugin brings both hooks, but Codex runs a plugin hook only once the user has trusted it.
    let plugin = re(r#"\[plugins\."denden@[^"]+"\]\s*\r?\n\s*enabled\s*=\s*true"#).is_match(&config);
    let trusted = |event: &str| re(&format!(r#"\[hooks\.state\.["']denden@[^:"']+:[^"']*:{event}:[^\]]*\]\s*\r?\n\s*trusted_hash\s*="#)).is_match(&config);
    let hf = tilde(&hooks_file);
    let hook_item = |marker: &str, event: &str, ok: &str, missing: &str, manual_fix: String| {
        if raw.contains(marker) {
            item("ok", ok.into(), None)
        } else if plugin && trusted(event) {
            item("ok", ok.into(), None)
        } else if plugin {
            item("missing", format!("{missing}: the denden plugin's hook is not approved yet"), Some("open /hooks in Codex and trust the hooks of the denden plugin".into()))
        } else {
            item("missing", missing.into(), Some(format!("install the denden plugin (see the README), or {manual_fix}")))
        }
    };
    let h = tilde(&home());
    let mut items = vec![
        if plugin {
            item("ok", "the denden plugin is installed for Codex ($im)".into(), None)
        } else {
            item("missing", "the denden plugin is not installed for Codex".into(), Some("install it (see the README)".into()))
        },
        hook_item(
            "hook-prompt",
            "user_prompt_submit",
            "an invitation code pasted in the chat is blocked before the AI sees it",
            "an invitation code pasted in the chat would reach the AI",
            format!("add a UserPromptSubmit hook in {hf} running: {cmd} hook-prompt, then approve it once in /hooks"),
        ),
        hook_item("hook_stop", "stop", "messages are handed over at the end of a turn (Stop hook)", "messages wait for the next prompt instead of the end of the turn", format!("add a Stop hook in {hf} running: {cmd} hook-stop, then approve it once in /hooks")),
        item("note", format!("denden commands need the network and {h}, which the Codex sandbox blocks"), Some(format!("approve running {cmd} outside the sandbox when Codex asks; keep the sandbox for everything else"))),
    ];
    if runs_unasked {
        items.push(item("advice", "Codex runs commands without asking you (approval_policy \"never\")".into(), Some("prefer on-request approvals, so messages are shown to you before they go".into())));
    }
    Some(json!({ "id": "codex", "name": "Codex", "where": tilde(&dir), "items": items }))
}

/// The codex binary that queues wake-ups, and whether its `queue` command answers.
fn codex_item() -> Value {
    match crate::codex::find_codex() {
        Some(c) => {
            let probe = crate::codex::run(&c.argv, &["queue", "--help"], std::time::Duration::from_secs(15));
            item(
                if probe.is_ok() { "ok" } else { "warn" },
                format!(
                    "codex for wake-ups (found through {}): {}; `codex queue --help` {}",
                    c.how,
                    c.argv.join(" "),
                    match &probe {
                        Ok(()) => "works".to_string(),
                        Err(e) => format!("fails ({e})"),
                    }
                ),
                probe.is_err().then(|| "set DENDEN_CODEX_BIN to a codex binary that has the queue command".to_string()),
            )
        }
        None => item("note", "no codex binary found for wake-ups (DENDEN_CODEX_BIN, CODEX_HOME's app-server releases, PATH, the Codex app): Codex then gets its messages at its next prompt or Stop hook".into(), None),
    }
}

fn machine_report(with_codex: bool) -> Value {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    let built = crate::time::iso(crate::binary::BUILD_TIME);
    let mut items = vec![item("ok", format!("denden {} built {built}, at {exe}", crate::binary::VERSION), None)];
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(home()) {
            let mode = meta.permissions().mode() & 0o777;
            let h = home();
            items.push(if mode & 0o077 == 0 {
                item("ok", format!("{} is private to your account", tilde(&h)), None)
            } else {
                item("missing", format!("{} is readable by other accounts", tilde(&h)), Some(format!("chmod 700 {}", h.display())))
            });
        }
    }
    // The daemon runs from a copy; it must still be the file it started from (its recorded hash).
    if let Some(rec) = read_json(&home().join("daemon.json")).filter(|_| daemon_info(&home()).is_some_and(|i| is_alive(i.pid))) {
        let s = |k: &str| rec.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let pid = rec.get("pid").and_then(Value::as_u64).unwrap_or(0);
        let runs = format!("the daemon (pid {pid}, denden {}) runs from {}", s("version"), s("exe"));
        items.push(match file_sha256(Path::new(&s("exe"))) {
            Some(h) if h == s("sha256") => item("ok", format!("{runs}, which matches its recorded hash"), None),
            Some(_) => item("warn", format!("{runs}, which no longer matches its recorded hash"), Some("restart it: denden stop, then any denden command starts it again from a verified copy".into())),
            None => item("note", format!("{runs}, which is no longer there"), None),
        });
    } else {
        items.push(item("note", "the daemon is not running (any denden command starts it)".into(), None));
    }
    if with_codex {
        items.push(codex_item());
    }
    json!({ "id": "machine", "name": "This machine", "where": tilde(&home()), "items": items })
}

pub const LIMITS: [&str; 4] = [
    "invitation codes travel through a private channel you choose (Signal, a password manager), never an AI chat",
    "machines of a channel see each other's IP address; DHT nodes see the address of whoever announces",
    "messages enter each conversation's context, so they reach that conversation's AI provider",
    "a leaked invitation code reveals the channel's secret for good: it admits nobody, but shows who is online",
];

pub fn security_report() -> Value {
    let exe = std::env::current_exe().map(|p| p.display().to_string().replace('\\', "/")).unwrap_or_else(|_| "denden".into());
    let cmd = format!("\"{exe}\"");
    let host = detect_host();
    let mut sections: Vec<Value> = [claude_report(&cmd), codex_report(&cmd)].into_iter().flatten().collect();
    sections.sort_by_key(|s| s.get("id").and_then(Value::as_str) != host);
    sections.push(machine_report(true));
    json!({ "host": host, "sections": sections, "limits": LIMITS })
}

pub fn format_report(report: &Value) -> String {
    let mut lines = vec![crate::output::info("denden check")];
    for s in report.get("sections").and_then(Value::as_array).into_iter().flatten() {
        lines.push(String::new());
        lines.push(format!("{} ({})", s["name"].as_str().unwrap_or(""), s["where"].as_str().unwrap_or("")));
        for i in s.get("items").and_then(Value::as_array).into_iter().flatten() {
            let status = i["status"].as_str().unwrap_or("");
            lines.push(format!("  {status:<8} {}", i["what"].as_str().unwrap_or("")));
            if let Some(fix) = i.get("fix").and_then(Value::as_str).filter(|_| status != "ok") {
                lines.push(format!("           -> {fix}"));
            }
        }
    }
    lines.push(String::new());
    lines.push("Always true, whatever the settings:".into());
    lines.extend(LIMITS.iter().map(|l| format!("  - {l}")));
    lines.join("\n")
}
