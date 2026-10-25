// What machines of a channel prove to each other: invitation codes, membership certificates (a
// member vouches for a machine key), and name certificates (a machine owns a denden name). Pure
// functions over JSON values, so the daemon's rules can be tested without a network.

use std::collections::{BTreeMap, BTreeSet};

use ed25519_dalek::SigningKey;
use serde_json::{Value, json};

use crate::core::*;

pub const PROTO_VERSION: u64 = 3;
/// An invitation code is valid this long, or up to INVITE_MAX when asked; the local page shows it
/// for INVITE_PAGE.
pub const INVITE_TTL: u64 = 30 * 60 * 1000;
pub const INVITE_MAX: u64 = 24 * 3600 * 1000;
pub const INVITE_PAGE: u64 = 15 * 60 * 1000;
const MAX_CHAIN: usize = 8;
const MAX_INVITE_LIST: usize = 200;

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

// ---------- invitation codes: denden_<base64url(JSON + 4-byte checksum)> ----------

/// What a code carries: the channel secret, the inviter's machine, a one-time token, and what the
/// inviter knows of the channel (machines and denden names), so the new machine starts with the
/// same trust anchors and can refuse a taken name before it even connects.
#[derive(Clone, Debug, PartialEq)]
pub struct Invite {
    pub secret: String,
    pub label: Option<String>,
    pub inviter: String,
    pub inviter_key: String,
    pub token: String,
    pub expires: u64,
    pub machines: Vec<(String, String)>,
    pub names: Vec<(String, String)>,
}

fn pairs(list: &[(String, String)]) -> Vec<Value> {
    list.iter().take(MAX_INVITE_LIST).map(|(n, k)| json!([n, b64u(&hex::decode(k).unwrap_or_default())])).collect()
}

fn unpairs(v: Option<&Value>) -> Vec<(String, String)> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    let e = e.as_array()?;
                    let n = e.first()?.as_str()?;
                    let k = unb64u(&e.get(1).map(js_string).unwrap_or_default());
                    (valid_name(n) && k.len() == 32).then(|| (n.to_string(), hex::encode(k)))
                })
                .take(MAX_INVITE_LIST)
                .collect()
        })
        .unwrap_or_default()
}

pub fn encode_invite(inv: &Invite) -> String {
    let body = serde_json::to_vec(&json!({
        "v": PROTO_VERSION,
        "c": b64u(&hex::decode(&inv.secret).unwrap_or_default()),
        "l": inv.label,
        "n": inv.inviter,
        "k": b64u(&hex::decode(&inv.inviter_key).unwrap_or_default()),
        "t": b64u(&hex::decode(&inv.token).unwrap_or_default()),
        "x": inv.expires,
        "m": pairs(&inv.machines),
        "d": pairs(&inv.names),
    }))
    .unwrap_or_default();
    let mut raw = body.clone();
    raw.extend_from_slice(&sha256(&body)[..4]);
    format!("denden_{}", b64u(&raw))
}

pub fn decode_invite(code: &str) -> Result<Invite, String> {
    let clean: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    let rest = clean
        .strip_prefix("denden_")
        .filter(|r| !r.is_empty() && r.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-'))
        .ok_or("this is not a denden invitation code (it starts with denden_)")?;
    let raw = unb64u(rest);
    if raw.len() < 8 || sha256(&raw[..raw.len() - 4])[..4] != raw[raw.len() - 4..] {
        return Err("the invitation code is damaged: copy it again".into());
    }
    let j: Value = serde_json::from_slice(&raw[..raw.len() - 4]).map_err(|_| "the invitation code is damaged".to_string())?;
    if j.get("v").and_then(Value::as_u64) != Some(PROTO_VERSION) {
        return Err("this invitation code comes from another version of denden".into());
    }
    let field = |k: &str| unb64u(&j.get(k).map(js_string).unwrap_or_default());
    let (secret, key, token) = (field("c"), field("k"), field("t"));
    let inviter = s(&j, "n");
    if secret.len() != 32 || key.len() != 32 || token.len() != 16 || !valid_name(inviter) {
        return Err("the invitation code is malformed".into());
    }
    Ok(Invite {
        secret: hex::encode(secret),
        label: j.get("l").and_then(Value::as_str).filter(|l| valid_name(l)).map(str::to_string),
        inviter: inviter.to_string(),
        inviter_key: hex::encode(key),
        token: hex::encode(token),
        expires: j.get("x").and_then(Value::as_u64).unwrap_or(0),
        machines: unpairs(j.get("m")),
        names: unpairs(j.get("d")),
    })
}

/// The proof a joining machine puts in its hello: bound to the stream and to its machine key.
pub fn join_proof(token_hex: &str, channel_id: &[u8], local_t: &[u8], remote_t: &[u8], key_hex: &str) -> String {
    let tok = hex::decode(token_hex).unwrap_or_default();
    b64u(&hmac(&tok, &frame(&[b"join", channel_id, local_t, remote_t, &hex::decode(key_hex).unwrap_or_default()])))
}

// ---------- membership certificates: "machine b (key bk) vouches that machine n owns key k" ----------

fn cert_data(channel_id: &[u8], n: &str, k: &str, b: &str, bk: &str) -> Vec<u8> {
    frame(&[b"cert", channel_id, n.as_bytes(), &hex::decode(k).unwrap_or_default(), b.as_bytes(), &hex::decode(bk).unwrap_or_default()])
}

pub fn make_cert(channel_id: &[u8], sk: &SigningKey, by_name: &str, by_key_hex: &str, name: &str, key_hex: &str) -> Value {
    let sig = b64u(&sign(sk, &cert_data(channel_id, name, key_hex, by_name, by_key_hex)));
    json!({ "n": name, "k": key_hex, "b": by_name, "bk": by_key_hex, "s": sig })
}

pub fn cert_valid(channel_id: &[u8], c: &Value) -> bool {
    let (n, k, b, bk, sig) = (s(c, "n"), s(c, "k"), s(c, "b"), s(c, "bk"), s(c, "s"));
    valid_name(n) && valid_name(b) && is_hex64(k) && is_hex64(bk) && verify_sig(&hex::decode(bk).unwrap_or_default(), &unb64u(sig), &cert_data(channel_id, n, k, b, bk))
}

/// A chain [cert(key by X), cert(X by Y), ...] is accepted as soon as a link is signed by a machine
/// key known here. Returns the trusted signer's key and the (name, key) pairs the chain vouches for.
pub fn verify_chain(channel_id: &[u8], chain: Option<&Value>, key_hex: &str, known: &dyn Fn(&str) -> bool) -> Option<(String, Vec<(String, String)>)> {
    let chain = chain?.as_array()?;
    if chain.is_empty() || chain.len() > MAX_CHAIN || s(&chain[0], "k") != key_hex {
        return None;
    }
    for (i, c) in chain.iter().enumerate() {
        if !cert_valid(channel_id, c) {
            return None;
        }
        if known(s(c, "bk")) {
            let learned = chain[..=i].iter().map(|x| (s(x, "n").to_string(), s(x, "k").to_string())).collect();
            return Some((s(c, "bk").to_string(), learned));
        }
        if let Some(next) = chain.get(i + 1) {
            if s(next, "k") != s(c, "bk") {
                return None;
            }
        }
    }
    None
}

// ---------- name certificates: "machine k owns the denden name n, created at t" ----------

/// A denden name and what its machine says about it, signed by that machine. `t` is when the name
/// was created (it settles conflicts); `u` orders the states of one name (role changed, attached to a
/// conversation or not, deleted).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct NameCert {
    pub n: String,
    pub k: String,
    pub t: u64,
    pub u: u64,
    /// The application of the conversation it is attached to ("claude", "codex"), if any.
    pub app: String,
    pub role: String,
    /// Attached to a conversation.
    pub on: bool,
    /// Deleted: no longer used, but still bound to its machine.
    pub d: bool,
    /// Released after losing a conflict: the binding is gone, another machine may hold the name.
    pub r: bool,
    pub sig: String,
}

pub const MAX_ROLE: usize = 80;

fn name_data(channel_id: &[u8], c: &NameCert) -> Vec<u8> {
    let flags = format!("{}{}{}", if c.on { "o" } else { "" }, if c.d { "d" } else { "" }, if c.r { "r" } else { "" });
    frame(&[
        b"name",
        channel_id,
        c.n.as_bytes(),
        &hex::decode(&c.k).unwrap_or_default(),
        c.t.to_string().as_bytes(),
        c.u.to_string().as_bytes(),
        c.app.as_bytes(),
        c.role.as_bytes(),
        flags.as_bytes(),
    ])
}

impl NameCert {
    /// A new name of ours, created now.
    pub fn new(n: &str, key_hex: &str, now: u64) -> NameCert {
        NameCert { n: n.to_string(), k: key_hex.to_string(), t: now, u: now, ..Default::default() }
    }
    /// Signs this state (with `u` moved past the previous one).
    pub fn signed(mut self, channel_id: &[u8], sk: &SigningKey, now: u64) -> NameCert {
        self.u = self.u.max(now).max(self.t);
        self.role = self.role.chars().take(MAX_ROLE).collect();
        self.sig = b64u(&sign(sk, &name_data(channel_id, &self)));
        self
    }
    pub fn to_json(&self) -> Value {
        let mut v = json!({ "n": self.n, "k": self.k, "t": self.t, "u": self.u, "s": self.sig });
        if !self.app.is_empty() {
            v["app"] = json!(self.app);
        }
        if !self.role.is_empty() {
            v["role"] = json!(self.role);
        }
        for (f, on) in [("on", self.on), ("d", self.d), ("r", self.r)] {
            if on {
                v[f] = json!(true);
            }
        }
        v
    }
    pub fn from_json(channel_id: &[u8], v: &Value) -> Option<NameCert> {
        let c = NameCert {
            n: s(v, "n").to_string(),
            k: s(v, "k").to_string(),
            t: v.get("t").and_then(Value::as_u64)?,
            u: v.get("u").and_then(Value::as_u64)?,
            app: s(v, "app").chars().take(16).collect(),
            role: s(v, "role").chars().take(MAX_ROLE).collect(),
            on: truthy(v.get("on")),
            d: truthy(v.get("d")),
            r: truthy(v.get("r")),
            sig: s(v, "s").to_string(),
        };
        if !valid_name(&c.n) || is_broadcast(&c.n) || !is_hex64(&c.k) || c.u < c.t {
            return None;
        }
        verify_sig(&hex::decode(&c.k).unwrap_or_default(), &unb64u(&c.sig), &name_data(channel_id, &c)).then_some(c)
    }
    /// In use: neither deleted nor released.
    pub fn active(&self) -> bool {
        !self.d && !self.r
    }
}

/// What happened to a name certificate offered to the table.
#[derive(Debug, PartialEq)]
pub enum Offer {
    /// New, or a newer state of a name we had for that same machine.
    Accepted,
    /// Nothing new.
    Same,
    /// Bound to another machine here: refused.
    Refused,
    /// One of our own names, created earlier by another machine: it is theirs now. We must release
    /// it (a released certificate of ours) and tell the conversation that held it.
    Lost,
}

/// Who owns which denden name in a channel, as this machine sees it. A name is bound to the first
/// machine seen with it and stays so, deleted or not, until that machine releases it after losing a
/// conflict. When the conflict is over one of our own names, the earliest creation wins.
#[derive(Default, Debug, Clone)]
pub struct Names {
    pub map: BTreeMap<String, NameCert>,
}

impl Names {
    pub fn get(&self, n: &str) -> Option<&NameCert> {
        self.map.get(n)
    }
    /// The machine key that owns `n` and uses it.
    pub fn owner(&self, n: &str) -> Option<&str> {
        self.map.get(n).filter(|c| c.active()).map(|c| c.k.as_str())
    }
    pub fn active_of<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a NameCert> + 'a {
        self.map.values().filter(move |c| c.k == key && c.active())
    }
    /// Whether this machine may take `n`: free, or already bound to this machine.
    pub fn free_for(&self, n: &str, my_key: &str) -> bool {
        self.map.get(n).is_none_or(|c| c.k == my_key)
    }

    pub fn offer(&mut self, c: NameCert, my_key: &str) -> Offer {
        let Some(cur) = self.map.get(&c.n) else {
            if c.r {
                return Offer::Same; // releasing a binding we never had
            }
            self.map.insert(c.n.clone(), c);
            return Offer::Accepted;
        };
        if cur.k == c.k {
            if c.u <= cur.u {
                return Offer::Same;
            }
            if c.r {
                self.map.remove(&c.n);
            } else {
                self.map.insert(c.n.clone(), c);
            }
            return Offer::Accepted;
        }
        if c.r || c.d {
            return Offer::Refused;
        }
        // Another machine claims a name bound here. Ours, still in use, and created after theirs:
        // theirs wins (ties go to the smaller key, the same answer on both sides).
        if cur.k == my_key && cur.active() && (c.t, c.k.as_str()) < (cur.t, cur.k.as_str()) {
            self.map.insert(c.n.clone(), c);
            return Offer::Lost;
        }
        Offer::Refused
    }

    pub fn to_json(&self) -> Value {
        Value::Array(self.map.values().map(NameCert::to_json).collect())
    }
    pub fn from_json(channel_id: &[u8], v: Option<&Value>) -> Names {
        let mut names = Names::default();
        for c in v.and_then(Value::as_array).into_iter().flatten().filter_map(|c| NameCert::from_json(channel_id, c)) {
            names.map.insert(c.n.clone(), c);
        }
        names
    }
}

// ---------- names claimed from their machine ----------

/// A claim that reaches us this long after we showed its machine the owner's certificate did not
/// cross it: that machine knew whose name it was.
pub const KNEW_AFTER: u64 = 60 * 1000;

/// Impersonation evidence in one channel, for the daemon's run. A machine that created a name
/// without seeing it taken gives it up as soon as it sees an earlier creation. So a machine that,
/// on its own connection, still claims a name created before its own, after we showed it the owner,
/// is impersonating that owner. Claims that win the conflict rule, crossings, claims relayed by
/// another machine and names nobody uses any more are not reported.
#[derive(Default, Debug)]
pub struct Claims {
    /// (machine key, name) -> (owner key, when we first showed that machine this binding).
    shown: BTreeMap<(String, String), (String, u64)>,
    /// (machine key, name) already reported.
    reported: BTreeSet<(String, String)>,
}

impl Claims {
    /// We showed machine `to` that the name `n` is in use by machine `owner` (our hello, or a name
    /// certificate sent on to the machines online).
    pub fn showed(&mut self, to: &str, n: &str, owner: &str, at: u64) {
        if to == owner {
            return;
        }
        let e = self.shown.entry((to.to_string(), n.to_string())).or_insert_with(|| (owner.to_string(), at));
        if e.0 != owner {
            *e = (owner.to_string(), at);
        }
    }

    /// A claim refused by the table, received from machine `via` at `now`: the owner's key when it
    /// is impersonation reported for the first time.
    pub fn impersonation(&mut self, names: &Names, claim: &NameCert, via: &str, now: u64) -> Option<String> {
        let owner = names.get(&claim.n).filter(|o| o.active() && o.k != claim.k)?;
        let earlier = (owner.t, owner.k.as_str()) < (claim.t, claim.k.as_str());
        let id = (claim.k.clone(), claim.n.clone());
        let knew = self.shown.get(&id).is_some_and(|(k, at)| *k == owner.k && now >= at.saturating_add(KNEW_AFTER));
        if via != claim.k || !claim.active() || !earlier || !knew || !self.reported.insert(id) {
            return None;
        }
        Some(owner.k.clone())
    }
}

// ---------- late messages ----------

/// A message handed to a conversation this long after it was first sent is late: never acted on
/// without the user's go-ahead, since what it asks may no longer apply.
pub const LATE_AFTER: u64 = 3600 * 1000;

/// LATE_AFTER, or DENDEN_LATE_AFTER_SECONDS (tests only, like DENDEN_EPOCH_SECONDS).
pub fn late_after() -> u64 {
    static LATE: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *LATE.get_or_init(|| {
        std::env::var("DENDEN_LATE_AFTER_SECONDS").ok().and_then(|s| s.trim().parse::<u64>().ok()).filter(|s| *s > 0).map(|s| s * 1000).unwrap_or(LATE_AFTER)
    })
}

/// `waited`: what the sender reports it held the message; then the time it sat here before a
/// conversation got it. Each side measures with its own clock.
pub fn is_late(waited: u64, received_at: u64, handed_at: u64) -> bool {
    waited.saturating_add(handed_at.saturating_sub(received_at)) >= late_after()
}


#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> (SigningKey, String) {
        let sk = SigningKey::from_bytes(&[seed; 32]);
        let k = hex::encode(sk.verifying_key().to_bytes());
        (sk, k)
    }
    const CH: &[u8] = &[7u8; 32];

    /// Name `n` of machine (sk, k), created at t, in the state of time u.
    fn nc(sk: &SigningKey, k: &str, n: &str, t: u64, u: u64, d: bool, r: bool) -> NameCert {
        NameCert { d, r, ..NameCert::new(n, k, t) }.signed(CH, sk, u)
    }

    #[test]
    fn invite_round_trip() {
        let (_, k) = key(1);
        let (_, k2) = key(2);
        let inv = Invite {
            secret: "ab".repeat(32),
            label: Some("vibedoc-chan".into()),
            inviter: "laptop-ak".into(),
            inviter_key: k.clone(),
            token: "cd".repeat(16),
            expires: 1234,
            machines: vec![("srv-api".into(), k2.clone())],
            names: vec![("coord".into(), k.clone()), ("api".into(), k2)],
        };
        let code = encode_invite(&inv);
        assert!(code.starts_with("denden_"));
        assert!(has_invite(&code));
        assert_eq!(decode_invite(&code).unwrap(), inv);
        assert_eq!(decode_invite(&format!("  {code}\n")).unwrap(), inv, "whitespace around a pasted code");
        let mut bad = code.clone();
        let last = bad.pop().unwrap();
        bad.push(if last == 'A' { 'B' } else { 'A' });
        assert!(decode_invite(&bad).is_err());
        assert!(decode_invite("imv2_abc").is_err());
    }

    #[test]
    fn chains_reach_a_known_key() {
        let (a_sk, a) = key(1);
        let (b_sk, b) = key(2);
        let (_, c) = key(3);
        let b_by_a = make_cert(CH, &a_sk, "laptop", &a, "srv-api", &b);
        let c_by_b = make_cert(CH, &b_sk, "srv-api", &b, "srv-db", &c);
        let chain = json!([c_by_b, b_by_a]);
        let knows_a = |k: &str| k == a;
        let (signer, learned) = verify_chain(CH, Some(&chain), &c, &knows_a).unwrap();
        assert_eq!(signer, a);
        assert_eq!(learned, vec![("srv-db".to_string(), c.clone()), ("srv-api".to_string(), b.clone())]);
        assert!(verify_chain(CH, Some(&chain), &b, &knows_a).is_none(), "the chain is for c, not b");
        assert!(verify_chain(CH, Some(&chain), &c, &|_| false).is_none(), "no known signer");
        assert!(verify_chain(&[8u8; 32], Some(&chain), &c, &knows_a).is_none(), "another channel");
        let broken = json!([c_by_b, make_cert(CH, &a_sk, "laptop", &a, "other", &c)]);
        assert!(verify_chain(CH, Some(&broken), &c, &knows_a).is_none(), "links must follow each other");
    }

    #[test]
    fn name_certs_are_signed() {
        let (sk, k) = key(1);
        let c = NameCert { app: "claude".into(), role: "frontend React".into(), on: true, ..NameCert::new("front", &k, 100) }.signed(CH, &sk, 150);
        assert_eq!(c.u, 150);
        assert_eq!(NameCert::from_json(CH, &c.to_json()), Some(c.clone()));
        let tamper = |f: &str, v: Value| {
            let mut j = c.to_json();
            j[f] = v;
            NameCert::from_json(CH, &j)
        };
        assert_eq!(tamper("t", json!(50)), None, "an earlier creation needs a new signature");
        assert_eq!(tamper("d", json!(true)), None, "flags are signed");
        assert_eq!(tamper("role", json!("admin")), None, "the role is signed");
        assert_eq!(NameCert::from_json(CH, &NameCert::new("all", &k, 1).signed(CH, &sk, 1).to_json()), None, "reserved");
        let long = NameCert { role: "x".repeat(500), ..NameCert::new("db", &k, 1) }.signed(CH, &sk, 1);
        assert_eq!(long.role.len(), MAX_ROLE);
    }

    #[test]
    fn first_seen_wins_and_names_stay_bound() {
        let (a_sk, a) = key(1);
        let (b_sk, b) = key(2);
        let (_, me) = key(9);
        let mut names = Names::default();
        assert_eq!(names.offer(nc(&a_sk, &a, "alice", 200, 200, false, false), &me), Offer::Accepted);
        // b created it earlier, but we saw a first and neither is ours: a keeps it here.
        assert_eq!(names.offer(nc(&b_sk, &b, "alice", 100, 100, false, false), &me), Offer::Refused);
        assert_eq!(names.owner("alice"), Some(a.as_str()));
        // Deleted: still bound to a, not in use.
        assert_eq!(names.offer(nc(&a_sk, &a, "alice", 200, 300, true, false), &me), Offer::Accepted);
        assert_eq!(names.owner("alice"), None);
        assert!(!names.free_for("alice", &b));
        assert!(names.free_for("alice", &a), "its own machine may use it again");
        assert_eq!(names.offer(nc(&b_sk, &b, "alice", 400, 400, false, false), &me), Offer::Refused);
        // An older state of the same name changes nothing.
        assert_eq!(names.offer(nc(&a_sk, &a, "alice", 200, 250, false, false), &me), Offer::Same);
        // Released by its machine: the name is free for the other one.
        assert_eq!(names.offer(nc(&a_sk, &a, "alice", 200, 500, false, true), &me), Offer::Accepted);
        assert!(names.get("alice").is_none());
        assert_eq!(names.offer(nc(&b_sk, &b, "alice", 100, 100, false, false), &me), Offer::Accepted);
        assert_eq!(names.owner("alice"), Some(b.as_str()));
    }

    #[test]
    fn our_name_goes_to_the_earliest_creation() {
        let (me_sk, me) = key(9);
        let (b_sk, b) = key(2);
        let mut names = Names::default();
        names.offer(nc(&me_sk, &me, "alice", 200, 200, false, false), &me);
        assert_eq!(names.offer(nc(&b_sk, &b, "alice", 300, 300, false, false), &me), Offer::Refused, "theirs is later");
        assert_eq!(names.offer(nc(&b_sk, &b, "alice", 100, 100, false, false), &me), Offer::Lost, "theirs is earlier");
        assert_eq!(names.owner("alice"), Some(b.as_str()));
        // A name we deleted stays ours whatever its date.
        let mut names = Names::default();
        names.offer(nc(&me_sk, &me, "bob", 200, 200, true, false), &me);
        assert_eq!(names.offer(nc(&b_sk, &b, "bob", 100, 100, false, false), &me), Offer::Refused);
    }

    #[test]
    fn a_claim_from_a_machine_that_knew_is_reported_once() {
        let (a_sk, a) = key(1);
        let (b_sk, b) = key(2);
        let (me_sk, me) = key(9);
        let mut names = Names::default();
        names.offer(nc(&a_sk, &a, "alice", 100, 100, false, false), &me);
        names.offer(nc(&me_sk, &me, "coord", 100, 100, false, false), &me);
        let mut claims = Claims::default();
        claims.showed(&b, "alice", &a, 1000);
        claims.showed(&b, "coord", &me, 1000);
        claims.showed(&b, "alice", &a, 5000); // shown again: it knew since the first time
        let knew = 1000 + KNEW_AFTER;
        let alice = nc(&b_sk, &b, "alice", 2000, 2000, false, false);
        assert_eq!(names.offer(alice.clone(), &me), Offer::Refused);
        assert_eq!(claims.impersonation(&names, &alice, &b, knew - 1), None, "it may have crossed what we showed");
        assert_eq!(claims.impersonation(&names, &alice, &b, knew), Some(a.clone()));
        assert_eq!(claims.impersonation(&names, &alice, &b, knew + 1), None, "once per run");
        let later_state = nc(&b_sk, &b, "alice", 2000, 9000, false, false);
        assert_eq!(claims.impersonation(&names, &later_state, &b, knew + 9000), None, "once per machine and name");
        // One of our own names, created before theirs.
        let coord = nc(&b_sk, &b, "coord", 3000, 3000, false, false);
        assert_eq!(names.offer(coord.clone(), &me), Offer::Refused);
        assert_eq!(claims.impersonation(&names, &coord, &b, knew), Some(me.clone()));
    }

    #[test]
    fn legitimate_conflicts_are_not_reported() {
        let (a_sk, a) = key(1);
        let (b_sk, b) = key(2);
        let (_, n) = key(3);
        let (me_sk, me) = key(9);
        let mut names = Names::default();
        names.offer(nc(&a_sk, &a, "alice", 100, 100, false, false), &me);
        names.offer(nc(&a_sk, &a, "gone", 100, 300, true, false), &me);
        names.offer(nc(&a_sk, &a, "moved", 100, 100, false, false), &me);
        names.offer(nc(&a_sk, &a, "unseen", 100, 100, false, false), &me);
        names.offer(nc(&me_sk, &me, "coord", 200, 200, false, false), &me);
        let mut claims = Claims::default();
        for x in ["alice", "gone", "coord"] {
            claims.showed(&b, x, names.get(x).map(|c| c.k.as_str()).unwrap(), 1000);
        }
        claims.showed(&b, "moved", &n, 1000); // owned by n when we showed it, by a now
        let now = 1000 + KNEW_AFTER;
        let mut check = |c: NameCert, via: &str, why: &str| {
            assert_ne!(names.clone().offer(c.clone(), &me), Offer::Accepted, "{why}");
            assert_eq!(claims.impersonation(&names, &c, via, now), None, "{why}");
        };
        check(nc(&b_sk, &b, "alice", 50, 50, false, false), &b, "created before the owner: it wins once the owner sees it");
        check(nc(&b_sk, &b, "alice", 2000, 2000, false, false), &n, "relayed: another machine has not seen the release yet");
        check(nc(&b_sk, &b, "alice", 2000, 2000, true, false), &b, "a deletion");
        check(nc(&b_sk, &b, "alice", 2000, 2000, false, true), &b, "a release");
        check(nc(&b_sk, &b, "gone", 2000, 2000, false, false), &b, "nobody uses the name any more");
        check(nc(&b_sk, &b, "moved", 2000, 2000, false, false), &b, "we never showed it this owner");
        check(nc(&b_sk, &b, "unseen", 2000, 2000, false, false), &b, "a name we never showed it");
        // Created before ours: we lose it (4.12), nothing to report.
        let earlier = nc(&b_sk, &b, "coord", 150, 150, false, false);
        assert_eq!(claims.impersonation(&names, &earlier, &b, now), None);
        assert_eq!(names.offer(earlier, &me), Offer::Lost);
    }

    #[test]
    fn lateness_adds_both_sides() {
        let hour = LATE_AFTER;
        assert!(!is_late(0, 1000, 1000 + hour - 1));
        assert!(is_late(0, 1000, 1000 + hour));
        assert!(is_late(hour, 1000, 1000), "it waited an hour at the sender");
        assert!(is_late(hour / 2, 0, hour / 2), "half there, half here");
        assert!(!is_late(0, 5000, 1000), "a clock going back counts as no wait");
    }
}
