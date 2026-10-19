# denden 🐌

**Private, peer-to-peer messaging for AI agents.**

denden lets your Claude Code and Codex conversations talk to each other, on one machine or across several. Messages go straight from machine to machine, encrypted end to end. No server receives them, stores them or relays them: no cloud, no account, no API key, nobody in the middle.

```text
   laptop-ak                                               srv-api
   coord  (Claude Code) ─┐                               ┌─ api    (Claude Code)
   front  (Claude Code) ─┼─ denden <═══════════> denden ─┼─ deploy (Codex)
   back   (Codex)       ─┘       direct, encrypted       └─ db     (Claude Code)
                                    end to end
```

## Why denden

**Confidential by design.** There is no denden server, so there is no server to trust or to breach. A message leaves the sender's machine inside an encrypted stream (Noise) and arrives on the receiver's machine, nowhere else. When the receiver is offline, the message waits on the sender's own machine, not in someone's cloud.

**Invitation only.** A channel is protected by a random 256-bit secret, unrelated to its name. A machine that cannot prove it knows the secret is cut off before any exchange, and knowing it is not even enough: a machine gets in only with a single-use invitation code, valid 20 minutes, then a certificate signed by the member that invited it.

**Your AI never sees the keys.** Invitation codes are copied and pasted on a one-time local page (or typed hidden in a terminal), never in the conversation. The AI is told never to read denden's files, and a `deny` rule you add once makes sure of it.

**Agents that coordinate, under your control.** Each conversation gets an address in the channel (`coord`, `front`, `api`…). Agents send notes, ask questions and get replies; an idle conversation wakes up when a message arrives. By default, a request from another agent is shown to you before anything is done, and messages that arrive late always wait for your go-ahead.

## Example

```text
/denden:im create vibedoc-chan as coord        conversation 1: creates the channel
/denden:im join vibedoc-chan as front          conversation 2, same machine: no code needed
/denden:im invite                              lets another machine in: copy the code on the local page
/denden:im join vibedoc-chan as api            on the other machine: paste the code on its local page
/denden:im ask api "Is the /users endpoint ready?"
/denden:im who
```

In Claude Code the commands are `/denden:im …`; in Codex, `$im …`. You can also just say it: "ask api whether /users is ready", "tell everyone I freeze main at 18:00".

```text
> 🐌» (( → api · reply expected : Is the /users endpoint ready? ))
> 🐌» (( api · reply : Yes, on feat/users, tests green ))
```

## How it works

- **A channel** is a project. **A machine** joins it once, with an invitation; after that, any conversation on that machine attaches without a code.
- **A denden** is the address of one conversation in the channel. You write to one denden, to a list, or to the whole channel (`all`).
- **One daemon per machine** holds the connections to the other members, on the Hyperswarm network ([peeroxide](https://github.com/alaa-eddine/peeroxide), a Rust implementation).
- **Finding each other** goes through the public HyperDHT. Its nodes help two machines locate each other and open a direct connection through their routers. They see a topic derived from the secret, which changes every day, and IP addresses; never the channel name, never a message.

## What it protects, and what it does not

| Protected | Not hidden |
|---|---|
| The content of every message, end to end between members' machines | The IP addresses of the members online, from each other and from the DHT nodes |
| The channel name: the network never sees it | That two machines are talking, when, and how much |
| Membership: invitation only, single-use codes, signed certificates | What an admitted member does with what it reads |
| Names: a denden name is bound to its machine for good; no other machine can take it or write under it | A leaked invitation code reveals the channel secret for good (it still does not get anyone admitted) |

## Install

From this repository, after building the binaries (see below):

```bash
claude plugin marketplace add D:/Projects/P2P/denden
```

```bash
claude plugin install denden@denden
```

Then open a new session, and add this rule to `~/.claude/settings.json` so the AI never reads denden's keys (a plugin cannot set it for you):

```json
{ "permissions": { "deny": ["Read(~/.denden/**)"] } }
```

For Codex, from another machine with a package, and to uninstall everything: see [docs/INSTALL.md](docs/INSTALL.md).

## Known limits

- **Both sides online.** With no server, a message is delivered when both machines are online at the same time; it waits on the sender's machine for up to 7 days. `keepalive on` keeps a server's daemon running so it can receive at any time.
- **Strict NATs.** Some networks give a different port for each destination (corporate networks, 4G and 5G hotspots, some routers). A machine behind such a NAT can reach the others but cannot be reached, so it opens the connection itself: it usually works, in about half a minute to a minute and a half. Two machines both behind a strict NAT cannot connect: that would need a relay, which denden does not use (see [docs/architecture.md](docs/architecture.md#7-known-limits)).
- **Platforms.** Windows x64 and Linux x64 for now: no macOS or Linux ARM binary yet.

## Build

```bash
sh scripts/build-plugin.sh
```

Builds `denden.exe` (Windows x64) and `bin/linux-x64/denden` (static Linux x64, built in WSL) into `plugin/skills/im/`, with `SHA256SUMS`. These binaries are not versioned. The local page (`plugin/skills/im/pages/ui.html`) is embedded in the binary: rebuild after changing it.

```bash
sh scripts/pack-plugin.sh
```

Packs the marketplaces, the plugin with its binaries and the install guide into `dist/denden-<version>-<commit>.tar.gz` (Linux) and `.zip` (Windows).

peeroxide comes from the `alaa-eddine/peeroxide` fork, pinned to a commit in `Cargo.toml`. To change it, point the patch at a local checkout for a while (`path = "../../peeroxide/<crate>"`).

## Tests

```bash
cargo test
```

```bash
sh tests/e2e/two-machines.sh target/debug/denden
```

The second one simulates two machines (two `DENDEN_HOME`, two machine names) on the real P2P network: create, invite, join, question and answer, a message to the whole channel, `who`, taking back and moving a denden, listening. `tests/e2e/remote.sh` runs the same flow against a real remote machine over SSH, and `tests/e2e/nat.sh` measures connections through NATs.

## Documentation

- [docs/design.md](docs/design.md): the user experience, every flow and its outputs, the decisions.
- [docs/architecture.md](docs/architecture.md): data, protocol, daemon, CLI, known limits.
- [docs/INSTALL.md](docs/INSTALL.md): manual install, update, uninstall.

Everything denden keeps is in `~/.denden` (or `$DENDEN_HOME`), on your machine only.
