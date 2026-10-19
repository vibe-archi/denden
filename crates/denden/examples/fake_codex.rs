// A stand-in for `codex` in tests (DENDEN_CODEX_BIN): appends its argv as one JSON line to the file
// named by FAKE_CODEX_LOG and exits 0, or 1 while "<FAKE_CODEX_LOG>.fail" exists. That file may
// say which thread fails ("thread:<id>" on its first line) and the error to print (the rest; by
// default "Error: app-server unavailable"). `queue --help` prints a usage line, like the real one.
//
//     cargo build --release --example fake_codex

use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("queue") && args.iter().any(|a| a == "--help") {
        println!("Queue a message for an existing session\n\nUsage: codex queue [OPTIONS] --thread <THREAD> --message <TEXT>");
        return;
    }
    let log = std::env::var("FAKE_CODEX_LOG").ok().filter(|l| !l.is_empty());
    let thread = args.iter().position(|a| a == "--thread").and_then(|i| args.get(i + 1)).cloned().unwrap_or_default();
    let fail_spec = log.as_ref().and_then(|l| std::fs::read_to_string(format!("{l}.fail")).ok());
    let (failing, error) = match &fail_spec {
        None => (false, String::new()),
        Some(spec) => {
            let mut lines = spec.lines();
            let first = lines.next().unwrap_or("");
            match first.strip_prefix("thread:") {
                Some(only) => (only.trim() == thread, lines.collect::<Vec<_>>().join("\n")),
                None => (true, spec.trim().to_string()),
            }
        }
    };
    let error = if error.trim().is_empty() { "Error: app-server unavailable".to_string() } else { error };
    if let Some(l) = &log {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let line = serde_json::json!({ "t": t, "argv": args, "failed": failing });
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(l) {
            let _ = writeln!(f, "{line}");
        }
    }
    if failing {
        eprintln!("{error}");
        std::process::exit(1);
    }
    println!("Queued message for thread {thread}");
}
