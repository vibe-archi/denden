#!/bin/sh
# Publishes a version of the plugin as users install it from GitHub: a commit on the local
# `release` branch (and the tag v<version>) with the marketplaces, the plugin and its binaries,
# the docs and the README. main keeps the sources only; the binaries are build outputs there.
#   claude plugin marketplace add vibe-archi/denden@release
#   codex plugin marketplace add vibe-archi/denden@release
# Run from Git Bash after scripts/build-plugin.sh, with the sources committed. It pushes nothing:
#   git push origin main release v<version>
# The version comes from plugin/.claude-plugin/plugin.json: Claude Code hands users a new copy only
# when it changes, so each release needs a new one (in both plugin manifests).
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
skill=plugin/skills/im
SOURCES="crates Cargo.toml Cargo.lock plugin/skills/im/pages"
SHIPPED=".gitattributes .claude-plugin .agents plugin docs README.md"

for f in "$skill/denden.exe" "$skill/bin/linux-x64/denden" "$skill/SHA256SUMS" "$skill/SOURCE"; do
  [ -f "$f" ] || { echo "missing $f: run sh scripts/build-plugin.sh first" >&2; exit 1; }
done
(cd "$skill" && sha256sum -c --quiet SHA256SUMS) || { echo "the binaries do not match SHA256SUMS: rebuild" >&2; exit 1; }
[ -z "$(git status --porcelain -- $SOURCES $SHIPPED)" ] || { echo "commit the sources and docs first: the release is made from HEAD" >&2; exit 1; }
built=$(cat "$skill/SOURCE")
head_src=$(git ls-tree -r HEAD -- $SOURCES | git hash-object --stdin)
[ "$built" = "$head_src" ] || { echo "the binaries were not built from the sources of HEAD: run sh scripts/build-plugin.sh again" >&2; exit 1; }

version_of() { sed -n 's/.*"version": *"\([^"]*\)".*/\1/p' | head -1; }
version=$(version_of < plugin/.claude-plugin/plugin.json)
[ "$version" = "$(version_of < plugin/.codex-plugin/plugin.json)" ] || { echo "the two plugin manifests disagree on the version" >&2; exit 1; }
if git rev-parse -q --verify "refs/tags/v$version" > /dev/null; then
  echo "v$version is already released: raise the version in plugin/.claude-plugin/plugin.json and plugin/.codex-plugin/plugin.json" >&2
  exit 1
fi

# The release tree, built in a separate index: the working tree and main's index stay as they are.
GIT_INDEX_FILE="$(git rev-parse --git-dir)/release-index"
export GIT_INDEX_FILE
rm -f "$GIT_INDEX_FILE"
trap 'rm -f "$GIT_INDEX_FILE"' EXIT
git read-tree --empty
for p in $SHIPPED; do git ls-tree -r HEAD -- "$p"; done | git update-index --index-info
git add -f "$skill/denden.exe" "$skill/bin/linux-x64/denden" "$skill/SHA256SUMS"
# A Linux checkout runs these directly (Codex starts ./skills/im/denden without a shell).
git update-index --chmod=+x "$skill/denden" "$skill/bin/linux-x64/denden"
tree=$(git write-tree)

from=$(git rev-parse --short HEAD)
parent=$(git rev-parse -q --verify refs/heads/release || true)
commit=$(git commit-tree "$tree" ${parent:+-p "$parent"} -m "denden $version (from main $from)")
git update-ref refs/heads/release "$commit" $parent
git tag -a "v$version" -m "denden $version" "$commit"

echo "release: denden $version, $(git rev-parse --short "$commit") (from main $from), tagged v$version"
git ls-tree -r --long release -- "$skill/denden.exe" "$skill/bin/linux-x64/denden" "$skill/denden"
echo "to publish: git push origin main release v$version"
