// How denden speaks: every line it writes for a conversation has the same look, whatever the AI.
//   > 🐌» (( information ))        a one-line quote, readable even where Markdown is not rendered
//   > 🐌» (( ⚠ problem ))          an alert
//   🐌»  **question?**             a decision for the user (bold, not quoted)

use serde_json::Value;

pub fn info(text: &str) -> String {
    format!("> 🐌» (( {} ))", quote_lines(text))
}

pub fn alert(text: &str) -> String {
    info(&format!("⚠ {text}"))
}

pub fn question(text: &str) -> String {
    format!("🐌»  **{text}**")
}

/// Multi-line text stays inside one quote.
fn quote_lines(text: &str) -> String {
    text.trim_end().replace("\r\n", "\n").replace('\n', "\n> ")
}

/// Lines printed together come under one snail: one line stays `> 🐌» (( … ))`, several become
///   > 🐌» ((
///   > line 1
///   > line 2
///   > ))
/// Other lines between them (a table row) go into the block; a question ends it.
pub fn group(lines: &[String]) -> String {
    const HEAD: &str = "> 🐌» (( ";
    const TAIL: &str = " ))";
    fn close(open: &mut Vec<String>, out: &mut Vec<String>) {
        match open.len() {
            0 => {}
            1 if !open[0].contains('\n') => out.push(format!("{HEAD}{}{TAIL}", open[0])),
            _ => {
                let body: Vec<String> = open.iter().flat_map(|t| t.lines().map(|l| format!("> {l}")).collect::<Vec<_>>()).collect();
                out.push(format!("> 🐌» ((\n{}\n> ))", body.join("\n")));
            }
        }
        open.clear();
    }
    let (mut out, mut open) = (Vec::new(), Vec::new());
    for line in lines {
        if line.starts_with(HEAD) && line.ends_with(TAIL) && line.len() >= HEAD.len() + TAIL.len() {
            open.push(line[HEAD.len()..line.len() - TAIL.len()].replace("\n> ", "\n"));
        } else if line.starts_with("🐌»  **") {
            close(&mut open, &mut out);
            out.push(line.clone());
        } else if open.is_empty() {
            out.push(line.clone());
        } else {
            open.push(line.clone());
        }
    }
    close(&mut open, &mut out);
    out.join("\n")
}

/// What a command prints for the conversation waits here, and comes out grouped (`group`) when the
/// command ends, fails or asks a question.
static PENDING: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub fn say(line: impl Into<String>) {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).push(line.into());
}

pub fn flush() {
    let lines = std::mem::take(&mut *PENDING.lock().unwrap_or_else(|e| e.into_inner()));
    if !lines.is_empty() {
        println!("{}", group(&lines));
    }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}
fn b(v: &Value, k: &str) -> bool {
    v.get(k).and_then(Value::as_bool).unwrap_or(false)
}

/// Who a message went to, as the receiving side shows it: nothing for itself, "to all" for the
/// whole channel, the list when others got it too.
fn audience(item: &Value, me: &str) -> Option<String> {
    match item.get("to") {
        Some(Value::String(t)) if t == "all" => Some("to all".into()),
        Some(Value::Array(a)) if a.len() > 1 => {
            let others: Vec<&str> = a.iter().filter_map(Value::as_str).filter(|n| *n != me).collect();
            Some(format!("also to {}", others.join(", ")))
        }
        _ => None,
    }
}

/// One record handed to a conversation (a message or a notice from denden).
pub fn item(it: &Value, me: &str) -> String {
    if s(it, "kind") == "note" {
        let text = s(it, "text");
        return if b(it, "alert") && !text.starts_with('⚠') { alert(text) } else { info(text) };
    }
    let mut head = vec![s(it, "from").to_string()];
    if let Some(a) = audience(it, me) {
        head.push(a);
    }
    if !s(it, "re").is_empty() {
        head.push("reply".into());
    } else if b(it, "ask") {
        head.push(format!("reply expected #{}", s(it, "id")));
    }
    if b(it, "late") {
        head.push(format!("late, sent {}", s(it, "ago")));
    }
    info(&format!("{} : {}", head.join(" · "), s(it, "text")))
}

/// A message this conversation sent, as `history` shows it.
pub fn sent(it: &Value) -> String {
    let to = match it.get("to") {
        Some(Value::String(t)) if t == "all" => "all".to_string(),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "),
        _ => String::new(),
    };
    let mut head = vec![format!("→ {to}")];
    if !s(it, "re").is_empty() {
        head.push("reply".into());
    } else if b(it, "ask") {
        head.push("reply expected".into());
    }
    if b(it, "queued") {
        head.push("not delivered yet".into());
    }
    info(&format!("{} : {}", head.join(" · "), s(it, "text")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shapes() {
        assert_eq!(info("Channel vibedoc-chan created on laptop-ak"), "> 🐌» (( Channel vibedoc-chan created on laptop-ak ))");
        assert_eq!(alert("bad key"), "> 🐌» (( ⚠ bad key ))");
        assert_eq!(question("Which name?"), "🐌»  **Which name?**");
        assert_eq!(info("a\nb"), "> 🐌» (( a\n> b ))");
    }

    #[test]
    fn grouped_under_one_snail() {
        let one = vec![info("Channel x created")];
        assert_eq!(group(&one), "> 🐌» (( Channel x created ))");
        let three = vec![info("This conversation is bob in x"), info("Role of bob: tests"), alert("bad key")];
        assert_eq!(group(&three), "> 🐌» ((\n> This conversation is bob in x\n> Role of bob: tests\n> ⚠ bad key\n> ))");
        let table = vec![info("x · 2 denden"), "| denden | state |".to_string(), "| bob | online |".to_string()];
        assert_eq!(group(&table), "> 🐌» ((\n> x · 2 denden\n> | denden | state |\n> | bob | online |\n> ))");
        let ask = vec![info("3 messages arrived while you were away"), info("a : b"), question("Which ones?")];
        assert_eq!(group(&ask), "> 🐌» ((\n> 3 messages arrived while you were away\n> a : b\n> ))\n🐌»  **Which ones?**");
        assert_eq!(group(&[info("a\nb")]), "> 🐌» ((\n> a\n> b\n> ))");
    }

    #[test]
    fn items() {
        let m = json!({ "kind": "msg", "id": "a1b2c3", "from": "coord", "to": ["back"], "text": "Is /users ready?", "ask": true, "late": false });
        assert_eq!(item(&m, "back"), "> 🐌» (( coord · reply expected #a1b2c3 : Is /users ready? ))");
        let r = json!({ "kind": "msg", "id": "x", "from": "back", "to": ["coord"], "text": "Yes", "re": "a1b2c3" });
        assert_eq!(item(&r, "coord"), "> 🐌» (( back · reply : Yes ))");
        let all = json!({ "kind": "msg", "id": "y", "from": "coord", "to": "all", "text": "Freeze at 18:00", "late": true, "ago": "2 h ago" });
        assert_eq!(item(&all, "db"), "> 🐌» (( coord · to all · late, sent 2 h ago : Freeze at 18:00 ))");
        let group = json!({ "kind": "msg", "id": "z", "from": "coord", "to": ["api", "stress", "db"], "text": "Restart" });
        assert_eq!(item(&group, "db"), "> 🐌» (( coord · also to api, stress : Restart ))");
        let note = json!({ "kind": "note", "text": "⚠ alice already existed", "alert": true });
        assert_eq!(item(&note, "alice"), "> 🐌» (( ⚠ alice already existed ))");
    }
}
