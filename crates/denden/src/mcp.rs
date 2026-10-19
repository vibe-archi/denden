// `denden mcp`: a stdio MCP server exposing the two Codex hooks as tools, for hooks of type
// "mcp_tool": no shell is needed to run them, on any OS. While it runs (Codex is open), it also
// keeps the daemon up when a Codex conversation holds a denden, so that conversation can be woken.
//
// JSON-RPC 2.0, one message per line on stdin and stdout; logs go to stderr only. tools/call runs on
// its own thread, so a slow call never holds up ping, other calls or notifications/cancelled. A
// cancelled call gets no response (MCP: the receiver should not answer it).

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};

/// Protocol versions we speak, newest first. Tools are all we use, and they behave the same in each.
const PROTOCOLS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Internal to the denden plugin: these tools are called by its Codex hooks (UserPromptSubmit and Stop), never by the agent. Do not call them; use the im skill's commands instead.";

/// An in-flight tools/call, so notifications/cancelled can stop it.
#[derive(Default)]
struct Call {
    cancelled: AtomicBool,
    // The socket of hook_stop's current long wait on the daemon (a new one after a daemon restart):
    // shutting it down ends the wait at once, and the daemon drops the waiter without consuming any
    // message. The very socket the wait reads from, shared: on Windows a duplicate made with
    // try_clone sometimes reports "not connected" to shutdown, and the wait would go on.
    socket: Mutex<Option<Arc<TcpStream>>>,
}

impl Call {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(s) = self.socket.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            let _ = s.shutdown(Shutdown::Both);
        }
    }
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

struct Server {
    out: Mutex<std::io::Stdout>,
    calls: Mutex<HashMap<String, Arc<Call>>>,
}

impl Server {
    fn send(&self, msg: &Value) {
        let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        let _ = writeln!(out, "{msg}");
        let _ = out.flush();
    }
}

fn reply(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn tools() -> Value {
    let output_schema = json!({
        "type": "object",
        "properties": {
            "decision": { "type": "string", "enum": ["block"] },
            "reason": { "type": "string" },
            "hookSpecificOutput": {
                "type": "object",
                "properties": {
                    "hookEventName": { "type": "string" },
                    "additionalContext": { "type": "string" }
                }
            }
        },
        "additionalProperties": false
    });
    json!([
        {
            "name": "hook_prompt",
            "title": "denden UserPromptSubmit hook",
            "description": "Internal to the denden plugin: called by its Codex UserPromptSubmit hook (type mcp_tool), never by the agent. Do not call this tool. It blocks a prompt that carries a denden invitation code. For a conversation that holds a denden, it marks it busy and adds its unread messages as context. It returns {} otherwise.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "hook_event_name": { "type": "string", "description": "Must be UserPromptSubmit; anything else returns {} at once." },
                    "prompt": { "type": "string" },
                    "cwd": { "type": "string" },
                    "session_id": { "type": "string" }
                },
                "additionalProperties": true
            },
            "outputSchema": output_schema
        },
        {
            "name": "hook_stop",
            "title": "denden Stop hook",
            "description": "Internal to the denden plugin: called by its Codex Stop hook (type mcp_tool), never by the agent. Do not call this tool. For a conversation that holds a denden, it marks the turn over and returns a block decision carrying its unread messages when its wake-up budget allows, or {} at once. It returns {} otherwise.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "hook_event_name": { "type": "string", "description": "Must be Stop; anything else returns {} at once." },
                    "cwd": { "type": "string" },
                    "session_id": { "type": "string" },
                    "stop_hook_active": { "type": "boolean" }
                },
                "additionalProperties": true
            },
            "outputSchema": output_schema
        }
    ])
}

/// The hook's JSON, twice: as text and as structured content (hosts read one or the other).
fn tool_result(decision: Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": decision.to_string() }],
        "structuredContent": decision,
        "isError": false
    })
}

fn run_tool(name: &str, args: &Map<String, Value>, _call: &Call) -> Value {
    let event = args.get("hook_event_name").and_then(Value::as_str).unwrap_or("");
    match name {
        "hook_prompt" if event == "UserPromptSubmit" => crate::hook_prompt_decision(&Value::Object(args.clone()).to_string(), "codex"),
        "hook_stop" if event == "Stop" => crate::hook_stop_decision(&Value::Object(args.clone())),
        // A call for another event (a mistaken call by the model, say) must not wait for anything.
        _ => json!({}),
    }
}

/// Handles one message. Returns the response to send now, if any; tools/call answers later from
/// its own thread (`wait` makes it answer in place, for batches).
fn handle(server: &Arc<Server>, msg: Value, wait: bool) -> Option<Value> {
    let Value::Object(msg) = msg else {
        return Some(error(&Value::Null, -32600, "Invalid Request"));
    };
    let id = msg.get("id").cloned();
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        // A response to a request of ours (we send none), or garbage.
        return id.filter(|_| !msg.contains_key("result") && !msg.contains_key("error")).map(|id| error(&id, -32600, "Invalid Request"));
    };
    let params = msg.get("params").cloned().unwrap_or(json!({}));
    let Some(id) = id else {
        // Notifications: nothing to answer.
        if method == "notifications/cancelled" {
            if let Some(req) = params.get("requestId") {
                if let Some(call) = server.calls.lock().unwrap_or_else(|e| e.into_inner()).get(&req.to_string()) {
                    call.cancel();
                }
            }
        }
        return None;
    };
    match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
            let version = PROTOCOLS.iter().find(|v| **v == asked).copied().unwrap_or(PROTOCOLS[0]);
            Some(reply(
                &id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "denden", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": INSTRUCTIONS
                }),
            ))
        }
        "ping" => Some(reply(&id, json!({}))),
        "tools/list" => Some(reply(&id, json!({ "tools": tools() }))),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str).map(str::to_string) else {
                return Some(error(&id, -32602, "Invalid params: tools/call needs a tool name"));
            };
            if name != "hook_prompt" && name != "hook_stop" {
                return Some(error(&id, -32602, &format!("Unknown tool: {name}")));
            }
            let args = match params.get("arguments") {
                None | Some(Value::Null) => Map::new(),
                Some(Value::Object(a)) => a.clone(),
                Some(_) => return Some(error(&id, -32602, "Invalid params: arguments must be an object")),
            };
            let call = Arc::new(Call::default());
            let key = id.to_string();
            server.calls.lock().unwrap_or_else(|e| e.into_inner()).insert(key.clone(), call.clone());
            let srv = server.clone();
            let worker = std::thread::spawn(move || {
                let result = run_tool(&name, &args, &call);
                srv.calls.lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
                (!call.is_cancelled()).then(|| reply(&id, tool_result(result)))
            });
            if wait {
                return worker.join().ok().flatten();
            }
            let srv = server.clone();
            std::thread::spawn(move || {
                if let Ok(Some(response)) = worker.join() {
                    srv.send(&response);
                }
            });
            None
        }
        _ => Some(error(&id, -32601, &format!("Method not found: {method}"))),
    }
}

pub fn serve() -> ! {
    // Hosts start us in the plugin's folder (Codex: "cwd": "."), and on Windows a process keeps its
    // current directory locked, which would block updating or removing the plugin while a session
    // runs. Nothing here depends on the current directory: paths come from DENDEN_HOME (made absolute in
    // main) and from the executable, and hook_stop takes its folder from the hook payload.
    if let Err(e) = std::env::set_current_dir(crate::core::neutral_dir()) {
        eprintln!("denden mcp: could not leave the start folder: {e}");
    }
    std::thread::spawn(presence);
    let server = Arc::new(Server { out: Mutex::new(std::io::stdout()), calls: Mutex::new(HashMap::new()) });
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("denden mcp: unreadable message: {e}");
                server.send(&error(&Value::Null, -32700, "Parse error"));
                continue;
            }
        };
        match msg {
            // JSON-RPC batches (MCP 2025-03-26): answered together, off the reading thread.
            Value::Array(items) if !items.is_empty() => {
                let srv = server.clone();
                std::thread::spawn(move || {
                    let responses: Vec<Value> = items.into_iter().filter_map(|m| handle(&srv, m, true)).collect();
                    if !responses.is_empty() {
                        srv.send(&Value::Array(responses));
                    }
                });
            }
            Value::Array(_) => server.send(&error(&Value::Null, -32600, "Invalid Request")),
            other => {
                if let Some(response) = handle(&server, other, false) {
                    server.send(&response);
                }
            }
        }
    }
    // stdin closed: the host is gone. Abandon any call still waiting and exit.
    for call in server.calls.lock().unwrap_or_else(|e| e.into_inner()).values() {
        call.cancel();
    }
    std::process::exit(0)
}

/// While Codex runs (and this server with it), the daemon stays up as long as a Codex conversation
/// holds a denden: it is what wakes that conversation. Checked every minute, reconnected when the
/// daemon restarts.
fn presence() {
    loop {
        let wanted = crate::conv::Convs::load(&crate::core::conversations_file()).attached().any(|c| c.app() == "codex");
        if wanted && crate::try_ensure_daemon().is_ok() {
            if let Some(info) = crate::core::daemon_info(&crate::core::home()) {
                // Returns when the daemon goes away (or never answers).
                let _ = crate::http::stream(info.port, "/presence", &info.token, |_| {});
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}
