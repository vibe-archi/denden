#!/bin/sh
# The heartbeat. Two machines on one computer (two DENDEN_HOME), over the public DHT. The server's
# daemon is frozen (SIGSTOP): the laptop must close the connection for want of an answer and show
# the server offline; once the daemon runs again (SIGCONT), the two connect again by themselves and
# what waited is delivered. Linux and macOS only (signals). The invitation code passes from page to
# page through a variable, never printed.
#
#   tests/e2e/heartbeat.sh [path/to/denden]
set -u
BIN=${1:-target/debug/denden}
T=$(mktemp -d 2>/dev/null || echo "${TMPDIR:-/tmp}/denden-hb-$$")
mkdir -p "$T"
A="$T/laptop"
B="$T/server"
# DISPLAY: the page flow for invite and join, even on a machine without a desktop (no browser opens).
a() { DENDEN_HOME="$A" COMPUTERNAME=laptop-hb HOSTNAME=laptop-hb DENDEN_NO_BROWSER=1 DISPLAY=${DISPLAY:-:0} "$BIN" "$@"; }
b() { DENDEN_HOME="$B" COMPUTERNAME=srv-hb HOSTNAME=srv-hb DENDEN_NO_BROWSER=1 DISPLAY=${DISPLAY:-:0} "$BIN" "$@"; }
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

step "laptop: create the channel, attach coord"
out=$(a create hb-chan as coord --session aaa); echo "$out"
expect "attached" "This conversation is coord in hb-chan" "$out"

step "server: join with an invitation from the laptop"
a invite --session aaa --no-browser > "$T/invite.out" 2>&1 &
inv_url=$(url_in "$T/invite.out") || { echo "no invite page"; cat "$T/invite.out"; exit 1; }
code=$(page_post "$inv_url" reveal '{}' | sed -n 's/.*"code":"\([^"]*\)".*/\1/p')
wait
b join hb-chan as api --session ccc --no-browser > "$T/join.out" 2>&1 &
join_pid=$!
join_url=$(url_in "$T/join.out") || { echo "no join page"; cat "$T/join.out"; exit 1; }
page_post "$join_url" join "{\"code\":\"$code\"}" > /dev/null
code=""
wait $join_pid
out=$(cat "$T/join.out"); echo "$out"
expect "api attached" "This conversation is api in hb-chan" "$out"
sleep 2
out=$(api_state); echo "$out"
expect "api online" "online" "$out"

step "both idle for 45 s: the pongs keep the connection open"
sleep 45
out=$(api_state); echo "$out"
expect "api still online" "online" "$out"
if grep -q "no answer\|offline:" "$A/daemon.log" "$B/daemon.log"; then echo "   FAIL: a live connection was closed"; fails=$((fails + 1)); else echo "   ok: nothing closed"; fi

step "server: the daemon freezes"
pid=$(sed -n 's/.*"pid": *\([0-9]*\).*/\1/p' "$B/daemon.json")
[ -n "$pid" ] || { echo "no server daemon pid"; exit 1; }
kill -STOP "$pid"
start=$(date +%s)
state=""
while [ $(($(date +%s) - start)) -lt 90 ]; do
  state=$(api_state)
  printf '%s' "$state" | grep -q offline && break
  sleep 2
done
took=$(($(date +%s) - start))
echo "$state"
echo "   (after ${took} s)"
expect "api offline on the laptop" "offline" "$state"
[ "$took" -le 60 ] && echo "   ok: within 60 s" || { echo "   FAIL: took ${took} s"; fails=$((fails + 1)); }
expect "closed by the heartbeat" "no answer from srv-hb" "$(cat "$A/daemon.log")"

step "laptop: a message while the server is frozen waits"
out=$(a send api "sent during the freeze" --session aaa); echo "$out"
expect "queued" "offline: queued" "$out"

step "server: the daemon runs again"
kill -CONT "$pid"
start=$(date +%s)
out=""
while [ $(($(date +%s) - start)) -lt 90 ]; do
  out=$(b inbox --session ccc)
  printf '%s' "$out" | grep -q "sent during the freeze" && break
  sleep 2
done
echo "$out"
echo "   (after $(($(date +%s) - start)) s)"
expect "delivered once connected again" "sent during the freeze" "$out"
out=$(api_state); echo "$out"
expect "api online again" "online" "$out"

step "stop both daemons"
a stop; b stop
echo
if [ "$fails" -eq 0 ]; then echo "ALL OK"; else echo "$fails FAILURE(S)"; fi
echo "(data in $T)"
exit "$fails"
