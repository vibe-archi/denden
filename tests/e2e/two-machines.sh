#!/bin/sh
# Two machines on one computer (two DENDEN_HOME, two machine names), over the public DHT: create,
# attach, invite, join, ask/reply, send to all, who, a new role, take-over, refusal of a name held elsewhere,
# and a Claude listener. Invitation codes pass from page to page through variables, never printed.
#
#   tests/e2e/two-machines.sh [path/to/denden]
set -u
BIN=${1:-target/debug/denden}
[ -x "$BIN" ] || BIN="$BIN.exe"
T=$(mktemp -d 2>/dev/null || echo "${TMPDIR:-/tmp}/denden-e2e-$$")
mkdir -p "$T"
A="$T/laptop"
B="$T/server"
a() { DENDEN_HOME="$A" COMPUTERNAME=laptop-ak HOSTNAME=laptop-ak DENDEN_NO_BROWSER=1 "$BIN" "$@"; }
b() { DENDEN_HOME="$B" COMPUTERNAME=srv-api HOSTNAME=srv-api DENDEN_NO_BROWSER=1 "$BIN" "$@"; }
step() { printf '\n== %s\n' "$*"; }
fails=0
expect() { # expect <what> <pattern> <text>
  if printf '%s' "$3" | grep -q -- "$2"; then echo "   ok: $1"; else echo "   FAIL: $1 (wanted: $2)"; fails=$((fails + 1)); fi
}
expect_no() { # expect_no <what> <pattern> <text>
  if printf '%s' "$3" | grep -q -- "$2"; then echo "   FAIL: $1 (not wanted: $2)"; fails=$((fails + 1)); else echo "   ok: $1"; fi
}
url_in() { # the local page URL in a file, waited for
  for _ in $(seq 1 100); do
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

step "laptop: create the channel, attach coord"
out=$(a create vibedoc-chan as coord --session aaa); echo "$out"
expect "created" "Channel vibedoc-chan created on laptop-ak" "$out"
expect "attached" "This conversation is coord in vibedoc-chan" "$out"

step "laptop: create without a name asks for one"
out=$(a create other-chan --session zzz); code=$?; echo "$out"
expect "question" "Which name for the denden of this conversation" "$out"
expect "exit 3" "3" "$code"

step "laptop: a second conversation attaches front, no code"
out=$(a join vibedoc-chan as front --session bbb); echo "$out"
expect "front attached" "This conversation is front in vibedoc-chan" "$out"

step "laptop: status"
out=$(a --session aaa); echo "$out"
expect "status line" "coord in vibedoc-chan · auto off" "$out"

step "laptop: invitation page, server: join page"
a invite --session aaa --no-browser > "$T/invite.out" 2>&1 &
inv_url=$(url_in "$T/invite.out") || { echo "no invite page"; cat "$T/invite.out"; exit 1; }
code=$(page_post "$inv_url" reveal '{}' | sed -n 's/.*"code":"\([^"]*\)".*/\1/p')
[ -n "$code" ] && echo "   ok: code revealed once (not printed)" || { echo "   FAIL: no code"; fails=$((fails + 1)); }
again=$(page_post "$inv_url" reveal '{}'); expect "single reveal" "already revealed" "$again"
wait
cat "$T/invite.out"
b join vibedoc-chan as api --session ccc --no-browser > "$T/join.out" 2>&1 &
join_pid=$!
join_url=$(url_in "$T/join.out") || { echo "no join page"; cat "$T/join.out"; exit 1; }
bad=$(page_post "$join_url" join '{"code":"denden_garbage"}'); expect "bad code refused on the page" "error" "$bad"
page_post "$join_url" join "{\"code\":\"$code\"}" > /dev/null
code=""
wait $join_pid
out=$(cat "$T/join.out"); echo "$out"
expect "server joined" "srv-api joined vibedoc-chan, invited by laptop-ak" "$out"
expect "api attached" "This conversation is api in vibedoc-chan" "$out"

step "server: coord is taken on another machine"
out=$(b join vibedoc-chan as coord --session ddd); code=$?; echo "$out"
expect "refused, other name asked" "coord already exists in vibedoc-chan, on another machine" "$out"
expect "exit 3" "3" "$code"

step "laptop: coord asks api"
out=$(a ask api "Is the /users endpoint ready?" --session aaa); echo "$out"
expect "sent with reply expected" "→ api · reply expected" "$out"
sleep 2
step "server: api reads and replies"
out=$(b inbox --session ccc); echo "$out"
expect "question received" "coord · reply expected #" "$out"
id=$(printf '%s' "$out" | grep -o '#[0-9a-f]*' | head -1 | tr -d '#')
out=$(b reply "$id" "Yes, on feat/users, tests green" --session ccc); echo "$out"
expect "reply sent" "→ coord · reply" "$out"
out=$(b reply "$id" "again" --session ccc); echo "$out"
expect "no second reply" "answered already" "$out"
sleep 2
out=$(a inbox --session aaa); echo "$out"
expect "reply received" "api · reply : Yes, on feat/users" "$out"

step "server: a message without reply expected cannot be answered"
out=$(a send api "FYI: freeze at 18:00" --session aaa); echo "$out"
sleep 2
out=$(b inbox --session ccc); echo "$out"
id=$(printf '%s' "$out" | grep -o '#[0-9a-f]*' | head -1 | tr -d '#')
expect "no id shown (no reply expected)" "coord : FYI" "$out"

step "server: to the whole channel"
out=$(b send all "Restart your services" --session ccc); echo "$out"
expect "to all" "→ all (2 denden)" "$out"
sleep 2
out=$(a inbox --session bbb); echo "$out"
expect "front got it" "api · to all : Restart your services" "$out"
out=$(a inbox --session aaa); echo "$out"
expect "coord got it" "api · to all : Restart your services" "$out"

step "who, from both machines"
out=$(a who --session aaa); echo "$out"
expect "api on srv-api" "| api | srv-api" "$out"
out=$(b role "API server" --session ccc); echo "$out"
sleep 2
out=$(b who --session ccc); echo "$out"
expect "coord on laptop" "| coord | laptop-ak" "$out"
out=$(a who --session aaa); echo "$out"
expect "role propagated" "API server" "$out"
out=$(a inbox --session aaa); echo "$out"
expect "new role announced" 'New role for api: "API server"' "$out"
out=$(b inbox --session ccc); echo "$out"
expect_no "not to the denden that changed it" "New role for api" "$out"

step "laptop: a new conversation takes front over (the old one does not listen)"
out=$(a join vibedoc-chan as front --session eee); echo "$out"
expect "taken over" "front taken over here" "$out"
out=$(a --session bbb); echo "$out"
expect "old one detached" "not attached" "$out"

step "laptop: a listener on front, server sends"
a watch --session eee --timeout 15 > "$T/watch.out" 2>&1 &
watch_pid=$!
sleep 3
b send front "Ping from the server" --session ccc > /dev/null
sleep 6
out=$(cat "$T/watch.out"); echo "$out"
expect "listener printed it" "api : Ping from the server" "$out"
step "laptop: moving a listening denden needs the user's go-ahead"
out=$(a join vibedoc-chan as front --session fff); code=$?; echo "$out"
expect "move question" "front is attached to" "$out"
expect "exit 3" "3" "$code"
out=$(a join vibedoc-chan as front --session fff --move); echo "$out"
expect "moved" "This conversation is now front in vibedoc-chan" "$out"
wait $watch_pid
out=$(cat "$T/watch.out"); echo "$out"
expect "old listener told and stopped" "front was moved to" "$out"

step "history"
out=$(a history --session aaa); echo "$out"
expect "history has both directions" "→ api · reply expected" "$out"

step "stop both daemons"
a stop; b stop
echo
if [ "$fails" -eq 0 ]; then echo "ALL OK"; else echo "$fails FAILURE(S)"; fi
echo "(data in $T)"
exit "$fails"
