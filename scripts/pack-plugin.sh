#!/bin/sh
# Packs what a machine needs to install denden by hand: the two marketplaces, the plugin with
# its binaries, and the install guide, in a folder named denden-package:
#   dist/denden-<version>-<commit>.tar.gz   Linux (executable bits kept: built in WSL)
#   dist/denden-<version>-<commit>.zip      Windows
# Run from Git Bash on Windows after scripts/build-plugin.sh.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
skill=plugin/skills/im
for f in "$skill/denden.exe" "$skill/bin/linux-x64/denden" "$skill/SHA256SUMS"; do
  [ -f "$f" ] || { echo "missing $f: run sh scripts/build-plugin.sh first" >&2; exit 1; }
done
(cd "$skill" && sha256sum -c --quiet SHA256SUMS) || { echo "the binaries do not match SHA256SUMS: rebuild" >&2; exit 1; }
version=$(sed -n 's/.*"version": *"\([^"]*\)".*/\1/p' plugin/.claude-plugin/plugin.json | head -1)
name="denden-$version-$(git rev-parse --short HEAD)"
[ -z "$(git status --porcelain -- crates plugin Cargo.toml Cargo.lock)" ] || echo "note: uncommitted changes in the sources; the name says $name"

stage=$(mktemp -d)
mkdir -p "$stage/denden-package"
cp -r .claude-plugin .agents plugin "$stage/denden-package/"
rm -f "$stage"/denden-package/plugin/skills/im/denden.exe.old-* "$stage"/denden-package/plugin/skills/im/SOURCE
cp docs/INSTALL.md "$stage/denden-package/INSTALL.md"
mkdir -p dist
rm -f "dist/$name.tar.gz" "dist/$name.zip"

# Linux archive, made in WSL so the launcher and the Linux binary keep their executable bit.
wsl_stage=$(wsl -e wslpath -a "$(cygpath -w "$stage")" | tr -d '\r')
wsl_dist=$(wsl -e wslpath -a "$(cygpath -w "$root/dist")" | tr -d '\r')
wsl -e bash -c "set -e; t=\$(mktemp -d); cp -r '$wsl_stage/denden-package' \"\$t/\"; find \"\$t/denden-package\" -type d -exec chmod 755 {} +; find \"\$t/denden-package\" -type f -exec chmod 644 {} +; chmod 755 \"\$t/denden-package/plugin/skills/im/denden\" \"\$t/denden-package/plugin/skills/im/bin/linux-x64/denden\"; tar -czf '$wsl_dist/$name.tar.gz' -C \"\$t\" denden-package; rm -rf \"\$t\""

# Windows archive, with Windows' own tar (bsdtar), which writes zip files.
(cd "$stage" && /c/Windows/System32/tar.exe -a -c -f "$(cygpath -w "$root/dist/$name.zip")" denden-package)

rm -rf "$stage"
cd dist
sha256sum "$name.tar.gz" "$name.zip" > "$name.sha256"
ls -l "$name.tar.gz" "$name.zip"
cat "$name.sha256"
