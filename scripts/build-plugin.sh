#!/bin/sh
# Builds the plugin's binaries and puts them in plugin/skills/im/:
#   denden.exe              Windows x64 (MSVC), next to the sh launcher (a host that starts
#                           ./skills/im/denden without a shell resolves it to denden.exe)
#   bin/linux-x64/denden    Linux x86-64, static (musl), built in WSL
# then writes SHA256SUMS. Run from Git Bash on Windows, at the root of the repository.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
skill="$root/plugin/skills/im"
cd "$root"

echo "== Windows x64"
cargo build --release
# A running denden.exe (a listener, Codex's MCP server) is locked by Windows: it cannot be
# overwritten, but it can be renamed. The renamed copies go once nothing runs them any more.
for old in "$skill"/denden.exe.old-*; do [ -e "$old" ] && rm -f "$old" 2>/dev/null; done
if [ -e "$skill/denden.exe" ] && ! rm -f "$skill/denden.exe" 2>/dev/null; then
  mv "$skill/denden.exe" "$skill/denden.exe.old-$(date +%s)"
fi
cp target/release/denden.exe "$skill/denden.exe"

echo "== Linux x64 (musl, in WSL)"
wsl_root=$(wsl -e wslpath -a "$(cygpath -w "$root")" | tr -d '\r')
wsl -e bash -lc "cd '$wsl_root' && CARGO_TARGET_DIR=target-linux cargo build --release --target x86_64-unknown-linux-musl"
mkdir -p "$skill/bin/linux-x64"
cp target-linux/x86_64-unknown-linux-musl/release/denden "$skill/bin/linux-x64/denden"
chmod +x "$skill/bin/linux-x64/denden" "$skill/denden"

cd "$skill"
sha256sum denden.exe bin/linux-x64/denden > SHA256SUMS
cat SHA256SUMS
ls -l denden.exe bin/linux-x64/denden
