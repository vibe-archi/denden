// One channel on disk, as this machine knows it: its secret and our machine key (channel.json), the
// machines we know (members.json), who owns which denden name (names.json), the messages of our
// denden with what each has read (messages.jsonl, state.json), and what waits to be delivered
// (outbox.json). The daemon owns these files; the CLI only lists channels and reads labels.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use ed25519_dalek::SigningKey;
use serde_json::{Map, Value, json};

use crate::core::*;
use crate::proto::*;

/// Messages older than this are purged from the history.
pub const HISTORY_TTL: u64 = 30 * 24 * 3600 * 1000;
/// A message still not delivered after this is dropped, and its sender told.
pub const OUTBOX_TTL: u64 = 7 * 24 * 3600 * 1000;

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// The folder name of a channel: from its id, so a local label can change freely.
pub fn cid_of(secret_hex: &str) -> String {
    hex::encode(ChannelKeys::new(secret_hex).id)[..16].to_string()
}

/// Channel folders on this machine with their local labels, sorted by label.
pub fn list_channels() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = std::fs::read_dir(channels_dir())
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let cid = e.file_name().to_string_lossy().to_string();
                    let cfg = read_json(&e.path().join("channel.json"))?;
                    (cfg.get("v").and_then(Value::as_u64) == Some(PROTO_VERSION)).then(|| (cid, s(&cfg, "label").to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

/// The channel this machine calls `label`.
pub fn find_channel(label: &str) -> Option<String> {
    list_channels().into_iter().find(|(_, l)| l == label).map(|(cid, _)| cid)
}

// ---------- machines ----------

#[derive(Clone, Debug)]
pub struct Member {
    pub name: String,
    /// The certificates that admitted it (empty for the creator of the channel).
    pub chain: Value,
    pub since: u64,
    pub seen: u64,
}

#[derive(Default, Debug)]
pub struct Members {
    pub machines: BTreeMap<String, Member>,
    /// Blocked here: machine key -> its name when it was blocked.
    pub blocked: BTreeMap<String, String>,
}

impl Members {
    pub fn knows(&self, key: &str) -> bool {
        self.machines.contains_key(key)
    }
    pub fn name_of(&self, key: &str) -> String {
        self.machines.get(key).map(|m| m.name.clone()).unwrap_or_else(|| short_fp(&hex::decode(key).unwrap_or_default()))
    }
    /// The name shown for a machine: with the start of its fingerprint when another one shares it.
    pub fn display(&self, key: &str) -> String {
        let name = self.name_of(key);
        let twins = self.machines.iter().filter(|(_, m)| m.name == name).count();
        if twins > 1 { format!("{name} [{}]", &fingerprint_hex(key)[..4]) } else { name }
    }
    /// Machine keys matching what a user typed: a name, or "name [abcd]" / an fingerprint prefix.
    pub fn resolve(&self, what: &str) -> Vec<String> {
        let w = what.trim().to_lowercase();
        let (name, fp) = match w.split_once('[') {
            Some((n, rest)) => (n.trim().to_string(), Some(norm_fp(rest))),
            None => (w.clone(), None),
        };
        self.machines
            .iter()
            .filter(|(k, m)| {
                let f = fingerprint_hex(k);
                match &fp {
                    Some(p) => m.name == name && f.starts_with(p.as_str()),
                    None => m.name == name || (name.len() >= 4 && f.starts_with(&norm_fp(&name))),
                }
            })
            .map(|(k, _)| k.clone())
            .collect()
    }
    pub fn add(&mut self, key: &str, name: &str, chain: Value, now: u64) -> bool {
        match self.machines.get_mut(key) {
            Some(m) => {
                let changed = m.name != name || (m.chain.as_array().is_none_or(|a| a.is_empty()) && chain.as_array().is_some_and(|a| !a.is_empty()));
                m.name = name.to_string();
                if chain.as_array().is_some_and(|a| !a.is_empty()) {
                    m.chain = chain;
                }
                changed
            }
            None => {
                self.machines.insert(key.to_string(), Member { name: name.to_string(), chain, since: now, seen: 0 });
                true
            }
        }
    }
    fn to_json(&self) -> Value {
        let machines: Map<String, Value> =
            self.machines.iter().map(|(k, m)| (k.clone(), json!({ "n": m.name, "c": m.chain, "since": m.since, "seen": m.seen }))).collect();
        json!({ "machines": machines, "blocked": self.blocked })
    }
    fn from_json(v: Option<Value>) -> Members {
        let v = v.unwrap_or(json!({}));
        let machines = v
            .get("machines")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter(|(k, _)| is_hex64(k))
                    .map(|(k, e)| {
                        let name = s(e, "n").to_string();
                        let chain = e.get("c").cloned().unwrap_or(json!([]));
                        let (since, seen) = (e.get("since").and_then(Value::as_u64).unwrap_or(0), e.get("seen").and_then(Value::as_u64).unwrap_or(0));
                        (k.clone(), Member { name, chain, since, seen })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let blocked = v
            .get("blocked")
            .and_then(Value::as_object)
            .map(|b| b.iter().map(|(k, n)| (k.clone(), js_string(n))).collect())
            .unwrap_or_default();
        Members { machines, blocked }
    }
}

// ---------- the message log ----------

/// What a denden has read: everything up to `upto`, plus `extra` out of order.
#[derive(Default, Debug, Clone)]
pub struct ReadState {
    pub upto: u64,
    pub extra: BTreeSet<u64>,
}

/// Records, one JSON object per line: `dir` is "in" (received by local denden `d`), "out" (sent by
/// `d`) or "note" (a notice from denden itself to the conversation of `d`).
#[derive(Default)]
pub struct Log {
    pub messages: Vec<Value>,
    pub seq: u64,
    by_seq: HashMap<u64, usize>,
    pub read: BTreeMap<String, ReadState>,
    seen: HashSet<String>,
}

fn seq_of(m: &Value) -> u64 {
    m.get("seq").and_then(Value::as_u64).unwrap_or(0)
}

/// Received or a notice: what a conversation is handed.
fn incoming(m: &Value) -> bool {
    matches!(s(m, "dir"), "in" | "note")
}

impl Log {
    fn load(file: &Path, state: Option<Value>) -> Log {
        let mut log = Log::default();
        if let Ok(text) = std::fs::read_to_string(file) {
            for line in text.lines().filter(|l| !l.trim().is_empty()) {
                if let Ok(v) = serde_json::from_str::<Value>(line) {
                    log.index(v);
                }
            }
        }
        if let Some(Value::Object(r)) = state.as_ref().and_then(|s| s.get("read")) {
            for (d, st) in r {
                let upto = st.get("upto").and_then(Value::as_u64).unwrap_or(0);
                let extra = st.get("extra").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
                log.read.insert(d.clone(), ReadState { upto, extra });
            }
        }
        log
    }
    fn index(&mut self, v: Value) {
        let seq = seq_of(&v);
        self.seq = self.seq.max(seq);
        if s(&v, "dir") == "in" {
            self.seen.insert(format!("{}/{}/{}", s(&v, "from"), s(&v, "id"), s(&v, "d")));
        }
        self.by_seq.insert(seq, self.messages.len());
        self.messages.push(v);
    }
    /// Already received by denden `d` (a resent message).
    pub fn has(&self, from: &str, id: &str, d: &str) -> bool {
        self.seen.contains(&format!("{from}/{id}/{d}"))
    }
    pub fn get(&self, seq: u64) -> Option<&Value> {
        self.by_seq.get(&seq).map(|&i| &self.messages[i])
    }
    pub fn by_id(&self, id: &str) -> Option<&Value> {
        self.messages.iter().rev().find(|m| s(m, "id") == id)
    }
    fn is_read(&self, d: &str, seq: u64) -> bool {
        self.read.get(d).is_some_and(|r| seq <= r.upto || r.extra.contains(&seq))
    }
    pub fn unread(&self, d: &str) -> Vec<&Value> {
        let upto = self.read.get(d).map(|r| r.upto).unwrap_or(0);
        self.messages.iter().filter(|m| seq_of(m) > upto && incoming(m) && s(m, "d") == d && !self.is_read(d, seq_of(m))).collect()
    }
    /// Marks `seqs` read for `d` (only its own incoming records count). True if anything changed.
    pub fn mark_read(&mut self, d: &str, seqs: &[u64]) -> bool {
        let mine: Vec<u64> =
            seqs.iter().copied().filter(|q| self.get(*q).is_some_and(|m| incoming(m) && s(m, "d") == d) && !self.is_read(d, *q)).collect();
        if mine.is_empty() {
            return false;
        }
        let st = self.read.entry(d.to_string()).or_default();
        st.extra.extend(mine);
        // Slide `upto` over what is read or not ours, so `extra` stays small.
        loop {
            let next = st.upto + 1;
            match self.by_seq.get(&next).map(|&i| &self.messages[i]) {
                Some(m) if !(incoming(m) && s(m, "d") == d) || st.extra.contains(&next) => {
                    st.extra.remove(&next);
                    st.upto = next;
                }
                None if next <= self.seq => st.upto = next, // purged
                _ => break,
            }
        }
        true
    }
    pub fn record(&mut self, file: &Path, mut m: Map<String, Value>) -> Value {
        self.seq += 1;
        m.insert("seq".into(), json!(self.seq));
        let v = Value::Object(m);
        let _ = append_private(file, &format!("{}\n", serde_json::to_string(&v).unwrap_or_default()));
        self.index(v.clone());
        v
    }
    /// Drops records older than `before` (their time here: `at`, else `ts`). True if any went.
    fn purge(&mut self, file: &Path, before: u64) -> bool {
        let keep = |m: &Value| m.get("at").or_else(|| m.get("ts")).and_then(Value::as_u64).unwrap_or(u64::MAX) >= before;
        if self.messages.iter().all(keep) {
            return false;
        }
        let kept: Vec<Value> = std::mem::take(&mut self.messages).into_iter().filter(keep).collect();
        let text: String = kept.iter().map(|m| format!("{}\n", serde_json::to_string(m).unwrap_or_default())).collect();
        let tmp = file.with_extension("jsonl.tmp");
        if write_text(&tmp, &text).and_then(|_| std::fs::rename(&tmp, file)).is_err() {
            let _ = write_text(file, &text);
        }
        let seq = self.seq;
        *self = Log { read: std::mem::take(&mut self.read), ..Log::default() };
        for m in kept {
            self.index(m);
        }
        self.seq = self.seq.max(seq);
        true
    }
    fn state_json(&self) -> Value {
        let read: Map<String, Value> = self.read.iter().map(|(d, r)| (d.clone(), json!({ "upto": r.upto, "extra": r.extra }))).collect();
        json!({ "read": read })
    }
}

fn write_text(file: &Path, text: &str) -> std::io::Result<()> {
    let _ = std::fs::remove_file(file);
    append_private(file, text)
}

// ---------- the channel ----------

pub struct Channel {
    pub cid: String,
    pub dir: PathBuf,
    pub cfg: Map<String, Value>,
    pub keys: ChannelKeys,
    pub sk: SigningKey,
    /// Our machine key on this channel (hex).
    pub me: String,
    pub members: Members,
    pub names: Names,
    pub log: Log,
    pub outbox: Vec<Value>,
}

impl Channel {
    fn file(&self, f: &str) -> PathBuf {
        self.dir.join(f)
    }
    pub fn str(&self, k: &str) -> String {
        self.cfg.get(k).and_then(Value::as_str).unwrap_or("").to_string()
    }
    pub fn label(&self) -> String {
        self.str("label")
    }
    pub fn machine(&self) -> String {
        self.str("machine")
    }
    pub fn seed(&self) -> String {
        self.str("seed")
    }
    pub fn chain(&self) -> Vec<Value> {
        self.cfg.get("chain").and_then(Value::as_array).cloned().unwrap_or_default()
    }
    pub fn is_creator(&self) -> bool {
        truthy(self.cfg.get("creator"))
    }
    /// Admitted: the creator, or holding a certificate.
    pub fn is_member(&self) -> bool {
        self.is_creator() || !self.chain().is_empty()
    }

    /// A new channel created on this machine.
    pub fn create(label: &str, now: u64) -> Result<Channel, String> {
        let secret = new_channel_secret();
        let mut cfg = Map::new();
        cfg.insert("creator".into(), json!(true));
        Channel::init(&secret, label, cfg, now)
    }

    /// A channel this machine joins with an invitation: it knows the inviter and what the inviter
    /// knew, and holds the token until a member admits it.
    pub fn join(inv: &Invite, label: &str, now: u64) -> Result<Channel, String> {
        let mut cfg = Map::new();
        cfg.insert("joinToken".into(), json!(inv.token));
        cfg.insert("inviter".into(), json!(inv.inviter));
        cfg.insert("inviterKey".into(), json!(inv.inviter_key));
        let mut ch = Channel::init(&inv.secret, label, cfg, now)?;
        ch.members.add(&inv.inviter_key, &inv.inviter, json!([]), now);
        for (n, k) in &inv.machines {
            if *k != ch.me {
                ch.members.add(k, n, json!([]), now);
            }
        }
        // The names it knew, unsigned: they only keep us from taking a name in use until the
        // signed certificates arrive with the first connection.
        let pending: Vec<Value> = inv.names.iter().map(|(n, k)| json!([n, k])).collect();
        ch.cfg.insert("knownNames".into(), Value::Array(pending));
        ch.save_cfg();
        ch.save_members();
        Ok(ch)
    }

    fn init(secret: &str, label: &str, mut cfg: Map<String, Value>, now: u64) -> Result<Channel, String> {
        let cid = cid_of(secret);
        let dir = channels_dir().join(&cid);
        if dir.join("channel.json").exists() {
            return Err("this machine is already in this channel".into());
        }
        ensure_dir(&dir).map_err(|e| e.to_string())?;
        cfg.insert("v".into(), json!(PROTO_VERSION));
        cfg.insert("secret".into(), json!(secret));
        cfg.insert("label".into(), json!(label));
        cfg.insert("seed".into(), json!(random_hex(32)));
        cfg.insert("machine".into(), json!(machine_name()));
        cfg.insert("createdAt".into(), json!(now));
        cfg.entry("chain").or_insert(json!([]));
        cfg.insert("invites".into(), json!([]));
        write_json(&dir.join("channel.json"), &Value::Object(cfg)).map_err(|e| e.to_string())?;
        Channel::load(&cid)
    }

    pub fn load(cid: &str) -> Result<Channel, String> {
        let dir = channels_dir().join(cid);
        let cfg = read_json(&dir.join("channel.json"))
            .and_then(|v| v.as_object().cloned())
            .filter(|c| c.get("v").and_then(Value::as_u64) == Some(PROTO_VERSION))
            .ok_or_else(|| format!("no channel in {}", dir.display()))?;
        let keys = ChannelKeys::new(cfg.get("secret").and_then(Value::as_str).unwrap_or(""));
        let id = identity_keys(cfg.get("seed").and_then(Value::as_str).unwrap_or("")).ok_or("bad machine key in channel.json")?;
        let names = Names::from_json(&keys.id, read_json(&dir.join("names.json")).as_ref());
        let members = Members::from_json(read_json(&dir.join("members.json")));
        let log = Log::load(&dir.join("messages.jsonl"), read_json(&dir.join("state.json")));
        let outbox = read_json(&dir.join("outbox.json")).and_then(|v| v.as_array().cloned()).unwrap_or_default();
        Ok(Channel { cid: cid.to_string(), dir, cfg, keys, sk: id.sk, me: hex::encode(id.pub_key), members, names, log, outbox })
    }

    pub fn save_cfg(&self) {
        let _ = write_json(&self.file("channel.json"), &Value::Object(self.cfg.clone()));
    }
    pub fn save_members(&self) {
        let _ = write_json(&self.file("members.json"), &self.members.to_json());
    }
    pub fn save_names(&self) {
        let _ = write_json(&self.file("names.json"), &self.names.to_json());
    }
    pub fn save_outbox(&self) {
        let _ = write_json(&self.file("outbox.json"), &Value::Array(self.outbox.clone()));
    }
    pub fn save_read(&self) {
        let _ = write_json(&self.file("state.json"), &self.log.state_json());
    }
    pub fn record(&mut self, m: Map<String, Value>) -> Value {
        let file = self.file("messages.jsonl");
        self.log.record(&file, m)
    }
    pub fn mark_read(&mut self, d: &str, seqs: &[u64]) {
        if self.log.mark_read(d, seqs) {
            self.save_read();
        }
    }
    /// History older than 30 days goes.
    pub fn purge(&mut self, now: u64) {
        let file = self.file("messages.jsonl");
        if self.log.purge(&file, now.saturating_sub(HISTORY_TTL)) {
            self.save_read();
        }
    }

    /// Known and not blocked: a machine we accept.
    pub fn knows(&self, key: &str) -> bool {
        key == self.me || (self.members.knows(key) && !self.members.blocked.contains_key(key))
    }
    /// Taken by another machine as far as we know: signed certificates, and until they arrive, the
    /// names the invitation listed.
    pub fn name_taken(&self, n: &str) -> bool {
        if !self.names.free_for(n, &self.me) {
            return true;
        }
        self.cfg.get("knownNames").and_then(Value::as_array).is_some_and(|a| a.iter().any(|e| e[0].as_str() == Some(n) && e[1].as_str() != Some(self.me.as_str())))
    }
    /// Our own denden in use.
    pub fn my_names(&self) -> Vec<String> {
        self.names.active_of(&self.me).map(|c| c.n.clone()).collect()
    }
    pub fn sign_name(&self, c: NameCert, now: u64) -> NameCert {
        c.signed(&self.keys.id, &self.sk, now)
    }
    /// What a new machine needs to know, for an invitation code.
    pub fn invite_lists(&self) -> (Vec<(String, String)>, Vec<(String, String)>) {
        let machines = self.members.machines.iter().filter(|(k, _)| !self.members.blocked.contains_key(*k)).map(|(k, m)| (m.name.clone(), k.clone())).collect();
        let names = self.names.map.values().filter(|c| !c.r).map(|c| (c.n.clone(), c.k.clone())).collect();
        (machines, names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(d: &str, dir: &str) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("d".into(), json!(d));
        m.insert("dir".into(), json!(dir));
        m.insert("at".into(), json!(now_ms()));
        m
    }

    #[test]
    fn read_state_per_denden() {
        let dir = std::env::temp_dir().join(format!("denden-log-test-{}", std::process::id()));
        ensure_dir(&dir).unwrap();
        let file = dir.join("messages.jsonl");
        let _ = std::fs::remove_file(&file);
        let mut log = Log::default();
        let a1 = seq_of(&log.record(&file, rec("front", "in")));
        let b1 = seq_of(&log.record(&file, rec("back", "in")));
        let _o = log.record(&file, rec("front", "out"));
        let a2 = seq_of(&log.record(&file, rec("front", "note")));
        assert_eq!(log.unread("front").len(), 2);
        assert_eq!(log.unread("back").len(), 1);
        assert!(!log.mark_read("front", &[b1]), "not front's");
        assert!(log.mark_read("front", &[a2]));
        assert_eq!(log.unread("front").iter().map(|m| seq_of(m)).collect::<Vec<_>>(), vec![a1]);
        log.mark_read("front", &[a1]);
        assert!(log.unread("front").is_empty());
        assert_eq!(log.read["front"].upto, a2, "slid over back's and its own sent message");
        assert!(log.read["front"].extra.is_empty());
        assert_eq!(log.unread("back").len(), 1, "back's state is its own");
        // Reloaded from disk, with the state.
        let reloaded = Log::load(&file, Some(log.state_json()));
        assert_eq!(reloaded.unread("back").len(), 1);
        assert!(reloaded.unread("front").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn purge_keeps_recent_and_seq() {
        let dir = std::env::temp_dir().join(format!("denden-purge-test-{}", std::process::id()));
        ensure_dir(&dir).unwrap();
        let file = dir.join("messages.jsonl");
        let _ = std::fs::remove_file(&file);
        let mut log = Log::default();
        let mut old = rec("front", "in");
        old.insert("at".into(), json!(1000));
        log.record(&file, old);
        log.record(&file, rec("front", "in"));
        assert!(log.purge(&file, 5000));
        assert_eq!(log.messages.len(), 1);
        assert_eq!(log.seq, 2);
        let next = log.record(&file, rec("front", "in"));
        assert_eq!(seq_of(&next), 3, "seq never goes back");
        assert_eq!(Log::load(&file, None).messages.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn machine_names_resolve() {
        let mut m = Members::default();
        let (k1, k2, k3) = ("a".repeat(64), "b".repeat(64), "c".repeat(64));
        m.add(&k1, "ubuntu", json!([]), 1);
        m.add(&k2, "ubuntu", json!([]), 1);
        m.add(&k3, "laptop-ak", json!([]), 1);
        assert_eq!(m.resolve("laptop-ak"), vec![k3.clone()]);
        assert_eq!(m.resolve("ubuntu").len(), 2);
        let shown = m.display(&k1);
        assert!(shown.starts_with("ubuntu ["), "{shown}");
        assert_eq!(m.resolve(&shown), vec![k1.clone()]);
        assert_eq!(m.display(&k3), "laptop-ak");
    }
}
