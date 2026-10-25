# denden: technical architecture

This document turns [design.md](design.md) into technical choices. It describes what the code does; the design document remains the reference for the user experience.

## 1. Choices made for the open points

Points of section 13 of design.md, settled here to move forward. They can still be revised:
- **Language of the fixed texts: English.** It is the language of the first approved examples (`Received message from bob@project-xx`) and that of a distributed plugin. The AI relays these lines in the user's language, keeping the `> 🐌» (( … ))` shape. The local pages are in English or French, after the browser's `Accept-Language`.
- **Outputs on several lines**: a command gathers what it prints for the conversation and writes it at the end as one block, `> 🐌» ((`, one `> ` line each, `> ))` (`output::group`); a single line keeps `> 🐌» (( … ))`, a question comes after the block. Lines that cannot wait (a local page's address, printed before the page waits) go out at once. `who` puts its table in the block; `watch --once` writes its batch as one block. The skill asks the AI to group what it relays the same way.
- **`DENDEN_AS` goes away.** A conversation is identified by its session: `--session ${CLAUDE_SESSION_ID}` for Claude, `CODEX_THREAD_ID` for Codex.
- **Protocol version: 3.** The derivation labels (`agent-im/v2/...`) do not change; what changes is the exchanges, and the opening proof announces `v: 3`. No compatibility with agent-im is sought.
- **New machine notice**: it goes to every conversation attached to the channel, on the machines that see the newcomer arrive, without waking them. It is handed to them with their next messages.
- **Sensitive commands**: on top of `invite`, `block`, `unblock` and `auto`, the skill forbids running `join`, `detach`, `delete`, `leave`, `wakes` and `keepalive` because a message asked.
- **Invitation page after the copy**: it shows the next step, then closes after 60 seconds. The join page does the same after a successful join.
- **Duplicate channel name (question 33)**: the local name of a channel is the one the user types in `join`. `join vibedoc-paul as front` joins, under that local name, the channel of the pasted code, whatever name its creator gave it; `join` on a name already present attaches the conversation to that channel.

## 2. Data on disk (`~/.denden`)

```text
~/.denden/
  daemon.json  daemon.lock  daemon.log   the machine's daemon (daemon.json: port, token, pid, build)
  restart.json                           a command is restarting a daemon from an older build
  bin/                                   checked copies of the binary (Windows locks a running exe)
  conversations.json                     conversation -> channel, denden, application, mode, budget, paused
  wakes.json                             wake-up state per conversation (budget spent, Codex notice pending)
  settings.json                          keepalive, notices already given once
  channels/<cid>/                        one folder per channel; <cid> = first 16 hex of the channel id
    channel.json                         secret, local label, machine key seed, certificates, invitations
    members.json                         known machines (key -> name, last seen), blocked keys
    names.json                           name certificates: denden -> owner machine
    messages.jsonl                       messages received and sent, per local denden
    state.json                           what was read, per local denden
    outbox.json                          messages waiting, per receiving machine
```

- **One machine key per channel**, so that nobody can link its channels together. Its name is the cleaned host name.
- **A channel's folder carries its id**, not its label, which is only a local name chosen when joining.
- **A conversation** is written `claude:<session>` or `codex:<thread>`.

## 3. Protocol (version 3)

Inside each Noise-encrypted stream, JSON frames separated by newlines:

| frame | role |
|---|---|
| `{t:'auth', v:3, proof}` | proof of knowledge of the secret, before any other exchange (unchanged) |
| `{t:'hello', machine, key, sig, chain?, inv?, needCert?, names, members?}` | the machine introduces itself; `names` carries the name certificates it knows |
| `{t:'cert', chain}` | a member hands a membership certificate to the machine it just admitted |
| `{t:'names', names}` | new names, deletions, releases after a conflict |
| `{t:'msg', id, from, to, text, ts, ask?, re?, waited}` | a message; `to` is a list of denden or `"all"` |
| `{t:'ack', id}` / `{t:'nack', id, error}` | acknowledgement, or refusal |
| `{t:'error', error}` | refusal of the connection |

**Membership.** A machine is admitted with a single-use invitation token, then receives a certificate signed by the machine that invited it. The others accept it if the certificate chain goes back to a key they know. Certificates name machine keys: two machines may carry the same name.

**Name certificates.** `{n, k, t, u, app, role, on, d, r, sig}`: the denden `n` belongs to the machine with key `k`, created at `t`, last updated at `u`, signed by `k`. `app`, `role` and `on` (attached to a conversation) feed `who`. `d` marks a deletion (the name stays bound to the machine), `r` a release after a conflict. Each machine spreads them in its `hello` and through `names`. Rules:
- an unknown name is accepted if it is signed by a member machine;
- a name already bound to another machine is refused, unless that machine releases it (`r`);
- a conflict that touches one of our names: if the other machine created it before us, we release it and tell the conversation; otherwise we keep it;
- a refused name is reported as an impersonation (`⚠ <machine> [<fp>] claimed <name>, which belongs to <owner> in <channel>: refused`, a quiet alert to every conversation attached to the channel, once per machine and name while the daemon runs) only when the machine that knew whose it was still claims it: the certificate comes on the claimant's own connection, it is neither a deletion nor a release, the name is in use by a machine that created it earlier, and we showed the claimant that owner (in our `hello` or a `names` we sent on) at least a minute before. A machine that created a name without seeing it taken gives it up as soon as it sees the earlier creation, so a conflict is never reported this way; nor is a claim created before the owner's (it wins once the owner sees it), one relayed by another machine, or one for a deleted name. What each machine was shown is kept in memory only.

**Messages.**
- A machine can only write on behalf of its own denden: the sender `from` must belong to the key of the connection. Otherwise the message is refused (`bad-sender`) and only the sending conversation is told; the receiver reports nothing, impersonation being detected on name certificates.
- The receiver accepts only the names that belong to it; `"all"` stands for all its denden.
- Between two denden of the same machine, nothing goes through the network.
- `waited`: the time spent at the sender before this send, to measure the delay without depending on clocks.

## 4. The daemon

- **One daemon per machine**, a local HTTP API protected by a token (`x-denden-token`, from `daemon.json`), one swarm connection per channel and per epoch, as in agent-im. A command from a newer build restarts a daemon from an older one.
- **Routing**: a send is split per receiving machine. Each part waits in `outbox.json` until it is acknowledged, and it is sent again at each reconnection. After 7 days it is dropped, and the sending denden gets a notice. A send to `all` reports how many denden it reached.
- **Reading**: each denden has its own read state. A message is handed to a conversation by Claude's listener, by Codex's hooks, or by `inbox`.
- **Listening in Claude**: by default in silent mode, `watch --once` run as a background command. It prints nothing until messages arrive, gathers what comes within 1.5 s, prints it with a last line for the agent, and ends: the end of a background command wakes the conversation, which starts the listener again. It held 33 idle minutes in a test, longer than a Monitor lasts. The fallback is a Monitor on `watch --timeout 1790`, started again at each expiry. (Codex has no such wake-up: the end of a background terminal does not start a turn, so Codex keeps its queued notices.)
- **Pause**: a paused conversation gets no listener (a running `watch` gets a last notice and ends; a new one ends at once), no queued Codex notice and no Stop-hook handover. Its messages wait for the user's next prompt (the prompt hook) or for `resume`. Attaching a conversation again resumes it.
- **Lateness**: on handover, `now - (received - waited) >= 1 h` marks the message as late (`DENDEN_LATE_AFTER_SECONDS` changes the hour, for tests). A late message comes with the question "Which ones should I handle?", whatever the mode.
- **History**: messages older than 30 days are purged at startup, then every hour.
- **Lifetime**: the daemon stays up while a conversation of the machine is attached, or while one of its invitations is unused. Otherwise it stops after 10 minutes with no Claude listener open, no Codex MCP server connected, no dashboard open and no activity, unless `keepalive on` is set.
- **Wake-ups** (`wake.rs`): messages that arrive together are announced together (2 s). A budget per conversation, 20 per hour by default. For Claude, beyond it the listener stops printing, announces the limit once, and messages wait for the user's next prompt (the prompt hook hands them over). For Codex, one notice at a time is queued with `codex queue`, at least 30 s apart, not during a turn (a turn counts as finished after 15 min without a Stop hook); a notice blocks the next ones until it is read, a hook comes, or 10 min pass; after 3 failed `codex queue` runs, the conversation is no longer woken.

## 5. The dashboard

- **Served by the daemon**, on its local API port, under `/ui/<token>`: the page, and `/ui/<token>/api/state`. These routes need no API token (a browser has none); they need a valid page token instead, a loopback `Host` (against DNS rebinding), and, for the API, the `x-denden` header (a cross-site request cannot send it without a preflight that is never answered). The page has a nonce CSP.
- **Page tokens** come from `POST /ui` (an API call, so from the CLI only). Each lives while it is polled: 2 minutes without a poll (10 minutes before the first one), or 12 hours after it was made, and it is gone. An open page counts as presence for the daemon's lifetime.
- **`api/state`** returns, channel by channel: its machines (fingerprint as id, online, connecting, blocked, last seen); its denden (machine, application, role, held or not), and for this machine's ones what waits for them and the conversation holding them (application, title, folder, `auto`, budget, pause, how it listens: silent mode, Monitor or Codex notices); which messages each local denden has not been handed yet (`unseen`); and the messages. Messages are those of the machine's denden, once each (a message between two local denden is stored twice), with how long they waited at their sender; notices are left out, except the channel's events and alerts. A notice to the whole channel (`note_all`) is therefore also recorded once with no denden, marked `event`, and each denden's copy is marked `shared`. The page sends what it has (`after`, per channel) and gets only what is new; it works out itself which questions are answered. Reading marks nothing as read.
- **The page** is `plugin/skills/im/pages/dashboard.html`, embedded in the binary like `ui.html`, in English, polling every 2 s.
- **`ui` in the CLI** opens the system browser and never prints the address when it runs for an agent (a conversation, or no terminal); in the user's own terminal it prints it, and on a server without a desktop the SSH tunnel to reach it.

## 6. The CLI

- Every command goes through the daemon, which it starts if needed.
- The conversation comes from `--session` (Claude), from `CODEX_THREAD_ID` (Codex, brought back to the user's conversation for a thread spawned by Codex), from `DENDEN_CONV` (tests), or does not exist (terminal).
- **When a decision is up to the user**, the CLI does not wait for input. It prints the question as `🐌»  **…**` and exits with code 3. The AI asks the question, then runs the command again with the answer (`as <name>`, `--move`).
- **Joining**: once the code is accepted, `join` waits up to 90 s for the admission, then says why it is not there yet (refused, no machine of the channel reached, certificate still to come). In a terminal, `denden join <channel>` asks for the code without echoing it, showing only its start and end as it comes (raw terminal mode, one byte at a time), checks it (`decode_invite`) and asks again after a bad paste, then shows where the admission stands. On a server without a desktop, `invite` serves no page and points to `denden invite <channel> --print` in the user's terminal, which prints the code on a line of its own and sends it to the user's clipboard with OSC 52 (wrapped for tmux too). `denden link` puts a link to the plugin's launcher in `~/.local/bin`, and the messages then say just `denden`.
- **The Codex sandbox** blocks the network and `~/.denden`. When the daemon cannot be reached from inside it, the CLI says so and asks for the command to be run outside the sandbox.
- **Internal commands**, not shown to the user: `inbox`, `watch` (and `watch --once`), `reply <id> <text>`, `hook-prompt`, `hook-stop`, `mcp` (the MCP server behind the Codex hooks), `stop`, and `__daemon` / `__launch` (starting the daemon).

## 7. Phases

All the phases are done; phase 7 was run against a Linux server, with Claude Code and Codex, on two networks.

| # | Phase | Content | End of phase |
|---|---|---|---|
| 1 | Foundation | repository, workspace, renamed crate, plugin without Node | it compiles, unit tests green |
| 2 | Model | v3 invitations, machine and name certificates, per-channel storage, conversation registry | unit tests of the rules (conflicts, signatures, lateness) |
| 3 | Daemon | single daemon, protocol v3, routing, waiting, purge, presence, `keepalive` | two simulated machines (two `DENDEN_HOME`) exchange messages |
| 4 | CLI | every command and its 🐌 outputs, time-limited invitations and 15 min page, hidden `denden join` in a terminal | the flow of design.md replayed by hand |
| 5 | Wake-ups | Claude listener, Codex queue, hooks, budget, late messages, MCP presence | a wake-up in Claude and in Codex |
| 6 | Plugin | SKILL.md rewritten, page, manifests, binary build script, README | install from the local marketplaces |
| 7 | Validation | end-to-end tests, a test with the user in Claude and Codex | full flow validated |

## 8. Known limits

### Strict NAT

Measured on 19 October 2026 with `tests/e2e/nat.sh`. One machine sat in a Linux network namespace behind an `nft masquerade fully-random` NAT, which gives a different port for each destination. The other machine sat behind an ordinary home router.

| Case | Result |
|---|---|
| Public machine ↔ ordinary NAT | connected in 15 to 17 s |
| Ordinary NAT ↔ ordinary NAT | connected in 16 to 23 s |
| Strict NAT ↔ ordinary NAT | usually connected, in 24 to ~100 s; a few attempts do not get through within the 90 s that `join` waits |
| Strict NAT ↔ strict NAT | not tested; impossible without a relay |

How peeroxide behaves (fork, commit `43d3de4`):
- **Side that opens the connection:** behind a strict NAT, peeroxide opens 256 spare sockets (birthday attack), and the winning probe lands on one of them. Before `43d3de4`, the connection then left from the primary socket, which the NAT maps to another port, and it never got through. It now leaves from the winning socket, as in the Node version.
- **Side that receives, behind a strict NAT: not supported.** libudx attaches a pending connection to the primary socket only, while the packets arrive on a spare socket. A machine behind a strict NAT therefore cannot be reached by anyone: it has to open the connection itself. Since each pair of machines tries both directions at the same time, that is enough most of the time.
- **Two strict NATs:** no hole punch can get through. It would take a blind relay: peeroxide can do it (`peeroxide relay`, `SwarmConfig::relay_through`), but denden does not use it yet. That relay would not see the content (end-to-end encryption), only IP addresses, times and volumes. It has to be hosted on a reachable machine: peeroxide provides no public relay.

Leads, to decide later:
1. in libudx, accept the first packet of a pending connection on any socket of the hole punch, as udx-native does;
2. in denden, a fallback blind relay, hosted by a member of the channel that has a public IP.
