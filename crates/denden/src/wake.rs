// When to wake a conversation for the messages of its denden: the same rules for Claude (the
// listener prints them, which wakes the agent) and Codex (a notice queued with `codex queue`).
//
// - Messages arriving together are announced together (DEBOUNCE).
// - A conversation in the middle of a turn is not woken: Codex's Stop hook hands the messages over
//   at the end of the turn; Claude's listener can print at any time.
// - A budget of wake-ups per hour (the conversation's own, 20 by default). Once it is spent, the
//   messages wait for the user's next prompt (the prompt hook hands them over), and the
//   conversation is told once, with how to raise the budget.
// - Codex only: one notice at a time, until it is read or expires; queue failures are retried a
//   few times before the conversation stops being woken.

use serde_json::{Value, json};

/// A Codex turn counts as finished after this long without a Stop hook (an interrupted turn sends none).
pub const BUSY_TIMEOUT: u64 = 15 * 60 * 1000;
/// A queued notice blocks further ones until its messages are read, a hook comes, or this passes.
pub const PENDING_TTL: u64 = 10 * 60 * 1000;
/// Minimum gap between two Codex wake-ups (one notice per turn is plenty).
pub const MIN_GAP: u64 = 30 * 1000;
pub const DEBOUNCE: u64 = 2 * 1000;
/// Consecutive failed `codex queue` runs before the conversation is no longer woken.
pub const MAX_FAILURES: u32 = 3;
const HOUR: u64 = 3600 * 1000;

/// One unread record, as far as waking is concerned.
pub struct Unread {
    pub seq: u64,
    /// When it arrived here.
    pub at: u64,
    pub from: String,
}

#[derive(Debug, PartialEq)]
pub enum Decision {
    Nothing,
    /// Due, but the budget is spent; `first`: the first time since the last wake-up, when the
    /// conversation is to be told.
    Spent { first: bool },
    Wake { n: usize, senders: Vec<String>, upto: u64 },
}

#[derive(Default, Debug, Clone)]
pub struct WakeState {
    /// Wake-ups in the last hour.
    pub wakes: Vec<u64>,
    /// The newest seq already announced or handed over: only newer ones wake.
    pub notified: u64,
    /// Codex: a turn is running since then.
    pub busy_since: Option<u64>,
    /// Codex: a notice was queued then and is not read yet.
    pub pending: Option<u64>,
    pub failures: u32,
    pub in_flight: bool,
    /// Codex: how the hook's environment runs codex, and its home.
    pub codex: Option<Vec<String>>,
    pub codex_home: Option<String>,
    pub last_error: Option<String>,
    spent_noted: bool,
}

impl WakeState {
    pub fn busy(&self, now: u64) -> bool {
        self.busy_since.is_some_and(|b| now.saturating_sub(b) < BUSY_TIMEOUT)
    }
    fn prune(&mut self, now: u64) {
        self.wakes.retain(|w| now.saturating_sub(*w) < HOUR);
    }
    /// Within the budget (`per_hour`, 0 = unlimited), and `gap` since the last wake-up.
    pub fn allowed(&mut self, now: u64, per_hour: u32, gap: u64) -> bool {
        self.prune(now);
        let under = per_hour == 0 || self.wakes.len() < per_hour as usize;
        under && self.wakes.last().is_none_or(|l| now.saturating_sub(*l) >= gap)
    }
    pub fn record(&mut self, now: u64) {
        self.wakes.push(now);
        self.spent_noted = false;
    }

    /// Whether to wake now for `unread`. `codex`: the Codex rules (busy turns, one pending notice,
    /// the minimum gap); otherwise Claude's listener.
    pub fn decide(&mut self, now: u64, unread: &[Unread], per_hour: u32, codex: bool) -> Decision {
        if unread.is_empty() {
            self.pending = None;
            return Decision::Nothing;
        }
        if codex {
            match self.pending {
                Some(at) if now.saturating_sub(at) < PENDING_TTL => return Decision::Nothing,
                _ => self.pending = None,
            }
        }
        let fresh: Vec<&Unread> = unread.iter().filter(|u| u.seq > self.notified).collect();
        if fresh.is_empty() || self.in_flight || (codex && self.failures >= MAX_FAILURES) {
            return Decision::Nothing;
        }
        if now < fresh.iter().map(|u| u.at).min().unwrap_or(0) + DEBOUNCE {
            return Decision::Nothing;
        }
        if codex && self.busy(now) {
            return Decision::Nothing; // its Stop hook will hand them over
        }
        if !self.allowed(now, per_hour, if codex { MIN_GAP } else { 0 }) {
            if self.wakes.len() < per_hour as usize || per_hour == 0 {
                return Decision::Nothing; // only the gap: soon
            }
            let first = !self.spent_noted;
            self.spent_noted = true;
            return Decision::Spent { first };
        }
        let mut senders: Vec<String> = Vec::new();
        for u in unread {
            if !senders.contains(&u.from) {
                senders.push(u.from.clone());
            }
        }
        let upto = unread.iter().map(|u| u.seq).max().unwrap_or(self.notified);
        self.record(now);
        if codex {
            self.pending = Some(now);
            self.in_flight = true;
        }
        Decision::Wake { n: unread.len(), senders, upto }
    }

    /// Announced or handed over up to `upto`: those no longer wake.
    pub fn handed(&mut self, upto: u64) {
        self.notified = self.notified.max(upto);
        self.pending = None;
    }

    /// A Codex queue command finished. On failure the messages are announced again later.
    pub fn queued(&mut self, result: &Result<(), String>, prev_notified: u64) {
        self.in_flight = false;
        match result {
            Ok(()) => {
                self.failures = 0;
                self.last_error = None;
            }
            Err(e) => {
                self.failures += 1;
                self.last_error = Some(e.clone());
                self.pending = None;
                self.notified = prev_notified;
                self.wakes.pop();
            }
        }
    }

    /// A Codex hook from this conversation: a prompt starts a turn, a Stop ends it.
    pub fn hook(&mut self, prompt: bool, now: u64) {
        self.busy_since = prompt.then_some(now);
        self.pending = None;
        self.failures = 0;
    }

    pub fn to_json(&self) -> Value {
        json!({
            "wakes": self.wakes, "notified": self.notified, "busySince": self.busy_since,
            "failures": self.failures, "codex": self.codex, "codexHome": self.codex_home, "lastError": self.last_error,
        })
    }
    pub fn from_json(v: &Value) -> WakeState {
        WakeState {
            wakes: v.get("wakes").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default(),
            notified: v.get("notified").and_then(Value::as_u64).unwrap_or(0),
            busy_since: v.get("busySince").and_then(Value::as_u64),
            failures: v.get("failures").and_then(Value::as_u64).unwrap_or(0) as u32,
            codex: v.get("codex").and_then(crate::codex::argv_of),
            codex_home: v.get("codexHome").and_then(Value::as_str).map(str::to_string),
            last_error: v.get("lastError").and_then(Value::as_str).map(str::to_string),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unread(list: &[(u64, u64)]) -> Vec<Unread> {
        list.iter().map(|(seq, at)| Unread { seq: *seq, at: *at, from: "coord".into() }).collect()
    }

    #[test]
    fn debounce_then_one_wake() {
        let mut w = WakeState::default();
        let u = unread(&[(1, 1000)]);
        assert_eq!(w.decide(1500, &u, 20, false), Decision::Nothing, "debounce");
        assert!(matches!(w.decide(3100, &u, 20, false), Decision::Wake { n: 1, upto: 1, .. }));
        w.handed(1);
        assert_eq!(w.decide(4000, &u, 20, false), Decision::Nothing, "already announced");
    }

    #[test]
    fn budget_spent_is_told_once() {
        let mut w = WakeState::default();
        let mut now = 10_000;
        for seq in 1..=3 {
            let u = unread(&[(seq, now)]);
            assert!(matches!(w.decide(now + DEBOUNCE, &u, 3, false), Decision::Wake { .. }));
            w.handed(seq);
            now += 60_000;
        }
        let u = unread(&[(4, now)]);
        assert_eq!(w.decide(now + DEBOUNCE, &u, 3, false), Decision::Spent { first: true });
        assert_eq!(w.decide(now + DEBOUNCE + 1, &u, 3, false), Decision::Spent { first: false });
        // An hour later the budget is back.
        assert!(matches!(w.decide(now + HOUR, &u, 3, false), Decision::Wake { .. }));
        // No limit at all.
        let mut free = WakeState::default();
        for seq in 1..50 {
            let u = unread(&[(seq, 0)]);
            assert!(matches!(free.decide(DEBOUNCE + seq, &u, 0, false), Decision::Wake { .. }));
            free.handed(seq);
        }
    }

    #[test]
    fn codex_waits_for_its_turn_and_its_notice() {
        let mut w = WakeState::default();
        let u = unread(&[(1, 0)]);
        w.hook(true, 0);
        assert_eq!(w.decide(DEBOUNCE, &u, 20, true), Decision::Nothing, "a turn is running");
        w.hook(false, 1000);
        assert!(matches!(w.decide(DEBOUNCE + 1000, &u, 20, true), Decision::Wake { .. }));
        assert_eq!(w.decide(DEBOUNCE + 2000, &unread(&[(1, 0), (2, 0)]), 20, true), Decision::Nothing, "in flight");
        w.queued(&Ok(()), 0);
        assert_eq!(w.decide(DEBOUNCE + 3000, &unread(&[(1, 0), (2, 0)]), 20, true), Decision::Nothing, "its notice is pending");
        // The notice was not taken up: after it expires, the newer message wakes again (gap passed).
        let later = DEBOUNCE + 1000 + PENDING_TTL + 1;
        assert!(matches!(w.decide(later, &unread(&[(1, 0), (2, 0)]), 20, true), Decision::Wake { upto: 2, .. }));
    }

    #[test]
    fn codex_failures_retry_then_stop() {
        let mut w = WakeState::default();
        let u = unread(&[(1, 0)]);
        let mut now = DEBOUNCE;
        for _ in 0..MAX_FAILURES {
            assert!(matches!(w.decide(now, &u, 20, true), Decision::Wake { .. }));
            w.queued(&Err("exit 1".into()), 0);
            now += MIN_GAP;
        }
        assert_eq!(w.decide(now, &u, 20, true), Decision::Nothing, "given up after repeated failures");
        w.hook(true, now);
        w.hook(false, now + 1);
        assert!(matches!(w.decide(now + MIN_GAP, &u, 20, true), Decision::Wake { .. }), "a hook shows it is back");
    }
}
