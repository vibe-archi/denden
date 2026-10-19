#!/bin/sh
# Both machines behind a NAT: this computer (behind its router) and a machine "natbox" inside a
# network namespace behind a NAT made by the remote host (see /tmp/nat-setup.sh there). Commands
# run inside the namespace through a tmux server started in it (socket /tmp/denden-nat.sock).
#
#   tests/e2e/nat.sh <user@host> <ssh-key> [local denden binary]
set -u
TARGET=$1
KEY=$2
BIN=${3:-plugin/skills/im/denden.exe}
SSH="ssh -i $KEY -o BatchMode=yes $TARGET"
T='tmux -S /tmp/denden-nat.sock'
# REMOTE_BIN / REMOTE_ENV: another binary in ~/denden-test (a trace build) and extra variables.
NBT='DENDEN_HOME=$HOME/denden-nat/home DENDEN_NO_BROWSER=1 HOSTNAME=natbox '"${REMOTE_ENV:-}"' $HOME/denden-test/'"${REMOTE_BIN:-denden}"
# The same with a simulated conversation: never for the terminal join, which has none (with one,
# join serves the page flow and nothing turns echo off).
NB="DENDEN_CONV=claude:nat-api $NBT"
A=$(mktemp -d)
a() { DENDEN_HOME="$A" COMPUTERNAME=ezprime-test HOSTNAME=ezprime-test DENDEN_NO_BROWSER=1 "$BIN" "$@" --session local-coord; }
# A command inside the namespace, its output back here.
n() {
  $SSH "rm -f /tmp/dn.out; $T new-window -d \"$NB $* > /tmp/dn.out 2>&1; $T wait-for -S dn-done\"; $T wait-for dn-done; cat /tmp/dn.out"
}
step() { printf '\n== %s\n' "$*"; }
fails=0
expect() {
  if printf '%s' "$3" | grep -q -- "$2"; then echo "   ok: $1"; else echo "   FAIL: $1 (wanted: $2)"; fails=$((fails + 1)); fi
}

step "natbox: its address on the Internet (behind the NAT)"
$SSH "$T new-window -d \"ip -4 -o addr show dn-ns > /tmp/dn.ip; $T wait-for -S dn-ip\"; $T wait-for dn-ip; cat /tmp/dn.ip" | grep -o 'inet [0-9./]*'
$SSH "$NB stop; rm -rf ~/denden-nat" > /dev/null 2>&1

step "this computer: create nattest, invitation"
a create nattest as coord > /dev/null
a invite --no-browser > "$A/invite.out" 2>&1 &
for _ in $(seq 1 50); do url=$(grep -o 'http://127\.0\.0\.1:[0-9]*/p/[0-9a-f]*' "$A/invite.out" 2>/dev/null | head -1); [ -n "$url" ] && break; sleep 0.2; done
origin=$(printf '%s' "$url" | grep -o 'http://127\.0\.0\.1:[0-9]*')
code=$(curl -s -X POST "$url/api/reveal" -H 'content-type: application/json' -H 'x-denden: 1' -H "Origin: $origin" -d '{}' | sed -n 's/.*"code":"\([^"]*\)".*/\1/p')
wait
[ -n "$code" ] && echo "   ok: code taken from the page (not printed)"

step "natbox: joins from its terminal, inside the namespace"
t0=$(date +%s)
$SSH "rm -f /tmp/dn.join; $T new-window -d -n join \"$NBT join nattest > /tmp/dn.join 2>&1; $T wait-for -S dn-join\""
# Typed only once the hidden prompt is up (echo off), and that pane is never read.
prompt=no
for _ in $(seq 1 40); do $SSH "grep -q 'Invitation code:' /tmp/dn.join 2>/dev/null" && { prompt=yes; break; }; sleep 0.5; done
if [ "$prompt" != yes ]; then echo "   FAIL: no hidden prompt, the code is not typed"; code=""; exit 1; fi
$SSH "$T send-keys -t natside:join -l '$code'; $T send-keys -t natside:join Enter"
code=""
$SSH "$T wait-for dn-join; cat /tmp/dn.join" > "$A/join.out"
out=$(sed -E 's/denden_[A-Za-z0-9_-]{20,}/denden_<masked>/g' "$A/join.out"); echo "$out"
echo "   ($(( $(date +%s) - t0 )) s)"
expect "admitted through both NATs" "joined nattest" "$out"

step "natbox: attach api, then talk"
out=$(n join nattest as api); echo "$out"
expect "api attached" "This conversation is api in nattest" "$out"
out=$(a ask api "Do you hear me through both NATs?"); echo "$out"
sleep 4
out=$(n inbox); echo "$out"
expect "question received" "coord · reply expected #" "$out"
id=$(printf '%s' "$out" | grep -o '#[0-9a-f]*' | head -1 | tr -d '#')
out=$(n reply "$id" "Loud and clear"); echo "$out"
sleep 4
out=$(a inbox); echo "$out"
expect "reply received" "api · reply : Loud and clear" "$out"

step "logs (end)"
echo "-- this computer"; tail -n 8 "$A/daemon.log"
echo "-- natbox"; $SSH 'tail -n 8 ~/denden-nat/home/daemon.log'

step "stop"
a stop; n stop
echo
if [ "$fails" -eq 0 ]; then echo "ALL OK"; else echo "$fails FAILURE(S)"; fi
exit "$fails"
