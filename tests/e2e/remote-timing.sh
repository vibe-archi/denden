#!/bin/sh
# Late messages and the wake-up budget, between this computer and a remote machine over SSH (see
# remote.sh). The late threshold is lowered to 30 s on the remote side (DENDEN_LATE_AFTER_SECONDS).
#
#   tests/e2e/remote-timing.sh <user@host> <ssh-key> [local denden binary]
set -u
TARGET=$1
KEY=$2
BIN=${3:-plugin/skills/im/denden.exe}
SSH="ssh -i $KEY -o BatchMode=yes $TARGET"
RB='DENDEN_HOME=$HOME/denden-test/home DENDEN_NO_BROWSER=1 DENDEN_LATE_AFTER_SECONDS=30 $HOME/denden-test/denden'
A=$(mktemp -d)
a() { DENDEN_HOME="$A" COMPUTERNAME=ezprime-test HOSTNAME=ezprime-test DENDEN_NO_BROWSER=1 "$BIN" "$@" --session local-coord; }
r() { $SSH "DENDEN_CONV=claude:remote-api $RB $*"; }
step() { printf '\n== %s\n' "$*"; }
fails=0
expect() {
  if printf '%s' "$3" | grep -q -- "$2"; then echo "   ok: $1"; else echo "   FAIL: $1 (wanted: $2)"; fails=$((fails + 1)); fi
}
refuse() {
  if printf '%s' "$3" | grep -q -- "$2"; then echo "   FAIL: $1 (unwanted: $2)"; fails=$((fails + 1)); else echo "   ok: $1"; fi
}

step "setup: channel timing, the remote machine joins, api attaches"
$SSH "$RB stop; rm -rf ~/denden-test/home/channels ~/denden-test/home/conversations.json ~/denden-test/home/wakes.json" > /dev/null 2>&1
a create timing as coord > /dev/null
a invite --no-browser > "$A/invite.out" 2>&1 &
for _ in $(seq 1 50); do url=$(grep -o 'http://127\.0\.0\.1:[0-9]*/p/[0-9a-f]*' "$A/invite.out" 2>/dev/null | head -1); [ -n "$url" ] && break; sleep 0.2; done
origin=$(printf '%s' "$url" | grep -o 'http://127\.0\.0\.1:[0-9]*')
code=$(curl -s -X POST "$url/api/reveal" -H 'content-type: application/json' -H 'x-denden: 1' -H "Origin: $origin" -d '{}' | sed -n 's/.*"code":"\([^"]*\)".*/\1/p')
wait
out=$( (sleep 4; printf '%s\n' "$code") | $SSH -tt "$RB join timing" 2>&1 | tr -d '\r' | sed -E 's/denden_[A-Za-z0-9_-]{20,}/denden_<masked>/g')
code=""
expect "admitted" "joined timing" "$out"
out=$(r join timing as api); expect "api attached" "This conversation is api in timing" "$out"
r inbox > /dev/null

step "late: held by the sender while the remote daemon is down"
$SSH "$RB stop" > /dev/null
a send api "Deploy the server now" > /dev/null
echo "   (waiting 40 s)"; sleep 40
$SSH "DENDEN_CONV=claude:remote-api $RB" > /dev/null
for _ in $(seq 1 20); do out=$(r inbox --peek); printf '%s' "$out" | grep -q "Deploy the server" && break; sleep 3; done
out=$(r inbox); echo "$out"
expect "flagged late" "late, sent" "$out"
expect "header" "1 message arrived while you were away" "$out"
expect "the user is asked" "Which ones should I handle" "$out"

step "late: arrived at once, read 40 s later"
a send api "Restart the cache" > /dev/null
sleep 3
out=$(r inbox --peek); expect "arrived" "Restart the cache" "$out"
refuse "not late yet" "late, sent" "$out"
echo "   (waiting 40 s)"; sleep 40
out=$(r inbox); echo "$out"
expect "late now" "late, sent" "$out"

step "not late: read at once"
a send api "Fresh news" > /dev/null
sleep 3
out=$(r inbox); echo "$out"
refuse "not flagged" "late" "$out"

step "budget: 2 wake-ups an hour for api"
out=$(r wakes 2); expect "budget set" "Up to 2 wake-ups per hour for api" "$out"
$SSH "DENDEN_CONV=claude:remote-api $RB watch --timeout 45 > ~/denden-test/watch.out 2>&1 &"
sleep 3
for i in 1 2 3; do a send api "Budget message $i" > /dev/null; sleep 5; done
sleep 5
out=$($SSH 'cat ~/denden-test/watch.out'); echo "$out"
expect "first wakes" "Budget message 1" "$out"
expect "second wakes" "Budget message 2" "$out"
refuse "third held back" "Budget message 3" "$out"
expect "limit told, with how to raise it" "Limit of 2 wake-ups per hour reached.*wakes off" "$out"
step "budget: the held message comes with the user's next prompt"
out=$($SSH "DENDEN_HOME=\$HOME/denden-test/home CLAUDECODE=1 \$HOME/denden-test/denden hook-prompt" <<'EOF'
{"session_id":"remote-api","prompt":"what's new?","cwd":"/home/ai"}
EOF
); echo "$out" | cut -c1-300
expect "handed over as context" "Budget message 3" "$out"
expect "as additional context" "additionalContext" "$out"
r wakes off > /dev/null

step "stop"
a stop; $SSH "$RB stop" > /dev/null
echo
if [ "$fails" -eq 0 ]; then echo "ALL OK"; else echo "$fails FAILURE(S)"; fi
exit "$fails"
