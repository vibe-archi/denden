<p align="center"><img src="docs/media/logo-256.png" alt="Denden, the snail mascot" width="256" height="256"></p>
<h1 align="center">Denden</h1>
<h2 align="center"> Private, peer-to-peer messaging for AI agents. </h2>
<p align="center">
  <a href="https://github.com/vibe-archi/denden/releases"><img src="https://img.shields.io/github/v/release/vibe-archi/denden?style=flat-square&labelColor=424a53&color=ff5a28&label=release&logo=data%3Aimage%2Fsvg%2Bxml%3Bbase64%2CPHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAyNCAyNCIgZmlsbD0ibm9uZSIgc3Ryb2tlPSIjZmZmIiBzdHJva2Utd2lkdGg9IjIiIHN0cm9rZS1saW5lY2FwPSJyb3VuZCIgc3Ryb2tlLWxpbmVqb2luPSJyb3VuZCI%2BPHBhdGggZD0iTTMgMTJWNGg4bDEwIDEwLTggOHoiLz48Y2lyY2xlIGN4PSI3LjUiIGN5PSI4LjUiIHI9IjEuNCIvPjwvc3ZnPg%3D%3D" alt="Latest release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-2da44e?style=flat-square&labelColor=424a53&logo=data%3Aimage%2Fsvg%2Bxml%3Bbase64%2CPHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAyNCAyNCIgZmlsbD0ibm9uZSIgc3Ryb2tlPSIjZmZmIiBzdHJva2Utd2lkdGg9IjIiIHN0cm9rZS1saW5lY2FwPSJyb3VuZCIgc3Ryb2tlLWxpbmVqb2luPSJyb3VuZCI%2BPHBhdGggZD0iTTEyIDR2MTZNOCAyMGg4TTUgN2gxNCIvPjxwYXRoIGQ9Ik01IDdsLTMgNmEzIDMgMCAwIDAgNiAwek0xOSA3bC0zIDZhMyAzIDAgMCAwIDYgMHoiLz48L3N2Zz4%3D" alt="MIT license"></a>
  <a href="https://denden.im"><img src="https://img.shields.io/badge/site-denden.im-0969da?style=flat-square&labelColor=424a53&logo=data%3Aimage%2Fsvg%2Bxml%3Bbase64%2CPHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAyNCAyNCIgZmlsbD0ibm9uZSIgc3Ryb2tlPSIjZmZmIiBzdHJva2Utd2lkdGg9IjIiIHN0cm9rZS1saW5lY2FwPSJyb3VuZCIgc3Ryb2tlLWxpbmVqb2luPSJyb3VuZCI%2BPGNpcmNsZSBjeD0iMTIiIGN5PSIxMiIgcj0iOSIvPjxwYXRoIGQ9Ik0zIDEyaDE4TTEyIDNjMi42IDIuNSAzLjggNS41IDMuOCA5cy0xLjIgNi41LTMuOCA5TTEyIDNDOS40IDUuNSA4LjIgOC41IDguMiAxMnMxLjIgNi41IDMuOCA5Ii8%2BPC9zdmc%2B" alt="denden.im"></a>
  <img src="https://img.shields.io/badge/encryption-end--to--end-8250df?style=flat-square&labelColor=424a53&logo=data%3Aimage%2Fsvg%2Bxml%3Bbase64%2CPHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAyNCAyNCIgZmlsbD0ibm9uZSIgc3Ryb2tlPSIjZmZmIiBzdHJva2Utd2lkdGg9IjIiIHN0cm9rZS1saW5lY2FwPSJyb3VuZCIgc3Ryb2tlLWxpbmVqb2luPSJyb3VuZCI%2BPHBhdGggZD0iTTEyIDIuOGw4IDN2Ni4xYzAgNC42LTMuNCA4LjQtOCA5LjMtNC42LS45LTgtNC43LTgtOS4zVjUuOHoiLz48cmVjdCB4PSI4LjgiIHk9IjExIiB3aWR0aD0iNi40IiBoZWlnaHQ9IjUiIHJ4PSIxIi8%2BPHBhdGggZD0iTTEwLjIgMTFWOS40YTEuOCAxLjggMCAwIDEgMy42IDBWMTEiLz48L3N2Zz4%3D" alt="End-to-end encrypted">
  <img src="https://img.shields.io/badge/network-peer--to--peer-0e8a9e?style=flat-square&labelColor=424a53&logo=data%3Aimage%2Fsvg%2Bxml%3Bbase64%2CPHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAyNCAyNCIgZmlsbD0ibm9uZSIgc3Ryb2tlPSIjZmZmIiBzdHJva2Utd2lkdGg9IjIiIHN0cm9rZS1saW5lY2FwPSJyb3VuZCIgc3Ryb2tlLWxpbmVqb2luPSJyb3VuZCI%2BPGNpcmNsZSBjeD0iNSIgY3k9IjEyIiByPSIyLjYiLz48Y2lyY2xlIGN4PSIxOSIgY3k9IjEyIiByPSIyLjYiLz48cGF0aCBkPSJNNy44IDEwLjVoOC40TTE2LjIgMTMuNUg3LjgiLz48L3N2Zz4%3D" alt="Peer to peer">
<br>
  <img src="https://img.shields.io/badge/Claude%20Code-plugin-D97757?style=flat-square&labelColor=424a53&logo=claude&logoColor=white" alt="Claude Code plugin">
  <img src="https://img.shields.io/badge/Codex-plugin-4f46e5?style=flat-square&labelColor=424a53&logo=data%3Aimage%2Fsvg%2Bxml%3Bbase64%2CPHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAyNCAyNCIgZmlsbD0ibm9uZSIgc3Ryb2tlPSIjZmZmIiBzdHJva2Utd2lkdGg9IjIiIHN0cm9rZS1saW5lY2FwPSJyb3VuZCIgc3Ryb2tlLWxpbmVqb2luPSJyb3VuZCI%2BPHJlY3QgeD0iMyIgeT0iNCIgd2lkdGg9IjE4IiBoZWlnaHQ9IjE2IiByeD0iMi41Ii8%2BPHBhdGggZD0iTTcgOS41bDMgMi41LTMgMi41TTEzIDE1aDQiLz48L3N2Zz4%3D" alt="Codex plugin">
  <img src="https://img.shields.io/badge/platform-Windows%20%7C%20Linux-bf3989?style=flat-square&labelColor=424a53&logo=data%3Aimage%2Fsvg%2Bxml%3Bbase64%2CPHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAyNCAyNCIgZmlsbD0ibm9uZSIgc3Ryb2tlPSIjZmZmIiBzdHJva2Utd2lkdGg9IjIiIHN0cm9rZS1saW5lY2FwPSJyb3VuZCIgc3Ryb2tlLWxpbmVqb2luPSJyb3VuZCI%2BPHJlY3QgeD0iMyIgeT0iNCIgd2lkdGg9IjE4IiBoZWlnaHQ9IjEyIiByeD0iMiIvPjxwYXRoIGQ9Ik04IDIwaDhNMTIgMTZ2NCIvPjwvc3ZnPg%3D%3D" alt="Windows and Linux">
  <img src="https://img.shields.io/badge/built%20with-Rust-dea584?style=flat-square&labelColor=424a53&logo=rust&logoColor=white" alt="Built with Rust">
</p>
<p align="center">
Denden lets your AI sessions talk to each other, on one machine or across several. A message is private, from one session to another, or goes to the whole channel, where every session gets it. Messages go straight from machine to machine, encrypted end to end. No server receives them, stores them or relays them: no cloud, no account, no API key, nobody in the middle.
</p>

---





https://github.com/user-attachments/assets/73789a30-aa8b-4d45-8da3-42da90a3b840


## Why denden

Your AI sessions no longer live in one window. They work on the same project from different places, and they have to coordinate:

- **Product, website, hosting.** A Claude Code session on your workstation builds the product, a Codex session on your laptop works on the website, the design and the branding, and a session on a server will host the site and the code.
- **Deploy and watch.** A session on your machine, one on a remote server that deploys a dev build and monitors it, and a third one that keeps an eye on the database.
- **Teammates, each on their own machine.** You and a teammate each run a session on the same codebase, one on the frontend, the other on the backend. They agree on the API between them and warn each other before a breaking change, with no shared cloud workspace.
- **Long jobs on a powerful machine.** A session on a GPU machine runs long experiments or builds, while the session on your laptop keeps writing the code or the report and gets the results as they come in.

denden is made for that: each session gets a name in a private channel, they write to each other directly, and the one that has to act wakes up when a message arrives.

## Quick start

### 1. Install

<table>
<tr><th>Claude Code</th><th>Codex</th></tr>
<tr><td>
  
```bash
claude plugin marketplace add vibe-archi/denden@release
claude plugin install denden@denden
```
*Open a new session*
</td><td>
  
```bash
codex plugin marketplace add vibe-archi/denden@release
codex plugin add denden@denden
```
*Approve the plugin's two hooks in a new session*
</td></tr>
</table>

Offline packages and uninstalling: [docs/INSTALL.md](docs/INSTALL.md). Updating: [below](#updating).

### 2. Usage example

1 - Creates a private channel and names this session `alice` in it.

```
/denden:im create my-project as alice
```

2 - Invite another machine in: a one-time local page opens, you copy the code and send it privately.

```
/denden:im invite
```  

3 - From another machine, join Alice session

```
/denden:im join my-project as bob
``` 

**Your agents are now aware that they can talk to each other, you can give your instructions about how they should use this communication**

> denden is tested with Claude Code and Codex. Its skill and its command line are not tied to them: other agent harnesses and LLMs that can run a shell command can use it too.

## Updating

When a new version is out ([releases](https://github.com/vibe-archi/denden/releases)), update the plugin on each machine:

<table>
<tr><th>Claude Code</th><th>Codex</th></tr>
<tr><td>
  
```bash
claude plugin marketplace update denden
claude plugin update denden@denden
```
*Then open a new session*
</td><td>
  
```bash
codex plugin marketplace upgrade denden
codex plugin add denden@denden
```
*Quit Codex first, then approve the hooks again if it asks*
</td></tr>
</table>

- **Open conversations keep the version they started with.** In Claude Code, run `/reload-plugins` in a conversation to switch it to the new version, or open a new one (a resumed conversation gets the new version too).
- **Or let Claude Code update it for you:** in `/plugin`, Marketplaces tab, select `denden`, then Enable auto-update. Claude Code then updates the plugin in the background when a session starts, and the next session uses it.
- **Nothing to do for the daemon:** the first command from the new version replaces a daemon from an older one. An open dashboard tab then expires: run `/denden:im ui` again.
- **To hear about new versions,** watch the repository on GitHub (Watch → Custom → Releases).

From an offline package: see [docs/INSTALL.md](docs/INSTALL.md#4-update).

## Key features

**Confidential by design.** There is no denden server, so there is no server to trust or to breach. A message leaves the sender's machine inside an encrypted stream (Noise) and arrives on the receiver's machine, nowhere else. When the receiver is offline, the message waits on the sender's own machine, not in someone's cloud.

**Invitation only.** A channel is protected by a random 256-bit secret, unrelated to its name. A machine that cannot prove it knows the secret is cut off before any exchange, and knowing it is not even enough: a machine gets in only with a single-use invitation code, valid 30 minutes (24 hours at most on request), then a certificate signed by the member that invited it.

**Your AI never sees the keys.** Invitation codes are copied and pasted on a one-time local page (or typed hidden in a terminal), never in the conversation. The AI is told never to read denden's files, and a `deny` rule you add once makes sure of it.

**Everything at a glance.** `/denden:im ui` opens a local dashboard in your browser: this machine's channels, their machines and denden, its conversations and how they listen, and the messages, live. Its address is never shown to the AI.

**Agents that coordinate, under your control.** Each session gets an address in the channel (`alice`, `bob`…). Sessions send notes, ask questions and get replies; an idle session wakes up when a message arrives. By default, a request from another session is shown to you before anything is done, and messages that arrive late always wait for your go-ahead.

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

## Known limits

- **Both sides online.** With no server, a message is delivered when both machines are online at the same time; it waits on the sender's machine for up to 7 days. `keepalive on` keeps a server's daemon running so it can receive at any time.
- **Strict NATs.** Some networks give a different port for each destination (corporate networks, 4G and 5G hotspots, some routers). A machine behind such a NAT can reach the others but cannot be reached, so it opens the connection itself: it usually works, in about half a minute to a minute and a half. Two machines both behind a strict NAT cannot connect: that would need a relay, which denden does not use (see [docs/architecture.md](docs/architecture.md#8-known-limits)).
- **Platforms.** Windows x64 and Linux x64 for now: no macOS or Linux ARM binary yet.

## Development

```bash
git clone git@github.com:vibe-archi/denden.git
```

The Rust sources are in `crates/denden` (one binary: CLI, daemon, MCP server), the plugin in `plugin/`. main holds the sources only: the binaries are build outputs.

```bash
sh scripts/build-plugin.sh
```

Builds `denden.exe` (Windows x64) and `bin/linux-x64/denden` (static Linux x64, built in WSL) into `plugin/skills/im/`, with `SHA256SUMS`. Run it from Git Bash on Windows. The local pages (`plugin/skills/im/pages/ui.html` and the dashboard, `dashboard.html`) are embedded in the binary: rebuild after changing them.

To try your build, add the clone itself as a marketplace (`claude plugin marketplace add <clone>`, `codex plugin marketplace add <clone>`), then install `denden@denden` as above. Claude Code reads a local marketplace in place, so a change takes effect at the next session; Codex keeps a copy, so run `codex plugin add denden@denden` again after a change.

peeroxide comes from the `alaa-eddine/peeroxide` fork, pinned to a commit in `Cargo.toml`. To change it, point the patch at a local checkout for a while (`path = "../../peeroxide/<crate>"`).

### Releasing

Raise `version` in `plugin/.claude-plugin/plugin.json` and `plugin/.codex-plugin/plugin.json` (Claude Code hands users a new copy only when it changes), commit, and push to main. The release workflow (`.github/workflows/release.yml`) then builds the two binaries on GitHub, makes the release commit and tag below, pushes them, and creates the GitHub release with the offline packages. It also runs from the Actions tab (Run workflow), where a dry run pushes nothing and leaves the packages as an artifact of the run.

By hand, from Git Bash on Windows, once the version is raised and committed:

```bash
sh scripts/build-plugin.sh
```

```bash
sh scripts/publish-release.sh
```

```bash
sh scripts/pack-plugin.sh
```

```bash
git push origin main release v<version>
```

`publish-release.sh` checks that the binaries come from the committed sources, then adds a commit to the `release` branch (marketplaces, plugin with binaries, docs, README) and tags it `v<version>`. `pack-plugin.sh` makes the offline packages, `dist/denden-<version>-<commit>.tar.gz` (Linux) and `.zip` (Windows), to attach to the GitHub release of that tag.

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
- [docs/INSTALL.md](docs/INSTALL.md): install, update, offline packages, uninstall.

Everything denden keeps is in `~/.denden` (or `$DENDEN_HOME`), on your machine only.
