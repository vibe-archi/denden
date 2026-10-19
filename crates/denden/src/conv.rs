// The conversations of this machine and the denden each one is attached to (conversations.json).
// A conversation is "claude:<session id>" or "codex:<thread id>". A denden is attached to one
// conversation at most; a conversation holds one denden at most.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::core::*;

/// Wake-ups per hour a conversation gets unless its user sets another budget.
pub const DEFAULT_WAKES: u32 = 20;

#[derive(Clone, Debug, PartialEq)]
pub struct Conv {
    pub key: String,
    /// The channel folder and the denden it holds; empty once detached.
    pub cid: String,
    pub denden: String,
    pub folder: String,
    pub cwd: String,
    pub title: String,
    /// When it was attached, and when it was last heard of (a command, a hook, a listener).
    pub at: u64,
    pub seen: u64,
    pub auto: bool,
    /// Wake-up budget per hour; 0 means no limit.
    pub wakes: u32,
}

impl Conv {
    pub fn app(&self) -> &str {
        self.key.split(':').next().unwrap_or("")
    }
    pub fn id(&self) -> &str {
        self.key.split_once(':').map(|(_, id)| id).unwrap_or("")
    }
    pub fn attached(&self) -> bool {
        !self.cid.is_empty() && !self.denden.is_empty()
    }
    fn to_json(&self) -> Value {
        json!({
            "cid": self.cid, "denden": self.denden, "folder": self.folder, "cwd": self.cwd, "title": self.title,
            "at": self.at, "seen": self.seen, "auto": self.auto, "wakes": self.wakes,
        })
    }
    fn from_json(key: &str, v: &Value) -> Conv {
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let u = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
        Conv {
            key: key.to_string(),
            cid: s("cid"),
            denden: s("denden"),
            folder: s("folder"),
            cwd: s("cwd"),
            title: s("title"),
            at: u("at"),
            seen: u("seen"),
            auto: truthy(v.get("auto")),
            wakes: v.get("wakes").and_then(Value::as_u64).map(|w| w as u32).unwrap_or(DEFAULT_WAKES),
        }
    }
    /// How the user knows which conversation this is: its title, else app, folder and last activity.
    pub fn describe(&self, now: u64) -> String {
        let app = match self.app() {
            "claude" => "Claude",
            "codex" => "Codex",
            other => other,
        };
        let ago = crate::time::ago(now, self.seen.max(self.at));
        if self.title.is_empty() {
            let place = if self.cwd.is_empty() { String::new() } else { format!(" in {}", self.cwd) };
            format!("a {app} conversation{place} (active {ago})")
        } else {
            format!("the conversation \"{}\" ({app}, active {ago})", self.title)
        }
    }
}

pub fn valid_key(key: &str) -> bool {
    match key.split_once(':') {
        Some(("claude" | "codex", id)) => !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)),
        _ => false,
    }
}

#[derive(Default, Debug)]
pub struct Convs {
    pub map: BTreeMap<String, Conv>,
}

impl Convs {
    pub fn load(file: &Path) -> Convs {
        let map = read_json(file)
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default()
            .iter()
            .filter(|(k, _)| valid_key(k))
            .map(|(k, v)| (k.clone(), Conv::from_json(k, v)))
            .collect();
        Convs { map }
    }
    pub fn save(&self, file: &Path) {
        let m: Map<String, Value> = self.map.iter().map(|(k, c)| (k.clone(), c.to_json())).collect();
        let _ = write_json(file, &Value::Object(m));
    }
    /// The conversation `denden` of channel `cid` is attached to.
    pub fn holder(&self, cid: &str, denden: &str) -> Option<&Conv> {
        self.map.values().find(|c| c.cid == cid && c.denden == denden)
    }
    pub fn get(&self, key: &str) -> Option<&Conv> {
        self.map.get(key)
    }
    /// The record of `key`, made on first use.
    pub fn entry(&mut self, key: &str, now: u64) -> &mut Conv {
        self.map.entry(key.to_string()).or_insert_with(|| Conv {
            key: key.to_string(),
            cid: String::new(),
            denden: String::new(),
            folder: String::new(),
            cwd: String::new(),
            title: String::new(),
            at: now,
            seen: now,
            auto: false,
            wakes: DEFAULT_WAKES,
        })
    }
    /// Attaches `denden` of `cid` to `key`, taking it from whichever conversation held it. Returns
    /// the conversation it was taken from, if any.
    pub fn attach(&mut self, key: &str, cid: &str, denden: &str, now: u64) -> Option<Conv> {
        let previous = self.holder(cid, denden).filter(|c| c.key != key).cloned();
        if let Some(p) = &previous {
            if let Some(c) = self.map.get_mut(&p.key) {
                c.cid.clear();
                c.denden.clear();
            }
        }
        let c = self.entry(key, now);
        c.cid = cid.to_string();
        c.denden = denden.to_string();
        c.at = now;
        c.seen = now;
        previous
    }
    pub fn detach(&mut self, key: &str) -> Option<(String, String)> {
        let c = self.map.get_mut(key).filter(|c| c.attached())?;
        let was = (std::mem::take(&mut c.cid), std::mem::take(&mut c.denden));
        Some(was)
    }
    /// Detaches whichever conversation holds `denden` of `cid`.
    pub fn release(&mut self, cid: &str, denden: &str) -> Option<String> {
        let key = self.holder(cid, denden)?.key.clone();
        self.detach(&key);
        Some(key)
    }
    pub fn attached(&self) -> impl Iterator<Item = &Conv> {
        self.map.values().filter(|c| c.attached())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_holder_per_denden() {
        let mut convs = Convs::default();
        assert_eq!(convs.attach("claude:a", "c1", "alice", 1), None);
        let prev = convs.attach("claude:b", "c1", "alice", 2).unwrap();
        assert_eq!(prev.key, "claude:a");
        assert_eq!(convs.holder("c1", "alice").unwrap().key, "claude:b");
        assert!(!convs.get("claude:a").unwrap().attached());
        // Taking another denden moves the conversation off its previous one.
        convs.attach("claude:b", "c1", "bob", 3);
        assert!(convs.holder("c1", "alice").is_none());
        assert_eq!(convs.release("c1", "bob").as_deref(), Some("claude:b"));
        assert_eq!(convs.attached().count(), 0);
    }

    #[test]
    fn keys() {
        assert!(valid_key("claude:5d2e9648-b196-4765-bf7d-d4651a5207c2"));
        assert!(valid_key("codex:019a2b3c-4d5e-7f80-9a1b-2c3d4e5f6a7b"));
        assert!(!valid_key("claude:"));
        assert!(!valid_key("other:abc"));
        assert!(!valid_key("claude:a b"));
    }
}
