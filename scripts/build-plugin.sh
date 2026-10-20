#!/bin/sh
# Builds the plugin's binaries and puts them in plugin/skills/im/:
#   denden.exe              Windows x64 (MSVC), next to the sh launcher (a host that starts
#                           ./skills/im/denden without a shell resolves it to denden.exe)
#   bin/linux-x64/denden    Linux x86-64, static (musl), built in WSL (natively on Linux)
# then writes SHA256SUMS and SOURCE. Run from Git Bash on Windows, at the root of the repository.
# The steps can also run apart, on machines that checked out the same commit, as the release
# workflow does (.github/workflows/release.yml):
#   sh scripts/build-plugin.sh windows   denden.exe only, on Windows
#   sh scripts/build-plugin.sh linux     the Linux binary only, on Linux (or in WSL from Windows)
#   sh scripts/build-plugin.sh sums      SOURCE and SHA256SUMS only, once both binaries are in place
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
skill="$root/plugin/skills/im"
cd "$root"
# What goes into the binary (the local page is embedded with include_str!).
SOURCES="crates Cargo.toml Cargo.lock plugin/skills/im/pages"
# Which sources the binaries come from, for scripts/publish-release.sh: a hash of the committed
# source tree, or "uncommitted" when the build starts with changes not committed yet.
if [ -z "$(git status --porcelain -- $SOURCES)" ]; then
  source=$(git ls-tree -r HEAD -- $SOURCES | git hash-object --stdin)
else
  source=uncommitted
fi

for step in ${*:-windows linux sums}; do
  case $step in
  windows)
    echo "== Windows x64"
    case $(uname -s) in MINGW*|MSYS*|CYGWIN*) ;; *) echo "denden.exe is built on Windows" >&2; exit 1 ;; esac
    cargo build --release
    # A running denden.exe (a listener, Codex's MCP server) is locked by Windows: it cannot be
    # overwritten, but it can be renamed. The renamed copies go once nothing runs them any more.
    for old in "$skill"/denden.exe.old-*; do [ -e "$old" ] && rm -f "$old" 2>/dev/null; done
    if [ -e "$skill/denden.exe" ] && ! rm -f "$skill/denden.exe" 2>/dev/null; then
      mv "$skill/denden.exe" "$skill/denden.exe.old-$(date +%s)"
    fi
    cp target/release/denden.exe "$skill/denden.exe"
    ;;
  linux)
    if [ "$(uname -s)" = Linux ]; then
      echo "== Linux x64 (musl)"
      CARGO_TARGET_DIR=target-linux cargo build --release --target x86_64-unknown-linux-musl
    else
      echo "== Linux x64 (musl, in WSL)"
      wsl_root=$(wsl -e wslpath -a "$(cygpath -w "$root")" | tr -d '\r')
      wsl -e bash -lc "cd '$wsl_root' && CARGO_TARGET_DIR=target-linux cargo build --release --target x86_64-unknown-linux-musl"
    fi
    mkdir -p "$skill/bin/linux-x64"
    cp target-linux/x86_64-unknown-linux-musl/release/denden "$skill/bin/linux-x64/denden"
    chmod +x "$skill/bin/linux-x64/denden" "$skill/denden"
    ;;
  sums)
    for f in "$skill/denden.exe" "$skill/bin/linux-x64/denden"; do
      [ -f "$f" ] || { echo "missing $f: build it first" >&2; exit 1; }
    done
    echo "$source" > "$skill/SOURCE"
    cd "$skill"
    sha256sum denden.exe bin/linux-x64/denden > SHA256SUMS
    cat SHA256SUMS
    ls -l denden.exe bin/linux-x64/denden
    cd "$root"
    ;;
  *)
    echo "unknown step $step: windows, linux or sums" >&2
    exit 2
    ;;
  esac
done
