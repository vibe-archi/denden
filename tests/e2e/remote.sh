#!/bin/sh
# Two machines on two networks: this computer and a remote one over SSH (key-based). A throwaway
# channel on both sides (separate DENDEN_HOME): invitation, terminal join (the code typed into a
# pseudo-terminal, hidden), attach, ask/reply, send to all, and a message queued while the remote
# daemon is down. The invitation code goes from the page to the remote terminal through a variable.
#
#   tests/e2e/remote.sh <user@host> <ssh-key> [local denden binary]
# The remote needs the Linux binary at ~/denden-test/denden.
set -u
TARGET=$1
KEY=$2
BIN=${3:-plugin/skills/im/denden.exe}
SSH="ssh -i $KEY -o BatchMode=yes $TARGET"
RB='DENDEN_HOME=$HOME/denden-test/home DENDEN_NO_BROWSER=1 $HOME/denden-test/denden'
A=$(mktemp -d)
a() { DENDEN_HOME="$A" COMPUTERNAME=ezprime-test HOSTNAME=ezprime-test DENDEN_NO_BROWSER=1 "$BIN" "$@"; }
r() { $SSH "DENDEN_CONV=claude:remote-api $RB $*"; }
step() { printf '\n== %s\n' "$*"; }
fails=0
expect() {
  if printf '%s' "$3" | grep -q -- "$2"; then echo "   ok: $1"; else echo "   FAIL: $1 (wanted: $2)"; fails=$((fails + 1)); fi
}

step "this computer: create xnet, attach coord"
out=$(a create xnet as coord --session local-coord); echo "$out"
expect "attached" "This conversation is coord in xnet" "$out"

step "this computer: invitation"
a invite --session local-coord --no-browser > "$A/invite.out" 2>&1 &
for _ in $(seq 1 50); do url=$(grep -o 'http://127\.0\.0\.1:[0-9]*/p/[0-9a-f]*' "$A/invite.out" 2>/dev/null | head -1); [ -n "$url" ] && break; sleep 0.2; done
origin=$(printf '%s' "$url" | grep -o 'http://127\.0\.0\.1:[0-9]*')
code=$(curl -s -X POST "$url/api/reveal" -H 'content-type: application/json' -H 'x-denden: 1' -H "Origin: $origin" -d '{}' | sed -n 's/.*"code":"\([^"]*\)".*/\1/p')
[ -n "$code" ] && echo "   ok: code taken from the page (not printed)" || { echo "   FAIL: no code"; exit 1; }
wait

step "remote: the machine joins from its terminal (code typed, hidden)"
t0=$(date +%s)
# The code goes in once the prompt has turned echo off (a pseudo-terminal echoes what arrives
# before), and anything that looks like a code is masked in what comes back, whatever happens.
out=$( (sleep 4; printf '%s\n' "$code") | $SSH -tt "$RB join xnet" 2>&1 | tr -d '\r' | sed -E 's/denden_[A-Za-z0-9_-]{20,}/denden_<masked>/g'); echo "$out"
if printf '%s' "$out" | grep -q 'denden_<masked>'; then echo "   FAIL: the terminal echoed the code (masked here)"; fails=$((fails + 1)); else echo "   ok: the code was not echoed"; fi
code=""
echo "   ($(( $(date +%s) - t0 )) s)"
expect "admitted" "joined xnet" "$out"

step "remote: a conversation attaches api"
out=$(r join xnet as api); echo "$out"
expect "api attached" "This conversation is api in xnet" "$out"

step "this computer: who"
out=$(a who --session local-coord); echo "$out"
expect "api seen" "| api |" "$out"

step "ask across the networks, then reply"
out=$(a ask api "Ping across networks?" --session local-coord); echo "$out"
sleep 3
out=$(r inbox); echo "$out"
expect "question received" "coord · reply expected #" "$out"
id=$(printf '%s' "$out" | grep -o '#[0-9a-f]*' | head -1 | tr -d '#')
out=$(r reply "$id" "Pong from vm1"); echo "$out"
sleep 3
out=$(a inbox --session local-coord); echo "$out"
expect "reply received" "api · reply : Pong from vm1" "$out"

step "remote: to the whole channel"
out=$(r send all "Hello everyone from vm1"); echo "$out"
sleep 3
out=$(a inbox --session local-coord); echo "$out"
expect "coord got it" "api · to all : Hello everyone from vm1" "$out"

step "remote daemon down: the message waits here, then goes"
$SSH "$RB stop" > /dev/null
sleep 2
out=$(a send api "Sent while you were down" --session local-coord); echo "$out"
expect "queued" "offline: queued" "$out"
$SSH "DENDEN_CONV=claude:remote-api $RB" > /dev/null # starts the remote daemon again
for i in $(seq 1 20); do
  out=$(r inbox --peek)
  printf '%s' "$out" | grep -q "Sent while you were down" && break
  sleep 3
done
echo "$out"
expect "delivered after the restart" "coord : Sent while you were down" "$out"

step "logs (end)"
echo "-- this computer"; tail -n 25 "$A/daemon.log"
echo "-- remote"; $SSH 'tail -n 25 ~/denden-test/home/daemon.log'

step "stop"
a stop; $SSH "$RB stop"
echo
if [ "$fails" -eq 0 ]; then echo "ALL OK"; else echo "$fails FAILURE(S)"; fi
echo "(local data in $A)"
exit "$fails"
