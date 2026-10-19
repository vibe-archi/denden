# denden 🐌

Messages between your AI conversations (Claude Code, Codex), on this machine and on others, peer to peer: no server, end-to-end encrypted, in invitation-only channels. You use it from a conversation with `/denden:im …` (Claude Code) or `$im …` (Codex).

- **A channel** is a project. **A machine** joins it once, with an invitation.
- **Each conversation** has its **denden**, its address in the channel (`coord`, `front`, `api`…), and attaches to the channel without a code.
- You write to one denden, to several, or to the whole channel.

The design is described in [docs/design.md](docs/design.md), the architecture in [docs/architecture.md](docs/architecture.md), the manual install in [docs/INSTALL.md](docs/INSTALL.md).

## Example

```text
/denden:im create vibedoc-chan as coord        conversation 1: creates the channel
/denden:im join vibedoc-chan as front          conversation 2, same machine: no code needed
/denden:im invite                              to let another machine in
/denden:im join vibedoc-chan as api            on the other machine: paste the code on the local page
/denden:im ask api "Is the /users endpoint ready?"
/denden:im who
```

## Install

This repository is a local marketplace, `denden`, that holds the `denden` plugin. Build the plugin binaries first (see below). To install on another machine without the repository, see [docs/INSTALL.md](docs/INSTALL.md) and `scripts/pack-plugin.sh`.

### Claude Code

```bash
claude plugin marketplace add D:/Projects/P2P/denden
```

```bash
claude plugin install denden@denden
```

Then open a new session. Claude Code loads a plugin from a local marketplace straight from the repository: a change takes effect at the next session.

A plugin cannot declare permissions. Add this rule to `~/.claude/settings.json` yourself, so that the AI never reads denden's keys:

```json
{ "permissions": { "deny": ["Read(~/.denden/**)"] } }
```

### Codex

```bash
codex plugin marketplace add D:/Projects/P2P/denden
```

```bash
codex plugin add denden@denden
```

Codex copies the plugin into its cache: after a change, quit Codex and run `codex plugin add denden@denden` again. Installing a plugin does not approve its hooks: open `/hooks` in Codex and approve the two hooks of the `denden` plugin.

denden needs the network and `~/.denden`, which the Codex sandbox blocks, so Codex asks to run its commands outside the sandbox. Approve the denden command prefix once and for all. If Codex answers that the session rejects these requests, switch `/permissions` to "Ask for approval".

On a server, install Codex with its official installer (`https://chatgpt.com/codex/install.sh`): the bare binary, without its package, does not start, and its app server is what lets denden wake a conversation.

## Build

```bash
sh scripts/build-plugin.sh
```

The script builds `denden.exe` (Windows x64) and `bin/linux-x64/denden` (static Linux x64, built in WSL), puts them in `plugin/skills/im/` and writes `SHA256SUMS`. These binaries are not versioned. The local page (`plugin/skills/im/pages/ui.html`) is embedded in the binary: rebuild after changing it.

```bash
sh scripts/pack-plugin.sh
```

This one packs the marketplaces, the plugin with its binaries and the install guide into `dist/denden-<version>-<commit>.tar.gz` (Linux) and `.zip` (Windows).

peeroxide comes from the `alaa-eddine/peeroxide` fork, pinned to a commit in `Cargo.toml`. To change it, point the patch at a local checkout for a while (`path = "../../peeroxide/<crate>"`).

## Tests

```bash
cargo test
```

```bash
sh tests/e2e/two-machines.sh target/debug/denden
```

The second one simulates two machines (two `DENDEN_HOME`, two machine names) on the real P2P network: create, invite, join, question and answer, a message to the whole channel, `who`, taking back and moving a denden, listening.

`tests/e2e/remote.sh` runs the same flow against a real remote machine over SSH, and `tests/e2e/nat.sh` measures connections through NATs (see [docs/architecture.md](docs/architecture.md#7-known-limits)).

## Known limits

- **Strict NAT.** Some NATs give a different port for each destination: corporate networks, 4G and 5G hotspots, some home routers. A machine behind such a NAT can reach the others, but the others cannot reach it, so it is the one that has to open the connection. Since both machines try at the same time, it usually works, in about half a minute to a minute and a half. If both machines are behind a strict NAT, they cannot reach each other: that would need a relay, which is not supported yet (see [docs/architecture.md](docs/architecture.md#7-known-limits)).
- **Platforms.** Windows x64 and Linux x64 only for now: no macOS or Linux ARM binary.
- **Offline.** A message is delivered only when both machines are online at the same time. It waits on the sender's machine for up to 7 days. `keepalive on` keeps a server's daemon running so it can receive at any time.

## Data

Everything is in `~/.denden` (or `$DENDEN_HOME`): one folder per channel (secret, machine key, known machines, names, messages, outbox), the attached conversations, the daemon. One daemon per machine. It stays up while a conversation is attached or an invitation is waiting to be used; otherwise it stops after 10 minutes with nothing to do, unless `keepalive on` is set.
