// The network side of the daemon: for each channel, one swarm per epoch (topics and transport keys
// change every day, overlapping around the change), and the protocol spoken on each connection.
//
// Inside each Noise-encrypted stream, newline-delimited JSON frames (protocol 3):
//   {t:'auth', v:3, proof}    proof = HMAC(channel auth key, 'auth' | my transport key | theirs);
//                             nothing else is sent until the other side's proof checks out.
//   {t:'hello', machine, key, sig, chain?, inv?, needCert?, names, members}
//   {t:'cert', chain}         a member hands a certificate to the machine it just admitted.
//   {t:'names', names}        name certificates: new, changed, deleted, released.
//   {t:'msg', id, from, to, text, ts, ask?, re?, waited}, {t:'ack', id}, {t:'nack', id, error}
//   {t:'error', error}        the connection is refused.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use peeroxide::{JoinOpts, KeyPair, SwarmConfig, SwarmConnection, spawn};
use serde_json::{Map, Value, json};
use tokio::sync::{Notify, mpsc};

use crate::core::*;
use crate::daemon::{AckResult, Chan, Effect, Live, Shared, State, SwarmSlot, notify, role_note, run_effects, s};
use crate::dlog as log;
use crate::proto::*;

const RATE_PER_MIN: f64 = 20.0;
const FRAME_LIMIT: u32 = 300; // frames per connection per 10 seconds
const MAX_LINE: usize = 256 * 1024;
const MAX_GOSSIP: usize = 500;

static GENERATION: AtomicU64 = AtomicU64::new(1);

pub fn grace_ms() -> u64 {
    (30 * 60 * 1000).min(epoch_ms() / 4)
}

fn send_to(st: &State, conn: u64, frame: &Value) {
    if let Some(l) = st.live.get(&conn) {
        let _ = l.send.send(format!("{}\n", serde_json::to_string(frame).unwrap_or_default()));
    }
}

/// Name certificates to every machine online in channel `cid`, except the one on `skip`. The names
/// in use each machine was shown are kept as impersonation evidence.
pub fn send_names(st: &mut State, cid: &str, certs: &[NameCert], skip: Option<u64>) {
    let Some(c) = st.chans.get_mut(cid) else { return };
    let now = now_ms();
    let mut conns = Vec::new();
    for (key, &conn) in c.peers.iter().filter(|(_, c)| Some(**c) != skip) {
        conns.push(conn);
        for cert in certs.iter().filter(|x| x.active()) {
            c.claims.showed(key, &cert.n, &cert.k, now);
        }
    }
    let frame = json!({ "t": "names", "names": certs.iter().map(NameCert::to_json).collect::<Vec<_>>() });
    for conn in conns {
        send_to(st, conn, &frame);
    }
}

/// One outbox entry on one connection. `waited`: how long it has been held here.
pub fn transmit(st: &State, conn: u64, e: &Value, now: u64) {
    let mut f = json!({ "t": "msg", "id": e.get("id"), "from": e.get("from"), "to": e.get("to"), "text": e.get("text"), "ts": e.get("ts") });
    f["waited"] = json!(now.saturating_sub(e.get("q").and_then(Value::as_u64).unwrap_or(now)));
    for k in ["ask", "re"] {
        if let Some(v) = e.get(k) {
            f[k] = v.clone();
        }
    }
    send_to(st, conn, &f);
}

fn flush_outbox(st: &State, cid: &str, key: &str, conn: u64) {
    let Some(c) = st.chans.get(cid) else { return };
    let now = now_ms();
    for e in c.ch.outbox.iter().filter(|e| s(e, "mk") == key) {
        transmit(st, conn, e, now);
    }
}

fn kill(st: &State, conn: u64, fx: &mut Vec<Effect>) {
    if let Some(l) = st.live.get(&conn) {
        fx.push(Effect::Kill(l.kill.clone()));
    }
}

fn refuse(st: &State, conn: u64, who: &str, error: &str, why: &str, fx: &mut Vec<Effect>) {
    log!("refused {who}: {why}");
    send_to(st, conn, &json!({ "t": "error", "error": error }));
    if let Some(l) = st.live.get(&conn) {
        fx.push(Effect::KillLater(l.kill.clone(), 500));
    }
}

// ---------- hello ----------

fn send_hello(st: &mut State, conn: u64) {
    let Some(l) = st.live.get(&conn) else { return };
    let Some(c) = st.chans.get(&l.cid) else { return };
    let ch = &c.ch;
    let shown: Vec<(String, String)> = ch.names.map.values().take(MAX_GOSSIP).filter(|x| x.active()).map(|x| (x.n.clone(), x.k.clone())).collect();
    let machine = ch.machine();
    let sig = sign(&ch.sk, &frame(&[b"hello", &ch.keys.id, &l.local_t, &l.remote_t, machine.as_bytes(), &hex::decode(&ch.me).unwrap_or_default()]));
    let members: Vec<Value> = ch
        .members
        .machines
        .iter()
        .filter(|(k, _)| !ch.members.blocked.contains_key(*k))
        .take(MAX_GOSSIP)
        .map(|(k, m)| json!({ "n": m.name, "k": k, "c": m.chain }))
        .collect();
    let names: Vec<Value> = ch.names.map.values().take(MAX_GOSSIP).map(NameCert::to_json).collect();
    let mut hello = json!({ "t": "hello", "machine": machine, "key": ch.me, "sig": b64u(&sig), "names": names, "members": members });
    let chain = ch.chain();
    if !chain.is_empty() {
        hello["chain"] = Value::Array(chain);
    }
    if !ch.is_member() {
        hello["needCert"] = json!(true);
    }
    let token = ch.str("joinToken");
    if !token.is_empty() {
        hello["inv"] = json!(join_proof(&token, &ch.keys.id, &l.local_t, &l.remote_t, &ch.me));
    }
    send_to(st, conn, &hello);
    if let Some(l) = st.live.get_mut(&conn) {
        (l.shown, l.hello_at) = (shown, now_ms());
    }
}

/// A single-use invitation of ours, presented by machine `key` on this stream.
fn match_invite(st: &mut State, cid: &str, inv: Option<&Value>, remote_t: &[u8], local_t: &[u8], key: &str) -> bool {
    let Some(inv) = inv.and_then(Value::as_str) else { return false };
    let Some(c) = st.chans.get_mut(cid) else { return false };
    let now = now_ms();
    let mut invites: Vec<Value> =
        c.ch.cfg.get("invites").and_then(Value::as_array).cloned().unwrap_or_default().into_iter().filter(|i| i.get("exp").and_then(Value::as_u64).unwrap_or(0) > now).collect();
    let hit = invites.iter().position(|i| safe_eq(inv, &join_proof(s(i, "token"), &c.ch.keys.id, remote_t, local_t, key)));
    if let Some(i) = hit {
        invites.remove(i); // single use
    }
    c.ch.cfg.insert("invites".into(), Value::Array(invites));
    c.ch.save_cfg();
    hit.is_some()
}

fn on_hello(st: &mut State, conn: u64, f: &Value, fx: &mut Vec<Effect>) {
    let Some(l) = st.live.get(&conn) else { return };
    let (cid, local_t, remote_t) = (l.cid.clone(), l.local_t, l.remote_t);
    let machine = s(f, "machine").to_string();
    let key = s(f, "key").to_string();
    let tpub = hex::encode(remote_t);
    if !valid_name(&machine) || !is_hex64(&key) {
        return kill(st, conn, fx);
    }
    let c = &st.chans[&cid];
    let sig = unb64u(s(f, "sig"));
    if !verify_sig(&hex::decode(&key).unwrap_or_default(), &sig, &frame(&[b"hello", &c.ch.keys.id, &remote_t, &local_t, machine.as_bytes(), &hex::decode(&key).unwrap_or_default()])) {
        log!("rejected {}: bad signature", &tpub[..8]);
        return kill(st, conn, fx);
    }
    let who = format!("{machine} [{}]", short_fp(&hex::decode(&key).unwrap_or_default()));
    if key == c.ch.me {
        return refuse(st, conn, &who, "self", "it presents our own machine key", fx);
    }
    if c.ch.members.blocked.contains_key(&key) {
        return refuse(st, conn, &who, "blocked", "blocked here", fx);
    }
    let known = c.ch.members.knows(&key);
    let vouched = if known { None } else { verify_chain(&c.ch.keys.id, f.get("chain"), &key, &|k| c.ch.knows(k)) };
    let label = c.ch.label();
    let how = if known {
        "known"
    } else if match_invite(st, &cid, f.get("inv"), &remote_t, &local_t, &key) {
        "invited"
    } else if vouched.is_some() {
        "vouched"
    } else {
        let epoch = l_epoch(st, conn);
        st.chans.get_mut(&cid).unwrap().refused_by.insert(tpub, epoch);
        return refuse(st, conn, &who, "not-trusted", "neither invited nor vouched for", fx);
    };
    let now = now_ms();
    let c = st.chans.get_mut(&cid).unwrap();
    let chain = f.get("chain").cloned().unwrap_or(json!([]));
    c.ch.members.add(&key, &machine, chain, now);
    for (n, k) in vouched.map(|(_, learned)| learned).unwrap_or_default() {
        if k != c.ch.me && !c.ch.members.knows(&k) && !c.ch.members.blocked.contains_key(&k) {
            c.ch.members.add(&k, &n, json!([]), now);
        }
    }
    if let Some(m) = c.ch.members.machines.get_mut(&key) {
        m.seen = now;
    }
    // The machines it knows: added when their certificates lead to one we know.
    for m in f.get("members").and_then(Value::as_array).into_iter().flatten().take(MAX_GOSSIP) {
        let (n, k) = (s(m, "n"), s(m, "k"));
        if !valid_name(n) || !is_hex64(k) || k == c.ch.me || c.ch.members.knows(k) || c.ch.members.blocked.contains_key(k) {
            continue;
        }
        if verify_chain(&c.ch.keys.id, m.get("c"), k, &|x| c.ch.knows(x)).is_some() {
            c.ch.members.add(k, n, m.get("c").cloned().unwrap_or(json!([])), now);
        }
    }
    c.ch.save_members();
    c.peers.insert(key.clone(), conn);
    let (mut shown, mut hello_at) = (Vec::new(), 0);
    if let Some(l) = st.live.get_mut(&conn) {
        l.key = Some(key.clone());
        (shown, hello_at) = (std::mem::take(&mut l.shown), l.hello_at);
    }
    let c = &st.chans[&cid];
    if truthy(f.get("needCert")) && c.ch.is_member() {
        let mut chain = vec![make_cert(&c.ch.keys.id, &c.ch.sk, &c.ch.machine(), &c.ch.me, &machine, &key)];
        chain.extend(c.ch.chain());
        chain.truncate(8);
        send_to(st, conn, &json!({ "t": "cert", "chain": chain }));
    }
    log!("{label}: online: {who} ({how})");
    on_names(st, &cid, conn, f.get("names"));
    // Our hello crossed theirs: what it showed them only counts against what they send later.
    if let Some(c) = st.chans.get_mut(&cid) {
        for (n, owner) in &shown {
            c.claims.showed(&key, n, owner, hello_at);
        }
    }
    flush_outbox(st, &cid, &key, conn);
    if how != "known" {
        let fp = fmt_fp(&fingerprint_hex(&key)[..8]);
        let by = if how == "invited" { st.chans[&cid].ch.machine() } else { vouched_by(st, &cid, f) };
        st.note_all(&cid, &format!("New machine in {label}: {machine} [{fp}], invited by {by}"), false, true);
    }
}

fn vouched_by(st: &State, cid: &str, f: &Value) -> String {
    let bk = f.get("chain").and_then(Value::as_array).and_then(|c| c.first()).map(|c| s(c, "bk").to_string()).unwrap_or_default();
    st.chans[cid].ch.members.display(&bk)
}

fn l_epoch(st: &State, conn: u64) -> u64 {
    st.live.get(&conn).map(|l| l.epoch).unwrap_or(0)
}

// ---------- names ----------

/// Name certificates from a peer. New ones go on to the other machines online; a conflict lost
/// over one of our names releases it and tells its conversation; a name of another machine claimed
/// by a machine that knew is reported to the conversations of the channel, and so is a new role.
fn on_names(st: &mut State, cid: &str, from_conn: u64, list: Option<&Value>) {
    let via = st.live.get(&from_conn).and_then(|l| l.key.clone()).unwrap_or_default();
    let Some(c) = st.chans.get_mut(cid) else { return };
    let me = c.ch.me.clone();
    let now = now_ms();
    let mut fresh: Vec<NameCert> = Vec::new();
    let mut lost: Vec<NameCert> = Vec::new();
    let mut claimed: Vec<(String, String)> = Vec::new();
    let mut roles: Vec<(String, String)> = Vec::new();
    for v in list.and_then(Value::as_array).into_iter().flatten().take(MAX_GOSSIP) {
        let Some(cert) = NameCert::from_json(&c.ch.keys.id, v) else { continue };
        if cert.k == me || !c.ch.knows(&cert.k) {
            continue; // our own names only come from us; others need a known machine
        }
        let mine = c.ch.names.get(&cert.n).filter(|x| x.k == me).cloned();
        let role_was = c.ch.names.get(&cert.n).filter(|x| x.k == cert.k && !x.d).map(|x| x.role.clone());
        match c.ch.names.offer(cert.clone(), &me) {
            Offer::Accepted => {
                if role_was.is_some_and(|r| r != cert.role) && !cert.d && !cert.r {
                    roles.push((cert.n.clone(), cert.role.clone()));
                }
                fresh.push(cert)
            }
            Offer::Lost => {
                if let Some(m) = mine {
                    lost.push(m);
                }
                fresh.push(cert);
            }
            Offer::Refused => {
                if let Some(owner) = c.claims.impersonation(&c.ch.names, &cert, &via, now) {
                    claimed.push((cert.n, owner));
                }
            }
            Offer::Same => {}
        }
    }
    if !claimed.is_empty() {
        let text = claim_alert(c, &via, &claimed);
        log!("{}: {}", c.ch.label(), text.trim_start_matches("⚠ "));
        st.note_all(cid, &text, true, true);
    }
    for (n, role) in roles {
        st.note_all(cid, &role_note(&n, &role), false, true);
    }
    if fresh.is_empty() {
        return;
    }
    let c = st.chans.get_mut(cid).unwrap();
    let label = c.ch.label();
    if c.ch.cfg.remove("knownNames").is_some() {
        c.ch.save_cfg();
    }
    let mut out: Vec<NameCert> = Vec::new();
    let mut alerts = Vec::new();
    for m in lost {
        // Theirs is older: ours is released, so the machines that saw ours first take theirs.
        out.push(c.ch.sign_name(NameCert { r: true, on: false, ..m.clone() }, now.max(m.u + 1)));
        log!("{label}: lost the name {} to an earlier creation on another machine", m.n);
        alerts.push(m.n.clone());
    }
    out.extend(fresh);
    c.ch.save_names();
    send_names(st, cid, &out, Some(from_conn));
    for n in alerts {
        st.note(cid, &n, &format!("⚠ {n} already existed in {label} on another machine · your messages no longer reach the others · choose another name: /denden:im join {label} as {n}-2"), true, false);
    }
    notify(st);
}

/// One line for the names (name, owner key) that machine `key` claimed from their owners.
fn claim_alert(c: &Chan, key: &str, claimed: &[(String, String)]) -> String {
    let fp = fmt_fp(&fingerprint_hex(key)[..8]);
    let mut names: Vec<&str> = claimed.iter().take(5).map(|(n, _)| n.as_str()).collect();
    let more = format!("{} more", claimed.len().saturating_sub(5));
    if claimed.len() > 5 {
        names.push(&more);
    }
    let mut owners: Vec<String> = Vec::new();
    for (_, k) in claimed {
        let o = if *k == c.ch.me { "this machine".to_string() } else { c.ch.members.display(k) };
        if !owners.contains(&o) {
            owners.push(o);
        }
    }
    let verb = if claimed.len() == 1 { "belongs" } else { "belong" };
    format!("⚠ {} [{fp}] claimed {}, which {verb} to {} in {}: refused", c.ch.members.name_of(key), names.join(", "), owners.join(", "), c.ch.label())
}

// ---------- frames ----------

fn on_refused(st: &mut State, conn: u64, f: &Value) {
    let Some(l) = st.live.get(&conn) else { return };
    let (cid, epoch, tpub) = (l.cid.clone(), l.epoch, hex::encode(l.remote_t));
    let err = f.get("error").map(js_string).unwrap_or_default();
    if let Some(c) = st.chans.get_mut(&cid) {
        c.refused_by.insert(tpub.clone(), epoch);
        c.last_refusal = Some((err.clone(), now_ms()));
        let why = match err.as_str() {
            "not-trusted" => "has not admitted us yet (an invitation or a certificate is needed)",
            "blocked" => "has blocked this machine",
            "self" => "uses our own machine key",
            _ => "refused the connection",
        };
        log!("{}: peer {} {why}", c.ch.label(), &tpub[..8]);
    }
}

fn on_cert(st: &mut State, conn: u64, key: &str, f: &Value, fx: &mut Vec<Effect>) {
    let Some(cid) = st.live.get(&conn).map(|l| l.cid.clone()) else { return };
    let Some(c) = st.chans.get_mut(&cid) else { return };
    if c.ch.is_member() {
        return;
    }
    let chain: Vec<Value> = f.get("chain").and_then(Value::as_array).cloned().unwrap_or_default().into_iter().take(8).collect();
    let Some(c0) = chain.first() else { return };
    if s(c0, "k") != c.ch.me || s(c0, "bk") != key || !cert_valid(&c.ch.keys.id, c0) {
        return;
    }
    c.ch.cfg.insert("chain".into(), Value::Array(chain));
    c.ch.cfg.remove("joinToken");
    c.ch.save_cfg();
    log!("{}: admitted (certificate from {})", c.ch.label(), c.ch.members.display(key));
    // Machines that refused us before: dial them again, we can show a certificate now.
    for (t, epoch) in std::mem::take(&mut c.refused_by) {
        let handle = c.swarms.get(&epoch).and_then(|s| s.handle.clone());
        if let (Some(h), Ok(k)) = (handle, <[u8; 32]>::try_from(hex::decode(&t).unwrap_or_default())) {
            fx.push(Effect::Redial(h, k));
        }
    }
}

fn take_token(st: &mut State, cid: &str, key: &str) -> bool {
    let Some(c) = st.chans.get_mut(cid) else { return false };
    let now = now_ms();
    let b = c.buckets.entry(key.to_string()).or_insert((RATE_PER_MIN, now));
    b.0 = RATE_PER_MIN.min(b.0 + (now - b.1) as f64 * RATE_PER_MIN / 60000.0);
    b.1 = now;
    if b.0 < 1.0 {
        return false;
    }
    b.0 -= 1.0;
    true
}

fn on_msg(st: &mut State, conn: u64, cid: &str, key: &str, f: &Value) {
    let fid = f.get("id").cloned().unwrap_or(Value::Null);
    let nack = |st: &State, e: &str| send_to(st, conn, &json!({ "t": "nack", "id": fid, "error": e }));
    let c = &st.chans[cid];
    let from = s(f, "from").to_string();
    if c.ch.names.owner(&from) != Some(key) {
        return nack(st, "bad-sender");
    }
    if !take_token(st, cid, key) {
        return nack(st, "rate-limited");
    }
    let text = f.get("text").map(js_string).unwrap_or_default();
    if js_len(&text) > MAX_TEXT {
        return nack(st, "too-long");
    }
    let c = &st.chans[cid];
    let me = c.ch.me.clone();
    let to = f.get("to").cloned().unwrap_or(Value::Null);
    let recipients: Vec<String> = match &to {
        Value::String(t) if t == "all" => c.ch.my_names(),
        Value::Array(a) => a.iter().filter_map(Value::as_str).filter(|n| c.ch.names.owner(n) == Some(me.as_str())).map(str::to_string).collect(),
        _ => Vec::new(),
    };
    if recipients.is_empty() && to.as_str() != Some("all") {
        return nack(st, "no-recipient");
    }
    send_to(st, conn, &json!({ "t": "ack", "id": fid }));
    let id: String = js_string(&fid).chars().take(32).collect();
    let now = now_ms();
    let c = st.chans.get_mut(cid).unwrap();
    let mut got = 0;
    for d in recipients {
        if c.ch.log.has(&from, &id, &d) {
            continue;
        }
        let mut m = Map::new();
        for (k, v) in [("id", json!(id)), ("dir", json!("in")), ("d", json!(d)), ("from", json!(from)), ("to", to.clone()), ("text", json!(text)), ("ts", f.get("ts").cloned().unwrap_or(json!(now))), ("at", json!(now))] {
            m.insert(k.into(), v);
        }
        m.insert("waited".into(), json!(f.get("waited").and_then(Value::as_u64).unwrap_or(0)));
        if truthy(f.get("ask")) {
            m.insert("ask".into(), json!(true));
        }
        if let Some(re) = f.get("re").and_then(Value::as_str) {
            m.insert("re".into(), json!(re.chars().take(32).collect::<String>()));
        }
        c.ch.record(m);
        got += 1;
    }
    if got > 0 {
        log!("{}: message from {from}", c.ch.label());
        notify(st);
    }
}

fn on_ack(st: &mut State, cid: &str, key: &str, f: &Value, ok: bool) {
    let id = f.get("id").map(js_string).unwrap_or_default();
    let Some(c) = st.chans.get_mut(cid) else { return };
    let entry = c.ch.outbox.iter().find(|e| s(e, "id") == id && s(e, "mk") == key).cloned();
    c.ch.outbox.retain(|e| !(s(e, "id") == id && s(e, "mk") == key));
    if entry.is_some() {
        c.ch.save_outbox();
    }
    let err = f.get("error").map(js_string).unwrap_or_else(|| "refused".into());
    if !ok {
        let machine = c.ch.members.display(key);
        log!("{}: {machine} refused message {id}: {err}", c.ch.label());
        if let Some(e) = &entry {
            let why = match err.as_str() {
                "no-recipient" => "nobody there by that name any more".to_string(),
                "rate-limited" => "too many messages at once".to_string(),
                "too-long" => "too long".to_string(),
                other => other.to_string(),
            };
            let (from, text) = (s(e, "from").to_string(), s(e, "text").chars().take(80).collect::<String>());
            st.note(cid, &from, &format!("{machine} refused your message ({why}): \"{text}\""), false, false);
        }
    }
    if let Some(tx) = st.ack_waiters.remove(&format!("{id}:{key}")) {
        let _ = tx.send(if ok { AckResult::Ok } else { AckResult::Refused(err) });
    }
}

fn on_frame(st: &mut State, conn: u64, f: &Value, fx: &mut Vec<Effect>) {
    let Some(l) = st.live.get(&conn) else { return };
    let (cid, authed, key, local_t, remote_t, epoch) = (l.cid.clone(), l.authed, l.key.clone(), l.local_t, l.remote_t, l.epoch);
    let Some(c) = st.chans.get_mut(&cid) else { return kill(st, conn, fx) };
    let t = s(f, "t");
    if !authed {
        // Only someone who knows the channel secret can compute this, and it is bound to this stream.
        let want = b64u(&hmac(&c.ch.keys.auth, &frame(&[b"auth", &remote_t, &local_t])));
        let proof = f.get("proof").map(js_string).unwrap_or_default();
        if t != "auth" || !safe_eq(&proof, &want) {
            let tpub = hex::encode(remote_t);
            if c.banned_t.insert(tpub.clone()) {
                log!("banned transport key {}: no valid channel proof", &tpub[..8]);
            }
            if let Some(h) = c.swarms.get(&epoch).and_then(|s| s.handle.clone()) {
                fx.push(Effect::Ban(h, remote_t));
            }
            return kill(st, conn, fx);
        }
        if let Some(l) = st.live.get_mut(&conn) {
            l.authed = true;
        }
        return send_hello(st, conn);
    }
    if t == "error" {
        return on_refused(st, conn, f);
    }
    let Some(key) = key else {
        if t == "hello" {
            on_hello(st, conn, f, fx);
        }
        return;
    };
    match t {
        "msg" => on_msg(st, conn, &cid, &key, f),
        "ack" => on_ack(st, &cid, &key, f, true),
        "nack" => on_ack(st, &cid, &key, f, false),
        "cert" => on_cert(st, conn, &key, f, fx),
        "names" => on_names(st, &cid, conn, f.get("names")),
        _ => {}
    }
}

async fn on_connection(shared: Shared, cid: String, conn: SwarmConnection, epoch: u64, local_t: [u8; 32]) {
    let remote_t = *conn.remote_public_key();
    let tpub = hex::encode(remote_t);
    if shared.lock().unwrap().chans.get(&cid).is_none_or(|c| c.banned_t.contains(&tpub)) {
        return;
    }
    // The stream moves out; the rest of the connection (its close guard included) lives until this
    // function returns, which tells the swarm the connection is over.
    let stream = conn.peer.stream;
    let (mut rd, mut wr) = stream.into_split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let killer = Arc::new(Notify::new());
    let writer_kill = killer.clone();
    let writer = tokio::spawn(async move {
        // A keepalive: an empty message, which both sides drop. It also gives UDX unacknowledged
        // data, the only way it notices a peer that went away.
        let mut keepalive = tokio::time::interval(Duration::from_secs(5));
        keepalive.tick().await;
        loop {
            let data = tokio::select! {
                line = rx.recv() => match line {
                    Some(l) => l.into_bytes(),
                    None => break,
                },
                _ = keepalive.tick() => Vec::new(),
            };
            if wr.write(&data).await.is_err() {
                writer_kill.notify_one();
                break;
            }
        }
        let _ = wr.shutdown().await;
    });

    let id = {
        let mut st = shared.lock().unwrap();
        st.next_id += 1;
        let id = st.next_id;
        let auth = st.chans[&cid].ch.keys.auth;
        st.live.insert(
            id,
            Live { cid: cid.clone(), key: None, send: tx.clone(), kill: killer.clone(), epoch, local_t, remote_t, authed: false, window_start: now_ms(), frame_count: 0, shown: Vec::new(), hello_at: 0 },
        );
        let proof = b64u(&hmac(&auth, &frame(&[b"auth", &local_t, &remote_t])));
        send_to(&st, id, &json!({ "t": "auth", "v": PROTO_VERSION, "proof": proof }));
        id
    };
    drop(tx);

    let admit_deadline = tokio::time::sleep(Duration::from_secs(20));
    tokio::pin!(admit_deadline);
    let mut admitted = false;
    let mut buf: Vec<u8> = Vec::new();
    loop {
        tokio::select! {
            r = rd.read() => {
                let data = match r {
                    Ok(Some(d)) => d,
                    _ => break,
                };
                buf.extend_from_slice(&data);
                if buf.len() > MAX_LINE {
                    break;
                }
                let mut fx = Vec::new();
                let mut drop_conn = false;
                {
                    let mut st = shared.lock().unwrap();
                    while let Some(i) = buf.iter().position(|&b| b == b'\n') {
                        let line: Vec<u8> = buf.drain(..=i).collect();
                        let now = now_ms();
                        let over = match st.live.get_mut(&id) {
                            Some(l) => {
                                if now - l.window_start > 10000 {
                                    l.window_start = now;
                                    l.frame_count = 0;
                                }
                                l.frame_count += 1;
                                l.frame_count > FRAME_LIMIT
                            }
                            None => true,
                        };
                        if over {
                            log!("dropping {}: too many frames", &tpub[..8]);
                            drop_conn = true;
                            break;
                        }
                        let Ok(f) = serde_json::from_slice::<Value>(&line[..line.len() - 1]) else { continue };
                        if f.is_object() {
                            on_frame(&mut st, id, &f, &mut fx);
                        }
                    }
                    if !admitted && st.live.get(&id).is_some_and(|l| l.key.is_some()) {
                        admitted = true;
                    }
                }
                run_effects(fx);
                if drop_conn {
                    break;
                }
            }
            _ = killer.notified() => break,
            _ = &mut admit_deadline, if !admitted => break,
        }
    }

    // Closed: the machine goes offline unless another connection carries it.
    {
        let mut st = shared.lock().unwrap();
        let key = st.live.remove(&id).and_then(|l| l.key);
        if let Some(key) = key {
            let other = st.live.iter().find(|(_, l)| l.cid == cid && l.key.as_deref() == Some(key.as_str())).map(|(i, _)| *i);
            if let Some(c) = st.chans.get_mut(&cid) {
                if c.peers.get(&key) == Some(&id) {
                    match other {
                        Some(o) => {
                            c.peers.insert(key.clone(), o);
                        }
                        None => {
                            c.peers.remove(&key);
                            if let Some(m) = c.ch.members.machines.get_mut(&key) {
                                m.seen = now_ms();
                            }
                            c.ch.save_members();
                            log!("{}: offline: {}", c.ch.label(), c.ch.members.display(&key));
                        }
                    }
                }
            }
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
    drop(rd);
}

// ---------- discovery: one swarm per channel and epoch, overlapping around the change ----------

fn current_epoch() -> u64 {
    now_ms() / epoch_ms()
}

fn wanted_epochs() -> Vec<u64> {
    let now = now_ms();
    let e = now / epoch_ms();
    let into = now - e * epoch_ms();
    let mut want = vec![e];
    if into < grace_ms() && e > 0 {
        want.push(e - 1);
    }
    if epoch_ms() - into < grace_ms() {
        want.push(e + 1);
    }
    want
}

/// Starts the swarms each channel needs now and drops the ones of past epochs.
pub fn reconcile(shared: &Shared) {
    let want = wanted_epochs();
    let mut st = shared.lock().unwrap();
    if st.stopping {
        return;
    }
    let mut starts = Vec::new();
    for (cid, c) in st.chans.iter_mut() {
        for &e in &want {
            if c.swarms.contains_key(&e) {
                continue;
            }
            let generation = GENERATION.fetch_add(1, Ordering::Relaxed);
            c.swarms.insert(e, SwarmSlot { generation, handle: None });
            starts.push((cid.clone(), e, generation, c.ch.seed(), c.ch.keys.topic(e)));
        }
        let gone: Vec<u64> = c.swarms.keys().copied().filter(|e| !want.contains(e)).collect();
        for e in gone {
            if let Some(slot) = c.swarms.remove(&e) {
                if let Some(h) = slot.handle {
                    tokio::spawn(async move {
                        let _ = h.destroy().await;
                    });
                }
                log!("{}: epoch {e} left", c.ch.label());
            }
        }
    }
    drop(st);
    for (cid, e, generation, seed, topic) in starts {
        tokio::spawn(start_swarm(shared.clone(), cid, e, generation, seed, topic));
    }
}

fn slot_is(st: &State, cid: &str, e: u64, generation: u64) -> bool {
    st.chans.get(cid).and_then(|c| c.swarms.get(&e)).is_some_and(|s| s.generation == generation)
}

async fn start_swarm(shared: Shared, cid: String, e: u64, generation: u64, seed: String, topic: [u8; 32]) {
    let kp = KeyPair::from_seed(transport_seed(&seed, e));
    let local_t = kp.public_key;
    let mut config = SwarmConfig::with_public_bootstrap();
    config.key_pair = Some(kp);
    let label = shared.lock().unwrap().chans.get(&cid).map(|c| c.ch.label()).unwrap_or_default();
    let (_task, handle, mut conn_rx) = match spawn(config).await {
        Ok(s) => s,
        Err(err) => {
            log!("{label}: epoch {e}: announce failed: {err}");
            let mut st = shared.lock().unwrap();
            if slot_is(&st, &cid, e, generation) {
                st.chans.get_mut(&cid).unwrap().swarms.remove(&e); // the next reconcile tries again
            }
            return;
        }
    };
    let still_wanted = {
        let mut st = shared.lock().unwrap();
        let stopping = st.stopping;
        match st.chans.get_mut(&cid).and_then(|c| c.swarms.get_mut(&e)) {
            Some(slot) if slot.generation == generation && !stopping => {
                slot.handle = Some(handle.clone());
                true
            }
            _ => false,
        }
    };
    if !still_wanted {
        let _ = handle.destroy().await;
        return;
    }
    {
        let (sh, cid) = (shared.clone(), cid.clone());
        tokio::spawn(async move {
            while let Some(conn) = conn_rx.recv().await {
                tokio::spawn(on_connection(sh.clone(), cid.clone(), conn, e, local_t));
            }
        });
    }
    let _ = handle.join(topic, JoinOpts::default()).await;
    log!("{label}: epoch {e}: joined topic {}, transport key {}", &hex::encode(topic)[..8], short_fp(&local_t));
    match handle.flush().await {
        Ok(()) => {
            {
                let mut st = shared.lock().unwrap();
                if e == current_epoch() {
                    if let Some(c) = st.chans.get_mut(&cid) {
                        c.announced = true;
                    }
                }
            }
            log!("{label}: epoch {e}: announced on the DHT");
            // Peers that started at the same moment announced after our first lookup, and the swarm
            // only looks again every 10 minutes: look again a few times now.
            for ms in [5000u64, 20000, 60000, 180000] {
                let (sh, h, cid) = (shared.clone(), handle.clone(), cid.clone());
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    if slot_is(&sh.lock().unwrap(), &cid, e, generation) {
                        let _ = h.refresh(topic).await;
                    }
                });
            }
        }
        Err(err) => log!("{label}: epoch {e}: announce failed: {err}"),
    }
}
