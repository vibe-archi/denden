// Where daemons run from, and which build they are.
//
// A daemon runs for days, and on Windows a running executable cannot be deleted or replaced. So the
// daemon never runs from the file a host installed (the plugin's im.exe): it runs from a copy under
// DENDEN_HOME/bin named after the version and the content hash (dev builds all say 0.2.0), written
// atomically and checked against that hash before every start. Daemons record their build in
// daemon.json; a command from a newer build restarts a daemon from an older one, never the reverse,
// so two installed builds (Claude Code runs the plugin from the repo, Codex from its own cache) never
// keep restarting each other.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::Value;

use crate::core::{ensure_dir, home, is_alive, random_hex, read_json, sha256};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Milliseconds since the Unix epoch, set by build.rs.
pub const BUILD_TIME: u64 = parse_u64(env!("DENDEN_BUILD_TIME"));

const fn parse_u64(s: &str) -> u64 {
    let b = s.as_bytes();
    let mut n = 0u64;
    let mut i = 0;
    while i < b.len() {
        n = n * 10 + (b[i] - b'0') as u64;
        i += 1;
    }
    n
}

pub fn bin_dir() -> PathBuf {
    home().join("bin")
}

pub fn file_sha256(p: &Path) -> Option<String> {
    fs::read(p).ok().map(|b| hex::encode(sha256(&b)))
}

/// The bytes of the running executable. On Linux through /proc/self/exe, which still reads the file
/// we were started from after it has been replaced or deleted.
fn own_bytes() -> std::io::Result<Vec<u8>> {
    #[cfg(target_os = "linux")]
    if let Ok(b) = fs::read("/proc/self/exe") {
        return Ok(b);
    }
    fs::read(std::env::current_exe()?)
}

/// SHA-256 (hex) of the running executable, computed once.
pub fn own_sha256() -> Option<&'static str> {
    static SHA: OnceLock<Option<String>> = OnceLock::new();
    SHA.get_or_init(|| own_bytes().ok().map(|b| hex::encode(sha256(&b)))).as_deref()
}

fn copy_name(sha: &str) -> String {
    format!("denden-{VERSION}-{}{}", &sha[..12], std::env::consts::EXE_SUFFIX)
}

#[cfg(unix)]
fn set_private_exec(p: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(0o700))
}
#[cfg(not(unix))]
fn set_private_exec(_: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Makes sure DENDEN_HOME/bin holds an exact copy of this executable and returns its path. An existing
/// copy is used only if its content still matches its hash; otherwise (missing, truncated, tampered
/// with) a fresh one is written next to it and renamed over it once checked.
pub fn ensure_copy() -> Result<PathBuf, String> {
    let bytes = own_bytes().map_err(|e| format!("cannot read this executable: {e}"))?;
    let sha = hex::encode(sha256(&bytes));
    let dir = bin_dir();
    ensure_dir(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let target = dir.join(copy_name(&sha));
    if file_sha256(&target).as_deref() == Some(sha.as_str()) {
        let _ = set_private_exec(&target);
        return Ok(target);
    }
    let tmp = dir.join(format!(".{}.{}-{}.tmp", copy_name(&sha), std::process::id(), random_hex(4)));
    let written = (|| -> Result<(), String> {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o700);
        }
        let mut f = opts.open(&tmp).map_err(|e| e.to_string())?;
        f.write_all(&bytes).and_then(|_| f.sync_all()).map_err(|e| e.to_string())?;
        drop(f);
        if file_sha256(&tmp).as_deref() != Some(sha.as_str()) {
            return Err("the copy does not match after writing it".into());
        }
        set_private_exec(&tmp).map_err(|e| e.to_string())?;
        // Replaces a bad copy too, unless something runs from it (Windows keeps it locked then).
        fs::rename(&tmp, &target).map_err(|e| format!("cannot put it in place: {e}"))
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(format!("cannot write {}: {e}", target.display()));
    }
    Ok(target)
}

/// Removes the copies in DENDEN_HOME/bin that no running daemon uses, except `current`, and temporary
/// files left behind. Best effort: a file still in use is locked on Windows and simply skipped.
pub fn clean_up(current: &Path) {
    let Ok(entries) = fs::read_dir(bin_dir()) else { return };
    let mut keep: HashSet<OsString> = HashSet::new();
    keep.extend(current.file_name().map(OsString::from));
    if let Some(rec) = read_json(&home().join("daemon.json")) {
        let alive = rec.get("pid").and_then(Value::as_u64).is_some_and(|p| is_alive(p as u32));
        if let (true, Some(exe)) = (alive, rec.get("exe").and_then(Value::as_str)) {
            keep.extend(Path::new(exe).file_name().map(OsString::from));
        }
    }
    for e in entries.flatten() {
        let name = e.file_name();
        let s = name.to_string_lossy();
        let stale_tmp = s.starts_with(".denden-")
            && s.ends_with(".tmp")
            && e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > Duration::from_secs(600));
        if keep.contains(&name) || !(s.starts_with("denden-") || stale_tmp) {
            continue;
        }
        let _ = fs::remove_file(e.path());
    }
}

fn parse_version(v: &str) -> [u64; 3] {
    let core = v.split(['-', '+']).next().unwrap_or("");
    let mut out = [0u64; 3];
    for (slot, part) in out.iter_mut().zip(core.split('.')) {
        *slot = part.trim().parse().unwrap_or(0);
    }
    out
}

/// Whether the daemon described by `rec` (its daemon.json) is from an older build than this one:
/// a lower version, or the same version built earlier. A daemon without build info is never older.
pub fn is_older(rec: &Value) -> bool {
    is_older_than(rec, VERSION, BUILD_TIME)
}

fn is_older_than(rec: &Value, version: &str, build_time: u64) -> bool {
    if rec.get("engine").and_then(Value::as_str) != Some("rust") {
        return false;
    }
    let (Some(v), Some(t)) = (rec.get("version").and_then(Value::as_str), rec.get("buildTime").and_then(Value::as_u64)) else {
        return false;
    };
    (parse_version(v), t) < (parse_version(version), build_time)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn older_builds() {
        let rec = |v: &str, t: u64| json!({ "engine": "rust", "version": v, "buildTime": t });
        assert!(is_older_than(&rec("0.2.0", 100), "0.2.0", 200));
        assert!(!is_older_than(&rec("0.2.0", 200), "0.2.0", 200), "same build: no restart");
        assert!(!is_older_than(&rec("0.2.0", 300), "0.2.0", 200), "never restart a newer one");
        assert!(is_older_than(&rec("0.1.9", 900), "0.2.0", 200), "the version comes first");
        assert!(!is_older_than(&rec("0.10.0", 1), "0.9.0", 200), "numeric, not textual");
        assert!(is_older_than(&rec("0.2.0-dev", 100), "0.2.0", 200));
        assert!(!is_older_than(&json!({ "pid": 1, "port": 2, "token": "t" }), "9.9.9", 200), "Node daemon: left alone");
        assert!(!is_older_than(&json!({ "engine": "rust" }), "9.9.9", 200), "no build info: left alone");
        assert!(!is_older_than(&json!({ "engine": "node", "version": "0.1.0", "buildTime": 1 }), "9.9.9", 200));
    }
}
