// The denden daemon: one per machine. It holds every channel this machine is in (net.rs keeps their
// swarm connections), routes messages between denden, here and on other machines, hands them to
// the conversations they are attached to, wakes those conversations (wake.rs), and serves the CLI
// on a localhost HTTP API guarded by a random token (daemon.json).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use peeroxide::SwarmHandle;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{Notify, mpsc, oneshot};

use crate::binary;
use crate::codex;
use crate::conv::{Conv, Convs};
use crate::core::*;
use crate::http;
use crate::net;
use crate::proto::*;
use crate::store::{self, Channel, OUTBOX_TTL};
use crate::time::iso;
use crate::wake::{self, Decision, Unread, WakeState};

/// Without a listener, an MCP server or any command for this long (and no keepalive), the daemon
/// stops: nobody on this machine would read what arrives.
const IDLE_STOP: u64 = 10 * 60 * 1000;

pub fn log_line(s: &str) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{} {s}", iso(now_ms()));
    let _ = out.flush();
}

#[macro_export]
macro_rules! dlog {
    ($($a:tt)*) => { $crate::daemon::log_line(&format!($($a)*)) };
}
use crate::dlog as log;

// ---------- state ----------

pub struct Live {
    pub cid: String,
    /// The machine key, once its hello is accepted.
    pub key: Option<String>,
    pub send: mpsc::UnboundedSender<String>,
    pub kill: Arc<Notify>,
    pub epoch: u64,
    pub local_t: [u8; 32],
    pub remote_t: [u8; 32],
    pub authed: bool,
    pub window_start: u64,
    pub frame_count: u32,
}

pub struct SwarmSlot {
    pub generation: u64,
    pub handle: Option<SwarmHandle>,
}

/// A channel and its live network state.
pub struct Chan {
    pub ch: Channel,
    pub swarms: BTreeMap<u64, SwarmSlot>,
    pub announced: bool,
    /// Machines online: key -> connection.
    pub peers: HashMap<String, u64>,
    pub banned_t: HashSet<String>,
    /// Transport keys that refused us (not admitted yet), per epoch: dialled again once we are.
    pub refused_by: HashMap<String, u64>,
    pub buckets: HashMap<String, (f64, u64)>,
    /// The last refusal we got, and when: shown while this machine waits to be admitted.
    pub last_refusal: Option<(String, u64)>,
}

pub struct Watcher {
    pub id: u64,
    pub conv: String,
    pub tx: mpsc::UnboundedSender<String>,
}

pub enum AckResult {
    Ok,
    Refused(String),
}

pub struct State {
    pub chans: BTreeMap<String, Chan>,
    pub convs: Convs,
    pub wakes: HashMap<String, WakeState>,
    pub keepalive: bool,
    pub live: BTreeMap<u64, Live>,
    pub watchers: Vec<Watcher>,
    pub ack_waiters: HashMap<String, oneshot::Sender<AckResult>>,
    pub next_id: u64,
    pub stopping: bool,
    pub started_at: u64,
    pub pid: u32,
    /// MCP servers connected (Codex is open on this machine).
    pub presence: usize,
    pub last_active: u64,
}

pub type Shared = Arc<Mutex<State>>;

/// What frame handling asks for once the state lock is released.
pub enum Effect {
    Kill(Arc<Notify>),
    KillLater(Arc<Notify>, u64),
    Ban(SwarmHandle, [u8; 32]),
    Redial(SwarmHandle, [u8; 32]),
}

pub fn run_effects(fx: Vec<Effect>) {
    for e in fx {
        match e {
            Effect::Kill(k) => k.notify_one(),
            Effect::KillLater(k, ms) => {
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    k.notify_one();
                });
            }
            Effect::Ban(h, key) => {
                tokio::spawn(async move {
                    let _ = h.ban_peer(key).await;
                });
            }
            Effect::Redial(h, key) => {
                tokio::spawn(async move {
                    let _ = h.join_peer(key).await;
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    let _ = h.leave_peer(key).await;
                });
            }
        }
    }
}

pub fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}
fn u(v: &Value, k: &str) -> u64 {
    v.get(k).and_then(Value::as_u64).unwrap_or(0)
}
fn seq_of(m: &Value) -> u64 {
    u(m, "seq")
}

impl State {
    pub fn save_convs(&self) {
        self.convs.save(&conversations_file());
    }
    pub fn save_wakes(&self) {
        let m: Map<String, Value> = self.wakes.iter().map(|(k, w)| (k.clone(), w.to_json())).collect();
        let _ = write_json(&home().join("wakes.json"), &Value::Object(m));
    }
    /// The channel and denden a conversation holds.
    pub fn held(&self, conv: &str) -> Option<(String, String)> {
        self.convs.get(conv).filter(|c| c.attached() && self.chans.contains_key(&c.cid)).map(|c| (c.cid.clone(), c.denden.clone()))
    }
    pub fn listening(&self, conv: &str) -> bool {
        self.watchers.iter().any(|w| w.conv == conv)
    }
    pub fn chan_by_label(&self, label: &str) -> Option<&Chan> {
        self.chans.values().find(|c| c.ch.label() == label)
    }
    pub fn cid_by_label(&self, label: &str) -> Option<String> {
        self.chan_by_label(label).map(|c| c.ch.cid.clone())
    }

    /// A notice from denden to the conversation of our denden `d` (it goes through like a message).
    /// `quiet` ones never wake: they wait for the next hand-over.
    pub fn note(&mut self, cid: &str, d: &str, text: &str, alert: bool, quiet: bool) {
        let Some(c) = self.chans.get_mut(cid) else { return };
        let mut m = Map::new();
        m.insert("id".into(), json!(random_hex(6)));
        m.insert("dir".into(), json!("note"));
        m.insert("d".into(), json!(d));
        m.insert("text".into(), json!(text));
        m.insert("at".into(), json!(now_ms()));
        if alert {
            m.insert("alert".into(), json!(true));
        }
        if quiet {
            m.insert("quiet".into(), json!(true));
        }
        c.ch.record(m);
    }
    /// The same notice to every attached denden of a channel.
    pub fn note_all(&mut self, cid: &str, text: &str, alert: bool, quiet: bool) {
        let dendens: Vec<String> = self.convs.attached().filter(|c| c.cid == cid).map(|c| c.denden.clone()).collect();
        for d in dendens {
            self.note(cid, &d, text, alert, quiet);
        }
    }

    /// Signs and stores a new state of one of our names, and tells the machines online.
    pub fn update_name(&mut self, cid: &str, n: &str, change: impl FnOnce(&mut NameCert)) {
        let now = now_ms();
        let Some(c) = self.chans.get_mut(cid) else { return };
        let mut cert = c.ch.names.get(n).filter(|x| x.k == c.ch.me).cloned().unwrap_or_else(|| NameCert::new(n, &c.ch.me, now));
        change(&mut cert);
        let cert = c.ch.sign_name(cert, now.max(c.ch.names.get(n).map(|x| x.u + 1).unwrap_or(0)));
        let me = c.ch.me.clone();
        c.ch.names.offer(cert.clone(), &me);
        c.ch.save_names();
        net::broadcast(self, cid, &json!({ "t": "names", "names": [cert.to_json()] }));
    }
}

// ---------- hand-over to conversations ----------

/// A record as a conversation gets it.
fn item(c: &Chan, m: &Value, now: u64) -> Value {
    let kind = if s(m, "dir") == "note" { "note" } else { "msg" };
    let mut v = json!({
        "seq": m.get("seq"), "id": m.get("id"), "kind": kind, "text": m.get("text"),
        "at": m.get("at"), "ago": crate::time::ago(now, u(m, "at")),
    });
    if kind == "msg" {
        v["from"] = json!(s(m, "from"));
        v["to"] = m.get("to").cloned().unwrap_or(Value::Null);
        v["ask"] = json!(truthy(m.get("ask")));
        v["late"] = json!(is_late(u(m, "waited"), u(m, "at"), now));
        if let Some(re) = m.get("re").and_then(Value::as_str) {
            v["re"] = json!(re);
            if let Some(q) = c.ch.log.by_id(re) {
                v["reText"] = json!(s(q, "text").chars().take(120).collect::<String>());
            }
        }
    } else if truthy(m.get("alert")) {
        v["alert"] = json!(true);
    }
    v
}

/// The unread records of a conversation's denden, as items.
fn unread_items(st: &State, conv: &str) -> Vec<Value> {
    let Some((cid, d)) = st.held(conv) else { return Vec::new() };
    let c = &st.chans[&cid];
    let now = now_ms();
    c.ch.log.unread(&d).into_iter().map(|m| item(c, m, now)).collect()
}

/// Hands every unread record to the conversation and marks it read (unless `peek`).
fn take_unread(st: &mut State, conv: &str, peek: bool) -> Vec<Value> {
    let items = unread_items(st, conv);
    if !peek && !items.is_empty() {
        mark_read(st, conv, &items.iter().map(seq_of).collect::<Vec<_>>());
    }
    items
}

fn mark_read(st: &mut State, conv: &str, seqs: &[u64]) {
    let Some((cid, d)) = st.held(conv) else { return };
    if let Some(c) = st.chans.get_mut(&cid) {
        c.ch.mark_read(&d, seqs);
    }
    if let Some(&max) = seqs.iter().max() {
        st.wakes.entry(conv.to_string()).or_default().handed(max);
    }
}

fn unread_for_wake(st: &State, conv: &str) -> Vec<Unread> {
    let Some((cid, d)) = st.held(conv) else { return Vec::new() };
    st.chans[&cid].ch.log.unread(&d).into_iter().filter(|m| !truthy(m.get("quiet"))).map(|m| Unread { seq: seq_of(m), at: u(m, "at"), from: s(m, "from").to_string() }).collect()
}

const SPENT: &str = "wake-up limit reached";

/// The wake-up budget of a conversation is spent: it is told once, with how to raise it.
fn spent_text(c: &Conv) -> String {
    format!(
        "Limit of {} wake-ups per hour reached: the next messages will wait for your next message here · to raise it: /denden:im wakes {}, or /denden:im wakes off to remove it",
        c.wakes,
        (c.wakes * 2).max(10)
    )
}

/// New records for Claude listeners, within each conversation's wake-up budget. Called after
/// anything is recorded, and every second (for the debounce).
pub fn notify(st: &mut State) {
    let now = now_ms();
    let convs: Vec<String> = st.watchers.iter().map(|w| w.conv.clone()).collect::<HashSet<_>>().into_iter().collect();
    for conv in convs {
        let Some(c) = st.convs.get(&conv).cloned() else { continue };
        let unread = unread_for_wake(st, &conv);
        let decision = st.wakes.entry(conv.clone()).or_default().decide(now, &unread, c.wakes, false);
        let lines: Vec<String> = match decision {
            Decision::Wake { upto, .. } => {
                // Everything unread goes, quiet notices included: the agent is awake anyway. Marked
                // read once the listener has written them out (POST /read).
                let items: Vec<Value> = unread_items(st, &conv).into_iter().filter(|i| seq_of(i) <= upto || i.get("kind") == Some(&json!("note"))).collect();
                let upto = items.iter().map(seq_of).max().unwrap_or(upto);
                st.wakes.entry(conv.clone()).or_default().handed(upto);
                items.iter().map(|i| i.to_string()).collect()
            }
            Decision::Spent { first: true } => vec![json!({ "kind": "note", "text": spent_text(&c), "spent": true }).to_string()],
            _ => continue,
        };
        st.watchers.retain_mut(|w| {
            if w.conv != conv {
                return true;
            }
            lines.iter().all(|l| w.tx.send(format!("{l}\n")).is_ok())
        });
        if matches!(decision, Decision::Spent { first: true }) {
            log!("{conv}: {SPENT}");
        }
    }
}

/// Codex conversations with unread messages, idle and within budget: queue a notice.
fn codex_tick(shared: &Shared) {
    let jobs = {
        let mut st = shared.lock().unwrap();
        if st.stopping {
            return;
        }
        let now = now_ms();
        let convs: Vec<Conv> = st.convs.attached().filter(|c| c.app() == "codex").cloned().collect();
        let mut jobs = Vec::new();
        for c in convs {
            let unread = unread_for_wake(&st, &c.key);
            let late = unread_items(&st, &c.key).iter().filter(|i| truthy(i.get("late"))).count();
            let w = st.wakes.entry(c.key.clone()).or_default();
            let prev = w.notified;
            match w.decide(now, &unread, c.wakes, true) {
                Decision::Wake { n, senders, .. } => match w.codex.clone().or_else(|| codex::find_codex().map(|b| b.argv)) {
                    Some(argv) => jobs.push((c.key.clone(), c.id().to_string(), argv, codex::notice(n, &senders, late), prev)),
                    None => {
                        w.queued(&Err("no codex binary found".into()), prev);
                        log!("codex: cannot wake {}: no codex binary found (DENDEN_CODEX_BIN, the Codex app, PATH)", c.key);
                    }
                },
                Decision::Spent { first: true } => {
                    let (cid, d, text) = (c.cid.clone(), c.denden.clone(), spent_text(&c));
                    st.note(&cid, &d, &text, false, true);
                    log!("{}: {SPENT}", c.key);
                }
                _ => {}
            }
        }
        if !jobs.is_empty() {
            st.save_wakes();
        }
        jobs
    };
    for (key, thread, argv, notice, prev) in jobs {
        let sh = shared.clone();
        tokio::spawn(async move {
            log!("codex: queueing a notice for {key}");
            let t = thread.clone();
            let r = tokio::task::spawn_blocking(move || codex::run(&argv, &["queue", "--thread", &t, "--message", &notice], Duration::from_secs(30)))
                .await
                .unwrap_or_else(|e| Err(e.to_string()));
            let mut st = sh.lock().unwrap();
            let w = st.wakes.entry(key.clone()).or_default();
            w.queued(&r, prev);
            match &r {
                Ok(()) => log!("codex: notice queued for {key}"),
                // Codex does not know that conversation any more: it is not woken again (its hooks,
                // should it come back, start the wake-ups again).
                Err(e) if codex::thread_gone(e) => {
                    w.failures = wake::MAX_FAILURES;
                    log!("codex: {key} no longer exists for Codex: not woken any more")
                }
                Err(e) => log!("codex: queueing for {key} failed: {e}"),
            }
            st.save_wakes();
        });
    }
}

// ---------- messages ----------

/// Splits what the user typed as recipients: one name, a comma list, or `all`.
fn recipients(to: &str) -> Result<Option<Vec<String>>, String> {
    let to = to.trim().trim_start_matches('@').to_lowercase();
    if is_broadcast(&to) {
        return Ok(None);
    }
    let mut list: Vec<String> = Vec::new();
    for n in to.split([',', ' ']).map(|n| n.trim().trim_start_matches('@')).filter(|n| !n.is_empty()) {
        if is_broadcast(n) {
            return Err("`all` goes alone: it already means the whole channel".into());
        }
        if !valid_name(n) {
            return Err(format!("\"{n}\" is not a denden name"));
        }
        if !list.iter().any(|x| x == n) {
            list.push(n.to_string());
        }
    }
    if list.is_empty() {
        return Err("no recipient".into());
    }
    Ok(Some(list))
}

/// Sends from the denden of `conv`. Returns what was delivered, queued or refused.
async fn send(shared: &Shared, b: &Value) -> Result<Value, String> {
    let conv = s(b, "conv").to_string();
    let text = s(b, "text").trim_end().to_string();
    if text.is_empty() {
        return Err("empty message".into());
    }
    if js_len(&text) > MAX_TEXT {
        return Err(format!("message too long (max {MAX_TEXT} characters)"));
    }
    let secrets = find_secrets(&text);
    if !secrets.is_empty() && !truthy(b.get("force")) {
        return Err(format!("blocked: the message seems to contain {}. Remove it; resend with --force only if your user explicitly approved sending it.", secrets.join(", ")));
    }
    let id = random_hex(6);
    let (waits, out) = {
        let mut st = shared.lock().unwrap();
        st.last_active = now_ms();
        let Some((cid, from)) = st.held(&conv) else { return Err("this conversation is not attached to a denden: /denden:im join <channel> as <name>".into()) };
        {
            let c = &st.chans[&cid];
            if c.ch.names.owner(&from) != Some(c.ch.me.as_str()) {
                return Err(format!("{from} belongs to another machine now: choose another name with /denden:im join {} as <name>", c.ch.label()));
            }
        }
        let mut ask = truthy(b.get("ask"));
        let mut re: Option<String> = None;
        let to: Option<Vec<String>> = if let Some(rid) = b.get("re").and_then(Value::as_str) {
            // A reply: to whoever asked, once, and only when they asked for an answer.
            let c = &st.chans[&cid];
            let q = c.ch.log.messages.iter().rev().find(|m| s(m, "id") == rid && s(m, "dir") == "in" && s(m, "d") == from).cloned();
            let Some(q) = q else { return Err(format!("no message #{rid} received by {from}")) };
            if !truthy(q.get("ask")) {
                return Err(format!("#{rid} did not ask for a reply: it needs none"));
            }
            if c.ch.log.messages.iter().any(|m| s(m, "dir") == "out" && s(m, "re") == rid) {
                return Err(format!("#{rid} has been answered already"));
            }
            ask = false;
            re = Some(rid.to_string());
            Some(vec![s(&q, "from").to_string()])
        } else {
            recipients(s(b, "to"))?
        };
        let c = st.chans.get(&cid).unwrap();
        let me = c.ch.me.clone();
        // Who gets it, by machine.
        let mut by_machine: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut local: Vec<String> = Vec::new();
        match &to {
            None => {
                local = c.ch.my_names().into_iter().filter(|n| *n != from).collect();
                for k in c.ch.members.machines.keys().filter(|k| **k != me && !c.ch.members.blocked.contains_key(*k)) {
                    by_machine.insert(k.clone(), Vec::new());
                }
            }
            Some(list) => {
                for n in list {
                    if *n == from {
                        return Err("that is you".into());
                    }
                    match c.ch.names.get(n) {
                        None => return Err(format!("no denden called {n} in {}", c.ch.label())),
                        Some(nc) if !nc.active() => return Err(format!("{n} was deleted")),
                        Some(nc) if nc.k == me => local.push(n.clone()),
                        Some(nc) if c.ch.members.blocked.contains_key(&nc.k) => return Err(format!("{n} is on a machine you blocked")),
                        Some(nc) => by_machine.entry(nc.k.clone()).or_default().push(n.clone()),
                    }
                }
            }
        }
        let now = now_ms();
        let to_v = match &to {
            None => json!("all"),
            Some(l) => json!(l),
        };
        let mut rec = Map::new();
        for (k, v) in [("id", json!(id)), ("dir", json!("out")), ("d", json!(from)), ("to", to_v.clone()), ("text", json!(text)), ("ts", json!(now)), ("at", json!(now))] {
            rec.insert(k.into(), v);
        }
        if ask {
            rec.insert("ask".into(), json!(true));
        }
        if let Some(r) = &re {
            rec.insert("re".into(), json!(r));
        }
        let c = st.chans.get_mut(&cid).unwrap();
        c.ch.record(rec);
        // Here: straight into the other denden's log.
        for n in &local {
            let mut m = Map::new();
            for (k, v) in [("id", json!(id)), ("dir", json!("in")), ("d", json!(n)), ("from", json!(from)), ("to", to_v.clone()), ("text", json!(text)), ("ts", json!(now)), ("at", json!(now)), ("waited", json!(0))] {
                m.insert(k.into(), v);
            }
            if ask {
                m.insert("ask".into(), json!(true));
            }
            if let Some(r) = &re {
                m.insert("re".into(), json!(r));
            }
            c.ch.record(m);
        }
        // Elsewhere: one entry per machine, kept until it acknowledges.
        let mut entries = Vec::new();
        for (k, names) in &by_machine {
            let mut e = json!({ "id": id, "from": from, "to": if to.is_none() { json!("all") } else { json!(names) }, "text": text, "ts": now, "mk": k, "q": now });
            if ask {
                e["ask"] = json!(true);
            }
            if let Some(r) = &re {
                e["re"] = json!(r);
            }
            c.ch.outbox.push(e.clone());
            entries.push((k.clone(), names.clone(), e, c.peers.get(k).copied()));
        }
        c.ch.save_outbox();
        let mut waits = Vec::new();
        for (k, names, e, conn) in entries {
            match conn {
                Some(conn) => {
                    let (tx, rx) = oneshot::channel();
                    st.ack_waiters.insert(format!("{id}:{k}"), tx);
                    net::transmit(&st, conn, &e, now);
                    waits.push((k, names, Some(rx)));
                }
                None => waits.push((k, names, None)),
            }
        }
        notify(&mut st);
        let names_of = |k: &str| st.chans[&cid].ch.members.display(k);
        let waits: Vec<(String, String, Vec<String>, Option<oneshot::Receiver<AckResult>>)> = waits.into_iter().map(|(k, n, rx)| (names_of(&k), k, n, rx)).collect();
        // For the whole channel: how many denden it reaches.
        let count = st.chans[&cid].ch.names.map.values().filter(|n| n.active() && n.n != from && !st.chans[&cid].ch.members.blocked.contains_key(&n.k)).count();
        (waits, json!({ "id": id, "from": from, "to": to_v, "local": local, "ask": ask, "re": re, "count": count }))
    };
    let mut delivered: Vec<Value> = Vec::new();
    let mut queued: Vec<Value> = Vec::new();
    let mut refused: Vec<Value> = Vec::new();
    for (machine, k, names, rx) in waits {
        let r = match rx {
            Some(rx) => tokio::time::timeout(Duration::from_secs(5), rx).await.ok().and_then(Result::ok),
            None => None,
        };
        shared.lock().unwrap().ack_waiters.remove(&format!("{id}:{k}"));
        let entry = json!({ "machine": machine, "names": names });
        match r {
            Some(AckResult::Ok) => delivered.push(entry),
            Some(AckResult::Refused(e)) => refused.push(json!({ "machine": machine, "names": names, "error": e })),
            None => queued.push(entry),
        }
    }
    let mut out = out;
    out["delivered"] = Value::Array(delivered);
    out["queued"] = Value::Array(queued);
    out["refused"] = Value::Array(refused);
    Ok(out)
}

/// Drops outbox entries older than 7 days and tells their senders.
fn expire_outbox(st: &mut State) {
    let now = now_ms();
    let mut notes = Vec::new();
    for (cid, c) in st.chans.iter_mut() {
        let (old, keep): (Vec<Value>, Vec<Value>) = std::mem::take(&mut c.ch.outbox).into_iter().partition(|e| now.saturating_sub(u(e, "q")) >= OUTBOX_TTL);
        c.ch.outbox = keep;
        if old.is_empty() {
            continue;
        }
        c.ch.save_outbox();
        for e in old {
            let who = match e.get("to") {
                Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "),
                _ => c.ch.members.display(s(&e, "mk")),
            };
            let text: String = s(&e, "text").chars().take(80).collect();
            notes.push((cid.clone(), s(&e, "from").to_string(), format!("Message not delivered to {who} after 7 days, dropped: \"{text}\"")));
        }
    }
    for (cid, d, text) in notes {
        st.note(&cid, &d, &text, false, true);
    }
}

// ---------- channels and denden ----------

fn api_err(msg: impl Into<String>) -> Value {
    json!({ "error": msg.into() })
}
fn need(what: &str, msg: impl Into<String>, extra: Value) -> Value {
    let mut v = json!({ "error": msg.into(), "need": what });
    if let Value::Object(m) = extra {
        for (k, x) in m {
            v[k] = x;
        }
    }
    v
}

/// The channel status a conversation sees.
fn conv_status(st: &State, conv: &str) -> Value {
    let Some(c) = st.convs.get(conv) else { return json!({ "attached": false }) };
    let Some((cid, d)) = st.held(conv) else { return json!({ "attached": false, "auto": c.auto, "wakes": c.wakes }) };
    let ch = &st.chans[&cid];
    let online: Vec<String> = ch.ch.names.map.values().filter(|n| n.active() && n.n != d && (n.k == ch.ch.me || ch.peers.contains_key(&n.k)) && n.on).map(|n| n.n.clone()).collect();
    json!({
        "attached": true, "label": ch.ch.label(), "denden": d, "machine": ch.ch.machine(), "member": ch.ch.is_member(),
        "auto": c.auto, "wakes": c.wakes, "online": online, "unread": ch.ch.log.unread(&d).len(),
        "role": ch.ch.names.get(&d).map(|n| n.role.clone()).unwrap_or_default(), "listening": st.listening(conv),
    })
}

fn start_channel(shared: &Shared, ch: Channel) {
    let cid = ch.cid.clone();
    {
        let mut st = shared.lock().unwrap();
        st.chans.insert(cid.clone(), Chan { ch, swarms: BTreeMap::new(), announced: false, peers: HashMap::new(), banned_t: HashSet::new(), refused_by: HashMap::new(), buckets: HashMap::new(), last_refusal: None });
    }
    net::reconcile(shared);
}

fn create(shared: &Shared, b: &Value) -> Value {
    let label = s(b, "label").to_lowercase();
    if !valid_name(&label) {
        return api_err(format!("\"{label}\" is not a valid channel name (a-z 0-9 . _ -, up to 32)"));
    }
    if shared.lock().unwrap().cid_by_label(&label).is_some() {
        return need("label", format!("this machine already has a channel called {label}"), json!({ "label": label }));
    }
    let conv = s(b, "conv").to_string();
    let name = s(b, "name").to_lowercase();
    if !name.is_empty() && (!valid_name(&name) || is_broadcast(&name)) {
        return api_err(format!("\"{name}\" is not a valid denden name (a-z 0-9 . _ -, up to 32, not \"all\")"));
    }
    let ch = match Channel::create(&label, now_ms()) {
        Ok(ch) => ch,
        Err(e) => return api_err(e),
    };
    let machine = ch.machine();
    log!("channel {label} created");
    start_channel(shared, ch);
    let mut out = json!({ "label": label, "machine": machine });
    if !conv.is_empty() && !name.is_empty() {
        let mut ab = b.clone();
        ab["label"] = json!(label);
        out["attach"] = attach(shared, &ab);
    }
    out
}

fn join(shared: &Shared, b: &Value) -> Value {
    let inv = match decode_invite(s(b, "code")) {
        Ok(i) => i,
        Err(e) => return api_err(e),
    };
    let now = now_ms();
    if inv.expires > 0 && inv.expires < now {
        return need("code", "This invitation code has expired · ask for a new one: /denden:im invite on a machine of the channel", json!({ "expired": true }));
    }
    let st = shared.lock().unwrap();
    let cid = store::cid_of(&inv.secret);
    if let Some(c) = st.chans.get(&cid) {
        return json!({ "label": c.ch.label(), "already": true });
    }
    let label = Some(s(b, "label").to_lowercase()).filter(|l| !l.is_empty()).or_else(|| inv.label.clone()).unwrap_or_else(|| default_label(&inv.secret));
    if !valid_name(&label) {
        return api_err(format!("\"{label}\" is not a valid channel name"));
    }
    if st.cid_by_label(&label).is_some() {
        return need("label", format!("You already have a channel called {label} on this machine"), json!({ "label": label }));
    }
    drop(st);
    let ch = match Channel::join(&inv, &label, now) {
        Ok(ch) => ch,
        Err(e) => return api_err(e),
    };
    log!("joining {label}, invited by {}", inv.inviter);
    start_channel(shared, ch);
    json!({ "label": label, "inviter": inv.inviter, "inviterFp": fmt_fp(&fingerprint_hex(&inv.inviter_key)[..8]) })
}

/// Attaches a denden to a conversation (creating the name if it is new to this machine).
fn attach(shared: &Shared, b: &Value) -> Value {
    let conv = s(b, "conv").to_string();
    if !crate::conv::valid_key(&conv) {
        return api_err("run this from a Claude or Codex conversation");
    }
    let mut st = shared.lock().unwrap();
    st.last_active = now_ms();
    let label = s(b, "label").to_lowercase();
    let Some(cid) = st.cid_by_label(&label) else {
        return need("code", format!("this machine is not in a channel called {label} yet"), json!({ "label": label }));
    };
    {
        let c = &st.chans[&cid];
        if !c.ch.is_member() {
            return need("admission", format!("this machine is waiting to be admitted to {label}: the machine that invited it must be online"), json!({}));
        }
    }
    let name = s(b, "name").to_lowercase();
    if name.is_empty() {
        let suggest = clean_name(Path::new(s(b, "cwd")).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default().as_str()).unwrap_or_default();
        return need("name", "Which name for the denden of this conversation?", json!({ "suggest": suggest }));
    }
    if !valid_name(&name) || is_broadcast(&name) {
        return api_err(format!("\"{name}\" is not a valid denden name (a-z 0-9 . _ -, up to 32, not \"all\")"));
    }
    if st.chans[&cid].ch.name_taken(&name) {
        return need("other-name", format!("{name} already exists in {label}, on another machine. Which other name for this conversation?"), json!({ "taken": name }));
    }
    let now = now_ms();
    // Held by another conversation here: moved only when the user says so, unless that one no
    // longer listens (a Claude conversation whose listener is gone).
    if let Some(h) = st.convs.holder(&cid, &name).filter(|h| h.key != conv).cloned() {
        let gone = h.app() == "claude" && !st.listening(&h.key);
        if !gone && !truthy(b.get("move")) {
            return need("move", format!("{name} is attached to {}. Move it here, or choose another name for this conversation?", h.describe(now)), json!({ "holder": h.key }));
        }
    }
    // This conversation leaves the denden it held.
    if let Some((old_cid, old_d)) = st.held(&conv).filter(|(c, d)| !(*c == cid && *d == name)) {
        st.convs.detach(&conv);
        st.update_name(&old_cid, &old_d, |n| n.on = false);
    }
    let app = conv.split(':').next().unwrap_or("").to_string();
    let previous = st.convs.attach(&conv, &cid, &name, now);
    {
        let c = st.convs.entry(&conv, now);
        c.folder = s(b, "folder").to_string();
        c.cwd = s(b, "cwd").to_string();
        if !s(b, "title").is_empty() {
            c.title = s(b, "title").to_string();
        }
    }
    let fresh = st.chans[&cid].ch.names.get(&name).is_none_or(|n| !n.active());
    st.update_name(&cid, &name, |n| {
        n.on = true;
        n.d = false;
        n.app = app.clone();
    });
    if let Some(w) = st.wakes.get_mut(&conv) {
        w.notified = 0;
    }
    let mut moved = Value::Null;
    if let Some(p) = &previous {
        moved = json!(p.describe(now));
        let here = st.convs.get(&conv).map(|c| c.describe(now)).unwrap_or_default();
        // Its listener, if any, is told and stops.
        let line = json!({ "kind": "note", "text": format!("{name} was moved to {here} · this conversation is no longer attached to {label}"), "detached": true }).to_string();
        st.watchers.retain(|w| {
            if w.conv == p.key {
                let _ = w.tx.send(format!("{line}\n"));
                false
            } else {
                true
            }
        });
        log!("{name}@{label} moved from {} to {conv}", p.key);
    } else {
        log!("{name}@{label} attached to {conv}{}", if fresh { " (new name)" } else { "" });
    }
    if let Some(argv) = codex::argv_from_request(b.get("codex")) {
        let w = st.wakes.entry(conv.clone()).or_default();
        w.codex = Some(argv);
        w.codex_home = b.get("codexHome").and_then(Value::as_str).map(str::to_string);
    }
    st.save_convs();
    st.save_wakes();
    let mut out = conv_status(&st, &conv);
    out["moved"] = moved;
    out["new"] = json!(fresh);
    out
}

fn detach(st: &mut State, b: &Value) -> Value {
    let conv = s(b, "conv");
    let Some((cid, d)) = st.held(conv) else { return api_err("this conversation is not attached to a denden") };
    st.convs.detach(conv);
    st.update_name(&cid, &d, |n| n.on = false);
    st.watchers.retain(|w| w.conv != conv);
    st.save_convs();
    let label = st.chans[&cid].ch.label();
    log!("{d}@{label} detached from {conv}");
    json!({ "denden": d, "label": label })
}

fn delete(st: &mut State, b: &Value) -> Value {
    let name = s(b, "name").to_lowercase();
    let cid = match st.held(s(b, "conv")).map(|(c, _)| c).or_else(|| st.cid_by_label(s(b, "label"))) {
        Some(c) => c,
        None => return api_err("which channel? run this from a conversation attached to it"),
    };
    let c = &st.chans[&cid];
    if c.ch.names.get(&name).is_none_or(|n| n.k != c.ch.me || !n.active()) {
        return api_err(format!("no denden called {name} on this machine in {}", c.ch.label()));
    }
    let label = c.ch.label();
    if let Some(key) = st.convs.release(&cid, &name) {
        st.watchers.retain(|w| w.conv != key);
    }
    st.update_name(&cid, &name, |n| {
        n.d = true;
        n.on = false;
    });
    st.save_convs();
    log!("{name}@{label} deleted");
    json!({ "denden": name, "label": label })
}

fn invite(st: &mut State, b: &Value) -> Value {
    let cid = match st.held(s(b, "conv")).map(|(c, _)| c).or_else(|| st.cid_by_label(s(b, "label"))) {
        Some(c) => c,
        None => return api_err("which channel? run this from a conversation attached to it, or name it"),
    };
    let c = st.chans.get_mut(&cid).unwrap();
    if !c.ch.is_member() {
        return api_err(format!("this machine is not admitted to {} yet: it cannot invite", c.ch.label()));
    }
    let now = now_ms();
    let token = random_hex(16);
    let expires = now + INVITE_TTL;
    let mut invites: Vec<Value> = c.ch.cfg.get("invites").and_then(Value::as_array).cloned().unwrap_or_default().into_iter().filter(|i| u(i, "exp") > now).collect();
    invites.push(json!({ "token": token, "exp": expires }));
    c.ch.cfg.insert("invites".into(), Value::Array(invites));
    c.ch.save_cfg();
    let (machines, names) = c.ch.invite_lists();
    let code = encode_invite(&Invite {
        secret: c.ch.str("secret"),
        label: Some(c.ch.label()),
        inviter: c.ch.machine(),
        inviter_key: c.ch.me.clone(),
        token,
        expires,
        machines,
        names,
    });
    log!("invitation made for {}", c.ch.label());
    json!({ "code": code, "expires": expires, "label": c.ch.label() })
}

fn who(st: &State, b: &Value) -> Value {
    let cid = match st.held(s(b, "conv")).map(|(c, _)| c).or_else(|| st.cid_by_label(s(b, "label"))) {
        Some(c) => c,
        None => return api_err("which channel? run this from a conversation attached to it, or name it"),
    };
    let me_d = st.held(s(b, "conv")).map(|(_, d)| d);
    let c = &st.chans[&cid];
    let now = now_ms();
    let rows: Vec<Value> = c
        .ch
        .names
        .map
        .values()
        .filter(|n| n.active() && !c.ch.members.blocked.contains_key(&n.k))
        .map(|n| {
            let mine = n.k == c.ch.me;
            let online = mine || c.peers.contains_key(&n.k);
            let state = if !online {
                let seen = c.ch.members.machines.get(&n.k).map(|m| m.seen).unwrap_or(0);
                if seen > 0 { format!("offline since {}", crate::time::ago(now, seen).trim_end_matches(" ago")) } else { "offline".into() }
            } else if !n.on {
                "idle".into()
            } else if mine {
                let unread = c.ch.log.unread(&n.n).len();
                match st.convs.holder(&cid, &n.n) {
                    Some(h) if h.app() == "claude" && !st.listening(&h.key) && unread > 0 => format!("idle, {unread} unread"),
                    Some(h) if h.app() == "claude" && !st.listening(&h.key) => "idle".into(),
                    _ if unread > 0 => format!("online, {unread} unread"),
                    _ => "online".into(),
                }
            } else {
                "online".into()
            };
            json!({
                "denden": n.n, "you": me_d.as_deref() == Some(n.n.as_str()), "machine": if mine { c.ch.machine() } else { c.ch.members.display(&n.k) },
                "app": n.app, "role": n.role, "state": state, "mine": mine,
            })
        })
        .collect();
    json!({ "label": c.ch.label(), "machine": c.ch.machine(), "denden": rows, "machines": c.ch.members.machines.len() + 1 })
}

fn history(st: &State, q: &http::Request) -> Value {
    let conv = q.q("conv").unwrap_or("");
    let Some((cid, d)) = st.held(conv) else { return api_err("this conversation is not attached to a denden") };
    let c = &st.chans[&cid];
    let with = q.q("with").map(|w| w.trim_start_matches('@').to_lowercase());
    let n = q.q("n").and_then(|n| n.parse::<usize>().ok()).unwrap_or(20).clamp(1, 500);
    let queued: HashSet<String> = c.ch.outbox.iter().map(|e| s(e, "id").to_string()).collect();
    let now = now_ms();
    let all: Vec<&Value> = c
        .ch
        .log
        .messages
        .iter()
        .filter(|m| s(m, "d") == d && s(m, "dir") != "note")
        .filter(|m| with.as_deref().is_none_or(|w| s(m, "from") == w || m.get("to").is_some_and(|t| t.as_str() == Some(w) || t.as_array().is_some_and(|a| a.iter().any(|x| x.as_str() == Some(w))))))
        .collect();
    let list: Vec<Value> = all[all.len().saturating_sub(n)..]
        .iter()
        .map(|m| {
            let mut v = item(c, m, now);
            v["dir"] = json!(s(m, "dir"));
            if s(m, "dir") == "out" {
                v["queued"] = json!(queued.contains(s(m, "id")));
            }
            v
        })
        .collect();
    json!({ "denden": d, "label": c.ch.label(), "messages": list })
}

fn set(st: &mut State, b: &Value) -> Value {
    let conv = s(b, "conv").to_string();
    let Some((cid, d)) = st.held(&conv) else { return api_err("this conversation is not attached to a denden") };
    if let Some(a) = b.get("auto").and_then(Value::as_bool) {
        st.convs.entry(&conv, now_ms()).auto = a;
        log!("{conv}: auto {}", if a { "on" } else { "off" });
    }
    if let Some(w) = b.get("wakes").and_then(Value::as_u64) {
        st.convs.entry(&conv, now_ms()).wakes = w.min(10_000) as u32;
    }
    if let Some(role) = b.get("role").and_then(Value::as_str) {
        let role: String = role.trim().chars().take(MAX_ROLE).collect();
        st.update_name(&cid, &d, |n| n.role = role);
    }
    st.save_convs();
    conv_status(st, &conv)
}

fn block(st: &mut State, b: &Value, on: bool) -> Value {
    let cid = match st.held(s(b, "conv")).map(|(c, _)| c).or_else(|| st.cid_by_label(s(b, "label"))) {
        Some(c) => c,
        None => return api_err("which channel? run this from a conversation attached to it"),
    };
    let what = s(b, "machine");
    let c = st.chans.get_mut(&cid).unwrap();
    let keys: Vec<String> = if on {
        c.ch.members.resolve(what).into_iter().filter(|k| !c.ch.members.blocked.contains_key(k)).collect()
    } else {
        let w = what.trim().to_lowercase();
        let (name, fp) = match w.split_once('[') {
            Some((n, rest)) => (n.trim().to_string(), norm_fp(rest)),
            None => (w.clone(), String::new()),
        };
        c.ch.members.blocked.iter().filter(|(k, n)| **n == name && fingerprint_hex(k).starts_with(&fp)).map(|(k, _)| k.clone()).collect()
    };
    match keys.len() {
        0 => return api_err(format!("no machine called {what} in {}", c.ch.label())),
        1 => {}
        _ => {
            let names: Vec<String> = keys.iter().map(|k| c.ch.members.display(k)).collect();
            return api_err(format!("several machines are called {what}: {}", names.join(", ")));
        }
    }
    let k = keys[0].clone();
    let name = c.ch.members.name_of(&k);
    let mut fx = Vec::new();
    if on {
        c.ch.members.blocked.insert(k.clone(), name.clone());
        c.ch.outbox.retain(|e| s(e, "mk") != k);
        c.ch.save_outbox();
        if let Some(conn) = c.peers.remove(&k) {
            if let Some(l) = st.live.get(&conn) {
                fx.push(Effect::Kill(l.kill.clone()));
            }
        }
    } else {
        c.ch.members.blocked.remove(&k);
    }
    let c = st.chans.get_mut(&cid).unwrap();
    c.ch.save_members();
    let label = c.ch.label();
    run_effects(fx);
    log!("{} {name} in {label}", if on { "blocked" } else { "unblocked" });
    json!({ "machine": name, "label": label })
}

async fn leave(shared: &Shared, b: &Value) -> Value {
    let (cid, handles, fx) = {
        let mut st = shared.lock().unwrap();
        let Some(cid) = st.held(s(b, "conv")).map(|(c, _)| c).or_else(|| st.cid_by_label(s(b, "label"))) else {
            return api_err("which channel? name it: /denden:im leave <channel>");
        };
        let c = st.chans.remove(&cid).unwrap();
        let mut fx = Vec::new();
        for l in st.live.values().filter(|l| l.cid == cid) {
            fx.push(Effect::Kill(l.kill.clone()));
        }
        let keys: Vec<String> = st.convs.attached().filter(|x| x.cid == cid).map(|x| x.key.clone()).collect();
        for k in keys {
            st.convs.detach(&k);
            st.watchers.retain(|w| w.conv != k);
        }
        st.save_convs();
        let handles: Vec<SwarmHandle> = c.swarms.values().filter_map(|s| s.handle.clone()).collect();
        log!("left {}", c.ch.label());
        (cid, handles, fx)
    };
    run_effects(fx);
    for h in handles {
        let _ = tokio::time::timeout(Duration::from_secs(3), h.destroy()).await;
    }
    let _ = std::fs::remove_dir_all(channels_dir().join(&cid));
    json!({ "ok": true })
}

/// A hook from Claude (UserPromptSubmit) or Codex (UserPromptSubmit, Stop). It speaks for the
/// conversation that holds a denden, or for nothing: Codex also runs hooks in threads it spawns
/// (a guardian review during a turn of the conversation, which is then busy).
fn hook(st: &mut State, b: &Value) -> Value {
    let now = now_ms();
    let app = s(b, "app");
    let prompt = s(b, "event") == "UserPromptSubmit";
    let session = s(b, "session");
    let conv = format!("{app}:{session}");
    if app == "codex" && st.held(&conv).is_none() {
        let home_c = b.get("codexHome").and_then(Value::as_str).filter(|h| !h.is_empty()).map(PathBuf::from).unwrap_or_else(codex::codex_home);
        if let codex::Kind::Sub(parent) = codex::classify(&home_c, session) {
            if let Some(root) = codex::user_thread(&home_c, &parent) {
                let key = format!("codex:{root}");
                if st.held(&key).is_some() {
                    st.wakes.entry(key).or_default().busy_since = Some(now);
                }
            }
        }
        return json!({ "accepted": false, "items": [] });
    }
    if st.held(&conv).is_none() {
        return json!({ "accepted": false, "items": [] });
    }
    st.last_active = now;
    let c = st.convs.entry(&conv, now);
    c.seen = now;
    if !s(b, "cwd").is_empty() {
        c.cwd = s(b, "cwd").to_string();
    }
    let (budget, auto) = (c.wakes, c.auto);
    if app == "codex" {
        if let Some(t) = codex::title(&b.get("codexHome").and_then(Value::as_str).map(PathBuf::from).unwrap_or_else(codex::codex_home), session) {
            st.convs.entry(&conv, now).title = t;
        }
        let w = st.wakes.entry(conv.clone()).or_default();
        w.hook(prompt, now);
        if let Some(argv) = codex::argv_from_request(b.get("codex")) {
            w.codex = Some(argv);
        }
    }
    let mut items = Vec::new();
    if truthy(b.get("check")) {
        // only asking
    } else if prompt {
        items = take_unread(st, &conv, false);
    } else if !unread_items(st, &conv).is_empty() {
        // Stop: the agent goes on with them, which costs a wake-up.
        let w = st.wakes.entry(conv.clone()).or_default();
        if w.allowed(now, budget, 0) {
            w.record(now);
            w.busy_since = Some(now);
            items = take_unread(st, &conv, false);
        }
    }
    let (cid, d) = st.held(&conv).unwrap();
    st.save_convs();
    st.save_wakes();
    json!({ "accepted": true, "items": items, "auto": auto, "denden": d, "label": st.chans[&cid].ch.label(), "conv": conv })
}

// ---------- local API ----------

fn json_reply(code: u16, v: &Value) -> Vec<u8> {
    http::response(code, &[("content-type", "application/json")], serde_json::to_string(v).unwrap_or_default().as_bytes())
}

fn reply(v: Value) -> Vec<u8> {
    let code = if v.get("error").is_some() { 400 } else { 200 };
    json_reply(code, &v)
}

fn body_json(req: &http::Request) -> Value {
    if req.body.is_empty() {
        return json!({});
    }
    serde_json::from_slice(&req.body).unwrap_or(json!({}))
}

async fn client_gone(rd: &mut tokio::net::tcp::OwnedReadHalf) {
    let mut b = [0u8; 64];
    loop {
        match rd.read(&mut b).await {
            Ok(0) | Err(_) => return,
            Ok(_) => continue,
        }
    }
}

/// A streaming answer: newline-delimited JSON until the client goes away or the sender closes.
async fn stream_out(wr: &mut tokio::net::tcp::OwnedWriteHalf, rd: &mut tokio::net::tcp::OwnedReadHalf, mut rx: mpsc::UnboundedReceiver<String>) {
    if wr.write_all(&http::chunked_head(200, "application/x-ndjson")).await.is_err() {
        return;
    }
    let mut ping = tokio::time::interval(Duration::from_secs(30));
    ping.tick().await;
    loop {
        let data = tokio::select! {
            line = rx.recv() => match line { Some(l) => l, None => break },
            _ = ping.tick() => "{\"kind\":\"ping\"}\n".to_string(),
            _ = client_gone(rd) => break,
        };
        if wr.write_all(&http::chunk(data.as_bytes())).await.is_err() {
            break;
        }
    }
    let _ = wr.write_all(&http::chunk(b"")).await;
}

async fn handle_api(shared: Shared, stream: tokio::net::TcpStream, token: String) {
    let (mut rd, mut wr) = stream.into_split();
    let Some(req) = http::server::read_request(&mut rd, MAX_TEXT * 8).await else { return };
    if !safe_eq(req.header("x-denden-token").unwrap_or(""), &token) {
        let _ = wr.write_all(&json_reply(403, &json!({ "error": "forbidden" }))).await;
        return;
    }
    let b = body_json(&req);
    let route = format!("{} {}", req.method, req.path);
    let out = match route.as_str() {
        "GET /status" => {
            let st = shared.lock().unwrap();
            let mut v = json!({
                "pid": st.pid, "uptime": now_ms() - st.started_at, "keepalive": st.keepalive, "presence": st.presence,
                "channels": st.chans.values().map(|c| json!({ "label": c.ch.label(), "member": c.ch.is_member(), "announced": c.announced, "online": c.peers.len(), "denden": c.ch.my_names() })).collect::<Vec<_>>(),
            });
            if let Some(conv) = req.q("conv") {
                v["conv"] = conv_status(&st, conv);
            }
            if let Some(label) = req.q("label") {
                v["channel"] = st.chan_by_label(label).map(|c| json!({ "label": c.ch.label(), "member": c.ch.is_member(), "online": c.peers.len(), "announced": c.announced, "refused": c.last_refusal.as_ref().filter(|(_, at)| now_ms().saturating_sub(*at) < 120_000).map(|(e, _)| e.clone()) })).unwrap_or(Value::Null);
            }
            reply(v)
        }
        "POST /create" => reply(create(&shared, &b)),
        "POST /join" => reply(join(&shared, &b)),
        "POST /attach" => reply(attach(&shared, &b)),
        "POST /detach" => reply(detach(&mut shared.lock().unwrap(), &b)),
        "POST /delete" => reply(delete(&mut shared.lock().unwrap(), &b)),
        "POST /invite" => reply(invite(&mut shared.lock().unwrap(), &b)),
        "GET /who" => {
            let q = json!({ "conv": req.q("conv").unwrap_or(""), "label": req.q("label").unwrap_or("") });
            reply(who(&shared.lock().unwrap(), &q))
        }
        "POST /send" => reply(send(&shared, &b).await.unwrap_or_else(api_err)),
        "GET /inbox" => {
            let mut st = shared.lock().unwrap();
            let conv = req.q("conv").unwrap_or("").to_string();
            match st.held(&conv) {
                None => reply(api_err("this conversation is not attached to a denden")),
                Some(_) => {
                    st.last_active = now_ms();
                    let items = take_unread(&mut st, &conv, req.q("peek").is_some());
                    let auto = st.convs.get(&conv).is_some_and(|c| c.auto);
                    st.save_wakes();
                    reply(json!({ "items": items, "auto": auto }))
                }
            }
        }
        "GET /watch" => {
            let conv = req.q("conv").unwrap_or("").to_string();
            let (tx, rx) = mpsc::unbounded_channel::<String>();
            let wid = {
                let mut st = shared.lock().unwrap();
                if st.held(&conv).is_none() {
                    None
                } else {
                    st.next_id += 1;
                    let wid = st.next_id;
                    st.last_active = now_ms();
                    // A new listener starts from what is unread: everything announced again.
                    st.wakes.entry(conv.clone()).or_default().notified = 0;
                    st.watchers.push(Watcher { id: wid, conv: conv.clone(), tx });
                    notify(&mut st);
                    Some(wid)
                }
            };
            let Some(wid) = wid else {
                let _ = wr.write_all(&reply(api_err("this conversation is not attached to a denden"))).await;
                return;
            };
            stream_out(&mut wr, &mut rd, rx).await;
            shared.lock().unwrap().watchers.retain(|w| w.id != wid);
            return;
        }
        "POST /read" => {
            let mut st = shared.lock().unwrap();
            let seqs: Vec<u64> = b.get("seqs").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
            mark_read(&mut st, s(&b, "conv"), &seqs);
            st.save_wakes();
            reply(json!({ "ok": true }))
        }
        "GET /history" => reply(history(&shared.lock().unwrap(), &req)),
        "POST /set" => reply(set(&mut shared.lock().unwrap(), &b)),
        "POST /keepalive" => {
            let mut st = shared.lock().unwrap();
            st.keepalive = truthy(b.get("on"));
            let _ = write_json(&settings_file(), &json!({ "keepalive": st.keepalive }));
            log!("keepalive {}", if st.keepalive { "on" } else { "off" });
            reply(json!({ "keepalive": st.keepalive }))
        }
        "POST /block" => reply(block(&mut shared.lock().unwrap(), &b, true)),
        "POST /unblock" => reply(block(&mut shared.lock().unwrap(), &b, false)),
        "POST /leave" => reply(leave(&shared, &b).await),
        "POST /hook" => reply(hook(&mut shared.lock().unwrap(), &b)),
        "GET /presence" => {
            let (tx, rx) = mpsc::unbounded_channel::<String>();
            {
                let mut st = shared.lock().unwrap();
                st.presence += 1;
                st.last_active = now_ms();
            }
            let _keep = tx;
            stream_out(&mut wr, &mut rd, rx).await;
            let mut st = shared.lock().unwrap();
            st.presence = st.presence.saturating_sub(1);
            st.last_active = now_ms();
            return;
        }
        "POST /stop" => {
            let _ = wr.write_all(&json_reply(200, &json!({ "ok": true }))).await;
            let _ = wr.shutdown().await;
            shutdown(shared).await;
            return;
        }
        _ => json_reply(404, &json!({ "error": "not found" })),
    };
    let _ = wr.write_all(&out).await;
    let _ = wr.shutdown().await;
}

pub async fn shutdown(shared: Shared) {
    let (handles, pid) = {
        let mut st = shared.lock().unwrap();
        if st.stopping {
            return;
        }
        st.stopping = true;
        for l in st.live.values() {
            l.kill.notify_one();
        }
        let handles: Vec<SwarmHandle> = st.chans.values().flat_map(|c| c.swarms.values().filter_map(|s| s.handle.clone())).collect();
        (handles, st.pid)
    };
    log!("stopping");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let destroy_all = async {
        for h in handles {
            let _ = h.destroy().await;
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(3), destroy_all).await;
    let dir = home();
    if daemon_info(&dir).is_some_and(|i| i.pid == pid) {
        let _ = std::fs::remove_file(dir.join("daemon.json"));
    }
    if read_json(&dir.join("daemon.lock")).and_then(|l| l.get("pid").and_then(Value::as_u64)) == Some(pid as u64) {
        let _ = std::fs::remove_file(dir.join("daemon.lock"));
    }
    std::process::exit(0);
}

/// Every 30 seconds: stop when nobody here could read what arrives.
fn idle_check(shared: &Shared) -> bool {
    let st = shared.lock().unwrap();
    let now = now_ms();
    // Attached conversations keep it up (design decision 27), and so does an invitation not used
    // yet: the new machine is admitted by this one, which must be online when it comes.
    let attached = st.convs.attached().any(|c| st.chans.contains_key(&c.cid));
    let inviting = st.chans.values().any(|c| c.ch.cfg.get("invites").and_then(Value::as_array).is_some_and(|a| a.iter().any(|i| u(i, "exp") > now)));
    !st.keepalive && !attached && !inviting && st.watchers.is_empty() && st.presence == 0 && now.saturating_sub(st.last_active) > IDLE_STOP
}

/// Hourly: history older than 30 days, and messages undelivered for 7 days.
fn housekeeping(shared: &Shared) {
    let mut st = shared.lock().unwrap();
    let now = now_ms();
    for c in st.chans.values_mut() {
        c.ch.purge(now);
    }
    expire_outbox(&mut st);
}

// ---------- startup ----------

pub fn run() -> Result<(), String> {
    let dir = home();
    ensure_dir(&dir).map_err(|e| e.to_string())?;
    // One daemon per machine: a fresh lock means another one is starting right now, an older one
    // counts only if that daemon actually answers (PIDs get reused).
    let lock_file = dir.join("daemon.lock");
    let pid = std::process::id();
    if let Some(lock) = read_json(&lock_file) {
        let lpid = lock.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32;
        let lt = lock.get("t").and_then(Value::as_u64).unwrap_or(0);
        if lpid != pid && is_alive(lpid) && (now_ms().saturating_sub(lt) < 10000 || daemon_up(&dir)) {
            log!("another daemon is already running, exiting");
            std::process::exit(0);
        }
    }
    let _ = write_json(&lock_file, &json!({ "pid": pid, "t": now_ms() }));

    #[cfg(feature = "trace")]
    if let Ok(filter) = std::env::var("DENDEN_TRACE") {
        tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::new(filter)).with_ansi(false).with_writer(std::io::stdout).init();
    }
    let now = now_ms();
    let wakes: HashMap<String, WakeState> =
        read_json(&dir.join("wakes.json")).and_then(|v| v.as_object().cloned()).unwrap_or_default().iter().map(|(k, v)| (k.clone(), WakeState::from_json(v))).collect();
    let state = State {
        chans: BTreeMap::new(),
        convs: Convs::load(&conversations_file()),
        wakes,
        keepalive: read_json(&settings_file()).is_some_and(|v| truthy(v.get("keepalive"))),
        live: BTreeMap::new(),
        watchers: Vec::new(),
        ack_waiters: HashMap::new(),
        next_id: 0,
        stopping: false,
        started_at: now,
        pid,
        presence: 0,
        last_active: now,
    };
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    rt.block_on(async move {
        let shared: Shared = Arc::new(Mutex::new(state));
        for (cid, label) in store::list_channels() {
            match Channel::load(&cid) {
                Ok(mut ch) => {
                    ch.purge(now);
                    start_channel(&shared, ch);
                }
                Err(e) => log!("channel {label}: {e}"),
            }
        }
        let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let token = random_hex(16);
        let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
        let _ = write_json(
            &dir.join("daemon.json"),
            &json!({
                "pid": pid, "port": port, "token": token, "startedAt": now,
                "engine": "rust", "version": binary::VERSION, "buildTime": binary::BUILD_TIME,
                "exe": exe, "sha256": binary::own_sha256(),
            }),
        );
        let n = shared.lock().unwrap().chans.len();
        log!("daemon up on {}, {n} channel(s), api port {port} (denden {} built {}, from {exe})", machine_name(), binary::VERSION, iso(binary::BUILD_TIME));
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(3));
            binary::clean_up(Path::new(&exe));
        });
        housekeeping(&shared);
        {
            let sh = shared.clone();
            tokio::spawn(async move {
                let every = Duration::from_millis(1000.max(60000.min(net::grace_ms() / 3)));
                let mut tick = tokio::time::interval(every);
                tick.tick().await;
                loop {
                    tick.tick().await;
                    net::reconcile(&sh);
                }
            });
        }
        {
            let sh = shared.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(1));
                let mut n: u64 = 0;
                loop {
                    tick.tick().await;
                    n += 1;
                    notify(&mut sh.lock().unwrap());
                    codex_tick(&sh);
                    if n % 30 == 0 && idle_check(&sh) {
                        log!("nobody on this machine listens any more: stopping (keepalive on keeps the daemon up)");
                        shutdown(sh.clone()).await;
                    }
                    if n % 3600 == 0 {
                        housekeeping(&sh);
                    }
                }
            });
        }
        {
            let sh = shared.clone();
            tokio::spawn(async move {
                wait_for_signal().await;
                shutdown(sh).await;
            });
        }
        loop {
            let Ok((stream, _)) = listener.accept().await else { continue };
            tokio::spawn(handle_api(shared.clone(), stream, token.clone()));
        }
    })
}

#[cfg(unix)]
async fn wait_for_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut term), Ok(mut int)) = (signal(SignalKind::terminate()), signal(SignalKind::interrupt())) else {
        return std::future::pending().await;
    };
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
}

#[cfg(windows)]
async fn wait_for_signal() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipient_lists() {
        assert_eq!(recipients("all").unwrap(), None);
        assert_eq!(recipients("@back").unwrap(), Some(vec!["back".to_string()]));
        assert_eq!(recipients("api,stress, db").unwrap(), Some(vec!["api".to_string(), "stress".to_string(), "db".to_string()]));
        assert_eq!(recipients("api,api").unwrap(), Some(vec!["api".to_string()]));
        assert!(recipients("api,all").is_err());
        assert!(recipients("Bad Name!").is_err());
        assert!(recipients("").is_err());
    }
}
