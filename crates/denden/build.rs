// Embeds the build time (milliseconds since the Unix epoch) as DENDEN_BUILD_TIME. Daemons record it, and
// a CLI restarts a daemon from an older build of the same version (dev builds share a version).
// Setting DENDEN_BUILD_TIME when building pins it (tests make an "older" and a "newer" build that way).

use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let pinned = std::env::var("DENDEN_BUILD_TIME").ok().filter(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()));
    let t = pinned.unwrap_or_else(|| SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0).to_string());
    println!("cargo:rustc-env=DENDEN_BUILD_TIME={t}");
    println!("cargo:rerun-if-env-changed=DENDEN_BUILD_TIME");
    // Any rerun-if line replaces cargo's default (rerun when any package file changes): list them.
    for p in ["src", "build.rs", "Cargo.toml", "Cargo.lock", "../../plugin/skills/im/pages/ui.html"] {
        println!("cargo:rerun-if-changed={p}");
    }
}
