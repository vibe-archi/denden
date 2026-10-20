# peeroxide vs. the Node agent-im daemon: interop report

Date: 2026-10-15. peeroxide 1.7.3 (crates.io, 2026-07-12) against the plugin's Node stack: hyperswarm
4.17.2, hyperdht 6.34.1, Node 24.18.0. Platform: Windows 11, one host on a home LAN behind one NAT.
Test channel: a throwaway `t1@proto` in a scratch `IM_HOME` (never `~/.agent-im`).

## Verdict

**A Rust binary built on peeroxide speaks the agent-im v2 protocol to the existing Node daemon without
any protocol change.** The topic and transport-key derivation, Noise/SecretStream, the proof-first
handshake, hello/admission by invitation, certificates, and messages with acks all work in both
directions.

- **Rust dials Node: works with stock peeroxide 1.7.3.**
- **Node dials Rust: fails with stock 1.7.3.** Bugs in peeroxide's server-side handshake path break
  it: two found live, a third found while drafting the upstream fix.
  - A first local patch of about 40 lines fixed the first two (`vendor/peeroxide`,
    `peeroxide-patch.diff`); with it, both directions work on the public DHT.
  - The upstream draft (`upstream/`, see below) fixes all three properly, with tests.
- **Real NAT-to-NAT hole punching is still untested.** Everything ran on one host. peeroxide's
  holepunch protocol (NAT analysis, PEER_HOLEPUNCH rounds, CONSISTENT punch) completed both ways,
  but the final packets went over LAN addresses.

## Step by step

| Step | Rust dials Node (stock) | Node dials Rust (stock) | Node dials Rust (patched) |
|---|---|---|---|
| Topic / transport key derivation | ✅ same topic `ea1929f0…` for epoch 20741 | ✅ | ✅ |
| DHT bootstrap, announce, lookup | ✅ bootstrap 0.85 s, lookup 0.8 s | ✅ Node finds Rust's announcement and sends PEER_HANDSHAKE | ✅ |
| Hole punching | ✅ LAN shortcut by default; ✅ full active holepunch with `--no-local` (`punch successful`) | ❌ `passive holepunch timed out` on every attempt; Node: `HOLEPUNCH_ABORTED` | ✅ `punches.consistent = 1` |
| Noise IK + SecretStream | ✅ | never reached | ✅ |
| Proof-first HMAC handshake | ✅ both proofs verify | – | ✅ |
| hello / admission | ✅ by invitation (Node logs `(invited)`, invite consumed); ✅ by pin on reconnect | – | ✅ |
| Membership certificate | ✅ received and verified (Ed25519 over `frame('cert', …)`) | – | ✅ |
| Messages | ✅ queued (outbox flush) and live, acks both ways, visible in `im history` | – | ✅ two queued messages plus ours, all acked |

Also checked:
- **Simultaneous start, both peers client and server:** one stable connection for 75 s, no flapping.
- **Same transport key reconnecting:** Node swaps in the new connection (Hyperswarm `ERR_DUPLICATE`
  on the old one).

## Where interop breaks (Node dials Rust)

### Bug 1: duplicate PEER_HANDSHAKE

- **Node client:** sends the same Noise message through up to two relays in parallel and keeps the
  first reply (`hyperdht/lib/connect.js:371-406`, `Semaphore(2)`; the Noise message is built once,
  `if (!c.requesting)`).
- **Node server:** deduplicates by the Noise bytes and answers each copy with the same reply
  (`lib/server.js:503-512`, `_connects` keyed by `hex(noise)`).
- **peeroxide:** runs a fresh Noise responder for every copy (`swarm.rs` `handle_server_handshake`),
  with a new stream id and a new holepunch session. `commit_passive_holepunch` then *replaces* the
  first session ("replacing stale in-flight entry").
- **Result:** the reply Node acted on points at a session peeroxide has already dropped.
- **Fix:** cache the reply by Noise bytes for 60 s and resend it. Fixing only bug 2 still fails, 2
  out of 2 runs (`IM_PROTO_NO_DEDUP=1` switches this fix off).

### Bug 2: wrong `peerAddress` in the holepunch relay list

- **Node:** an entry means `{relayAddress: the DHT node, peerAddress: the server's own address as
  that node sees it}` (`lib/announcer.js:265`). The relay forwards the client's PEER_HOLEPUNCH to
  that `peerAddress` (`lib/router.js:213-218`).
- **peeroxide:** fills it with the *client's* address (`swarm.rs`, `peer_address:
  client_address.clone()`).
- **Result:** the relay bounces the holepunch back to the Node client. Rust never sees a
  PEER_HOLEPUNCH, and Node aborts.
- **Fix:** use our NAT-consensus address (`dht.remote_address()`). That is correct behind a
  consistent NAT. The proper fix is in peeroxide-dht: keep `msg.to` for each relay when committing
  the announce. The upstream draft does exactly that.

### Bug 3: open or direct clients are given a holepunch (found while drafting the upstream fix)

- **Node:** the server dials the client straight away when the client says it is `FIREWALL_OPEN`, or
  reached the server without a relay (`lib/server.js:392`). An open Node client just waits for that
  dial (`lib/connect.js:236`).
- **peeroxide:** stages a passive holepunch whenever it is itself firewalled, whatever the client
  says. Each side waits for the other.
- **Who is affected:** any Node peer with a public address, for example on a VPS, connecting to a
  NATed Rust peer.
- **Fix:** don't holepunch a dialable client; take the existing direct path instead.

### The LAN shortcut does not work against a Rust server (not fixed)

Node↔Node on one LAN skips holepunching. The client pings an address from the server's
`addresses4`, then connects its stream to that address (`connect.js:248-260`).

- **On a LAN:** peeroxide advertises its LAN addresses with the *puncher* socket's port, which does
  not answer DHT pings (`REQUEST_TIMEOUT` seen). The holepunch then wins the race: it works, but
  takes a few seconds more.
- **On one host (loopback):** the first entry is the DHT socket. It answers the ping, so Node
  connects there, but peeroxide's stream listens only on the puncher socket. The result is
  `ETIMEDOUT`.
- **Also exposed:** a same-public-IP pair with no common subnet, behind a NAT that allows hairpin
  connections.
- **Status:** left as an upstream follow-up. The interop test sidesteps it with `--remote-client`.

### Upstream test coverage

peeroxide's own `tests/hyperswarm_interop.rs` has Node as the server and Rust as the client. That is
exactly the direction that works; the Node-client → Rust-server direction had no interop test.

### Upstream draft (`upstream/`)

- **Where:** branch `fix/node-client-to-rust-server` in `upstream/peeroxide`, opened as draft PR
  https://github.com/Rightbracket/peeroxide/pull/31 from the fork `alaa-eddine/peeroxide`.
- **Text:** the issue and PR description are in `upstream/ISSUE_AND_PR.md`.
- **What it does:** fixes bugs 1–3 with additive API only (`HyperDhtHandle::current_relays()`,
  `QueryReply::to`), and adds Node-client → Rust-server interop tests that fail on `main` without the
  fixes.
- **The prototype** now builds against that branch (`[patch.crates-io]` in `Cargo.toml`).

## Measurements (release build: `opt-level = "z"`, LTO, stripped, no tracing)

| | Node (today) | Rust prototype |
|---|---|---|
| UserPromptSubmit hook, 30 cold starts | median **144 ms** (min 133, p90 166); bare `node -e 0` is 91 ms | median **18 ms** (min 16.5, p90 19) |
| Daemon memory, 1 peer connected | WS **77 MB** (peak 79), private 38 MB, 9 threads | WS **9.4 MB** (peak 9.4), private 2.2 MB, 4–6 threads |
| What ships | node.exe 92.5 MB (prerequisite) + node_modules 31.4 MB / 59 packages, `npm ci` on first run | one file: **1.06 MiB** Windows exe; 1.35 MiB Linux (glibc, built in WSL) |
| Cold start → message sent and received | n/a (the CLI talks to a running daemon) | 1.9 s: bootstrap 0.85 s, lookup and connect 0.9 s, protocol 0.1 s |

**Caveats:**
- The prototype has no HTTP API, storage, rate limiting or epoch overlap. A full daemon would add a
  few MB at most.
- The hook timing is for the same 1 MiB binary (the hook needs none of the networking code). About
  10 ms of it is Windows process creation and antivirus scanning.
- The Rust hook gives the same block decision as Node on a payload with an invitation and on one
  without.

## Remaining gaps before committing to peeroxide

1. **Real cross-NAT test.**
   - Run two machines on different networks, for example one on a phone hotspot.
   - Include a RANDOM NAT (mobile or CGNAT), where Node uses birthday-paradox punching.
   - This is the one thing that could still change the verdict.
2. **Upstream the fixes**, or carry the patch.
   - Draft PR https://github.com/Rightbracket/peeroxide/pull/31 is open. Its CI waits for a
     maintainer to approve the workflow run.
   - The LAN-shortcut issue is still open upstream.
3. **Duplicate-connection tie-break parity.**
   - Hyperswarm keeps the *new* connection once the existing one has sent and received bytes
     (`hyperswarm/index.js:360-362`). With proof-first, that happens within milliseconds.
   - peeroxide uses only the key comparison on outgoing connections, and drops incoming duplicates
     outright.
   - The two can disagree after a reconnect. Not observed, but it should be aligned.
4. **API gaps for a full daemon port.**
   - No `joinPeer`/`leavePeer`. The daemon uses them to redial after `im trust` or a new
     certificate; `dht()` is exposed, but `connect` needs a `UdxRuntime` the public API does not
     give.
   - No firewall callback and no `ban()`. `SwarmConfig.firewall` is a NAT hint, not a filter.
     Banned transport keys would have to be dropped after connecting.
5. **Leaving is slow to notice.**
   - Node sees a Rust peer leave only 14–18 s after it exits, with or without `shutdown()`.
     peeroxide's shutdown sends a FIN, and the daemon does not close on `end`.
   - This matters for one-shot CLIs, not for a daemon.
6. **Not tested:** epoch rotation (two swarms overlapping), IPv6, macOS.
7. **Maturity.** peeroxide went from 1.0.0 (April 2026) to 1.7.3 (July 2026). One org (Rightbracket)
   maintains it, and its server path has the gaps above.

## Recommendation

peeroxide is a viable way to keep the current protocol and interoperate with existing Node users.
The client direction already works stock. The server direction needs three small, clear fixes, now
drafted for upstream. iroh is not needed for interop. Before deciding, run gap 1 (cross-NAT), then
post the upstream PR and align the tie-break (gap 3).

## Files

- `src/main.rs`: the prototype (`peer`, `derive`, `hook-prompt`).
- `upstream/peeroxide` (branch `fix/node-client-to-rust-server`) and `upstream/ISSUE_AND_PR.md`:
  the upstream fixes and the text for the issue and PR. The prototype builds against this.
- `vendor/peeroxide/` and `peeroxide-patch.diff`: the first local patch, kept for the record.
  Remove `[patch.crates-io]` from `Cargo.toml` to build against stock 1.7.3.
- `target/im-proto-stock-debug.exe`: the stock build used for the failing runs.
- `tools/hdcheck.mjs`: dials a Rust server with Node hyperdht and prints the decoded reply and the
  LAN pings.
- `tools/get-invite.mjs`, `tools/bench-hook.ps1`, `tools/bench-mem.ps1`: setup and measurements.

## Follow-up: a full Rust `im`

The prototype grew into a complete port of im.mjs, in `../rust-im/` (CLI, daemon, pages, hooks).
It was validated locally against Node peers and Node-made identities. See
[rust-im/E2E-REPORT.md](../rust-im/E2E-REPORT.md).
