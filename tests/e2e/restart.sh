#!/bin/sh
# Finding a machine again. Two machines on one computer (two DENDEN_HOME), over the public DHT.
# Idle for 4 minutes, their connection must hold (the heartbeat used to break it) and nothing is
# started again. Then the server's daemon stops: the laptop, alone in the channel, starts its
# network again after 3 minutes (only its log says so, its conversation sees nothing). The server's
# daemon then starts again, and the two must find each other through the laptop's new swarm. About
# 10 minutes. The invitation code passes from page to page through a variable, never printed.
#
#   tests/e2e/restart.sh [path/to/denden]
set -u
BIN=${1:-target/debug/denden}
T=$(mktemp -d 2>/dev/null || echo "${TMPDIR:-/tmp}/denden-restart-$$")
mkdir -p "$T"
A="$T/laptop"
B="$T/server"
# DISPLAY: the page flow for invite and join, even on a machine without a desktop (no browser opens).
a() { DENDEN_HOME="$A" COMPUTERNAME=laptop-rs HOSTNAME=laptop-rs DENDEN_NO_BROWSER=1 DISPLAY=${DISPLAY:-:0} "$BIN" "$@"; }
b() { DENDEN_HOME="$B" COMPUTERNAME=srv-rs HOSTNAME=srv-rs DENDEN_NO_BROWSER=1 DISPLAY=${DISPLAY:-:0} "$BIN" "$@"; }
step() { printf '\n== %s\n' "$*"; }
fails=0
expect() { # expect <what> <pattern> <text>
  if printf '%s' "$3" | grep -q -- "$2"; then echo "   ok: $1"; else echo "   FAIL: $1 (wanted: $2)"; fails=$((fails + 1)); fi
}
url_in() { # the local page URL in a file, waited for (a debug build starts its daemon slowly)
  for _ in $(seq 1 300); do
    u=$(grep -o 'http://127\.0\.0\.1:[0-9]*/p/[0-9a-f]*' "$1" 2>/dev/null | head -1)
    [ -n "$u" ] && { echo "$u"; return 0; }
    sleep 0.2
  done
  return 1
}
page_post() { # page_post <url> <action> <json>
  origin=$(printf '%s' "$1" | grep -o 'http://127\.0\.0\.1:[0-9]*')
  curl -s -X POST "$1/api/$2" -H 'content-type: application/json' -H 'x-denden: 1' -H "Origin: $origin" -d "$3"
}
api_state() { # what the laptop's who says of api
  a who --session aaa | grep '| api |'
}
wait_api() { # wait_api <online|offline> <seconds>: the state, and how long it took
  start=$(date +%s)
  state=""
  while [ $(($(date +%s) - start)) -lt "$2" ]; do
    state=$(api_state)
    printf '%s' "$state" | grep -q "$1" && break
    sleep 2
  done
  echo "$state"
  echo "   (after $(($(date +%s) - start)) s)"
}
restarts() { grep -c "network started again" "$A/daemon.log"; }
announces() { grep -c "announced on the DHT" "$A/daemon.log"; }

step "laptop: create the channel, attach coord"
out=$(a create rs-chan as coord --session aaa); echo "$out"
expect "attached" "This conversation is coord in rs-chan" "$out"

step "server: join with an invitation from the laptop"
a invite --session aaa --no-browser > "$T/invite.out" 2>&1 &
inv_url=$(url_in "$T/invite.out") || { echo "no invite page"; cat "$T/invite.out"; exit 1; }
code=$(page_post "$inv_url" reveal '{}' | sed -n 's/.*"code":"\([^"]*\)".*/\1/p')
wait
b join rs-chan as api --session ccc --no-browser > "$T/join.out" 2>&1 &
join_pid=$!
join_url=$(url_in "$T/join.out") || { echo "no join page"; cat "$T/join.out"; exit 1; }
page_post "$join_url" join "{\"code\":\"$code\"}" > /dev/null
code=""
wait $join_pid
out=$(cat "$T/join.out"); echo "$out"
expect "api attached" "This conversation is api in rs-chan" "$out"
out=$(wait_api online 30); echo "$out"
expect "api online" "online" "$out"

step "both online and idle for 4 minutes: the connection holds, and nothing is started again"
sleep 240
if grep -q "offline:" "$A/daemon.log" "$B/daemon.log"; then echo "   FAIL: a live connection was closed"; grep "offline:\|no answer" "$A/daemon.log" "$B/daemon.log"; fails=$((fails + 1)); else echo "   ok: the connection held"; fi
expect "nothing started again" "^0$" "$(restarts)"
out=$(api_state); echo "$out"
expect "api still online" "online" "$out"

step "server: the daemon stops, the laptop is alone"
b stop
stopped=$(date +%s)
out=$(wait_api offline 60); echo "$out"
expect "api offline on the laptop" "offline" "$out"
before=$(announces)
while [ "$(restarts)" -eq 0 ] && [ $(($(date +%s) - stopped)) -lt 330 ]; do sleep 5; done
took=$(($(date +%s) - stopped))
grep "network started again" "$A/daemon.log"
echo "   (after ${took} s)"
expect "the laptop started its network again" "^1$" "$(restarts)"
[ "$took" -ge 170 ] && echo "   ok: not before 3 minutes" || { echo "   FAIL: after ${took} s only"; fails=$((fails + 1)); }
for _ in $(seq 1 60); do [ "$(announces)" -gt "$before" ] && break; sleep 2; done
[ "$(announces)" -gt "$before" ] && echo "   ok: announced again" || { echo "   FAIL: no new announce"; fails=$((fails + 1)); }
out=$(a inbox --session aaa); echo "$out"
if printf '%s' "$out" | grep -qi "network\|started again"; then echo "   FAIL: the conversation was told"; fails=$((fails + 1)); else echo "   ok: nothing told to the conversation"; fi

step "server: the daemon starts again, and finds the laptop's new swarm"
b who --session ccc > /dev/null
out=$(wait_api online 150); echo "$out"
expect "api online again" "online" "$out"
out=$(a send api "sent after the restart" --session aaa); echo "$out"
expect "sent" "→ api" "$out"
start=$(date +%s)
out=""
while [ $(($(date +%s) - start)) -lt 60 ]; do
  out=$(b inbox --session ccc)
  printf '%s' "$out" | grep -q "sent after the restart" && break
  sleep 2
done
echo "$out"
expect "delivered" "sent after the restart" "$out"
expect "one restart only" "^1$" "$(restarts)"

step "stop both daemons"
a stop; b stop
echo
if [ "$fails" -eq 0 ]; then echo "ALL OK"; else echo "$fails FAILURE(S)"; fi
echo "(data in $T)"
exit "$fails"
