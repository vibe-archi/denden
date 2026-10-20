#!/bin/sh
# Packs what a machine needs to install denden by hand: the two marketplaces, the plugin with
# its binaries, and the install guide, in a folder named denden-package:
#   dist/denden-<version>-<commit>.tar.gz   Linux (executable bits kept: made on a Linux file system)
#   dist/denden-<version>-<commit>.zip      Windows
# Run after scripts/build-plugin.sh, from Git Bash on Windows (the .tar.gz is made in WSL) or on Linux.
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
mkdir -p "$stage/denden-package/$skill"
# The launcher first: once denden.exe is there, Git Bash takes "denden" for it and cannot create it.
cp "$skill/denden" "$stage/denden-package/$skill/"
cp -r .claude-plugin .agents plugin "$stage/denden-package/"
rm -f "$stage"/denden-package/plugin/skills/im/denden.exe.old-* "$stage"/denden-package/plugin/skills/im/SOURCE
cp docs/INSTALL.md "$stage/denden-package/INSTALL.md"
mkdir -p dist
rm -f "dist/$name.tar.gz" "dist/$name.zip"

# Linux archive, made on a Linux file system (WSL's, from Windows) so the launcher and the Linux
# binary keep their executable bit. $1: the staging folder, $2: the archive, as Linux paths.
tar_linux='set -e; t=$(mktemp -d); cp -r "$1/denden-package" "$t/"; find "$t/denden-package" -type d -exec chmod 755 {} +; find "$t/denden-package" -type f -exec chmod 644 {} +; chmod 755 "$t/denden-package/plugin/skills/im/denden" "$t/denden-package/plugin/skills/im/bin/linux-x64/denden"; tar -czf "$2" -C "$t" denden-package; rm -rf "$t"'
if [ "$(uname -s)" = Linux ]; then
  sh -c "$tar_linux" sh "$stage" "$root/dist/$name.tar.gz"
  (cd "$stage" && zip -qr "$root/dist/$name.zip" denden-package)
else
  wsl_stage=$(wsl -e wslpath -a "$(cygpath -w "$stage")" | tr -d '\r')
  wsl_dist=$(wsl -e wslpath -a "$(cygpath -w "$root/dist")" | tr -d '\r')
  # The paths are WSL's: Git Bash must not turn them into Windows paths.
  MSYS_NO_PATHCONV=1 wsl -e bash -c "$tar_linux" bash "$wsl_stage" "$wsl_dist/$name.tar.gz"
  # Windows archive, with Windows' own tar (bsdtar), which writes zip files.
  (cd "$stage" && /c/Windows/System32/tar.exe -a -c -f "$(cygpath -w "$root/dist/$name.zip")" denden-package)
fi

rm -rf "$stage"
cd dist
sha256sum "$name.tar.gz" "$name.zip" > "$name.sha256"
ls -l "$name.tar.gz" "$name.zip"
cat "$name.sha256"
