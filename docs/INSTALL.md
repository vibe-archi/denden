# Installing denden

Two ways to install, for Claude Code, Codex, or both:
- **From GitHub** (recommended): the `release` branch of [vibe-archi/denden](https://github.com/vibe-archi/denden) carries the plugin with its binaries, and updates come through the marketplace.
- **From a release package**, a `.tar.gz` or `.zip` from the [GitHub releases](https://github.com/vibe-archi/denden/releases): for a machine without access to GitHub, or to keep a fixed version.

Binaries ship for Windows x64 and Linux x64. macOS and Linux ARM are not supported yet.

## 1. From GitHub

### Claude Code

```bash
claude plugin marketplace add vibe-archi/denden@release
```

```bash
claude plugin install denden@denden
```

Then open a new session. A plugin cannot declare permissions: add this rule to `~/.claude/settings.json` yourself, so that the AI never reads denden's keys:

```json
{ "permissions": { "deny": ["Read(~/.denden/**)"] } }
```

### Codex

On a server, install Codex with its official installer (`curl -fsSL https://chatgpt.com/codex/install.sh | sh`): the bare binary does not start.

```bash
codex plugin marketplace add vibe-archi/denden@release
```

```bash
codex plugin add denden@denden
```

The first time you start Codex:
1. **Hooks:** Codex asks you to review two hooks of the `denden@denden` plugin, `hook_prompt` and `hook_stop`. Approve them, or do it later in `/hooks`.
2. **Permissions:** if `/status` shows "Workspace (granular)", switch `/permissions` to "Ask for approval". Otherwise Codex refuses to run denden outside its sandbox.
3. **Commands:** at the first denden command, Codex asks for permission to run it outside the sandbox (denden needs the network). Choose the option that allows this prefix once and for all.

### Over SSH

`vibe-archi/denden` is fetched over HTTPS. To go through your SSH key instead (a private copy of the repository, for example):

```bash
claude plugin marketplace add git@github.com:vibe-archi/denden.git#release
```

```bash
codex plugin marketplace add git@github.com:vibe-archi/denden.git --ref release
```

## 2. From a release package

Download `denden-<version>-<commit>.tar.gz` (Linux) or `.zip` (Windows) and its `.sha256` from the [releases](https://github.com/vibe-archi/denden/releases), check it (`sha256sum -c denden-<version>-<commit>.sha256 --ignore-missing`), then extract it into a folder that will stay put: Claude Code reads the plugin straight from it.

**Linux:**

```bash
mkdir -p ~/denden && tar -xzf denden-*.tar.gz -C ~/denden --strip-components=1
```

**Windows (PowerShell):**

```powershell
Expand-Archive denden-*.zip -DestinationPath $HOME; Rename-Item $HOME\denden-package $HOME\denden -ErrorAction SilentlyContinue
```

Then follow section 1 with that folder in place of `vibe-archi/denden@release`, for example `claude plugin marketplace add ~/denden` or `codex plugin marketplace add C:\Users\you\denden`.

## 3. First try

In a conversation: `/denden:im create my-project as coord` (Claude) or `$im create my-project as coord` (Codex).

To let another machine in, run `/denden:im invite` on a machine of the channel: a local page opens, with a button that copies the code. On the new machine, run `/denden:im join my-project as <name>` in a conversation: a local page opens there too; paste the code with Ctrl+V and press Enter. On a server without a desktop, there is no page: the conversation gives you a `denden join my-project` command to run in a terminal, outside the AI, where the code is typed hidden.

The code is valid for 20 minutes and the invitation page stays open for 15.

## 4. Update

**From GitHub**, Claude Code:

```bash
claude plugin marketplace update denden
```

```bash
claude plugin update denden@denden
```

Then open a new session.

**From GitHub**, Codex (quit Codex first):

```bash
codex plugin marketplace upgrade denden
```

```bash
codex plugin add denden@denden
```

Approve the hooks again if Codex asks.

**From a package:** extract the new package in the same place. Claude Code picks it up at the next session; for Codex, quit it and run `codex plugin add denden@denden` again.

The daemon needs nothing: the first command from the new build restarts a daemon from an older build.

## 5. Uninstall everything, to start from scratch

In this order:

1. **Stop the daemon.** It runs from a copy under `~/.denden/bin`, so it outlives the plugin, and on Windows that running copy cannot be deleted. On Linux:

   ```bash
   pkill -f "$HOME/.denden/bin/"
   ```

   On Windows (PowerShell):

   ```powershell
   Get-Process | Where-Object Path -like "$HOME\.denden\bin\*" | Stop-Process
   ```

2. **Claude Code:**

   ```bash
   claude plugin uninstall denden@denden
   ```

   ```bash
   claude plugin marketplace remove denden
   ```

3. **Codex** (quit Codex first):

   ```bash
   codex plugin remove denden@denden
   ```

   ```bash
   codex plugin marketplace remove denden
   ```

   Two leftovers to delete by hand:
   - in `~/.codex/config.toml`, the `[hooks.state."denden@denden:…"]` sections (two lines each). Otherwise a reinstall finds the hooks already approved;
   - the `~/.codex/plugins/cache/denden` folder.

4. **The data**: channels, machine keys, history, attached conversations. **This cannot be undone**: the machine will have to be invited again into each channel. On Linux:

   ```bash
   rm -rf ~/.denden
   ```

   On Windows (PowerShell):

   ```powershell
   Remove-Item -Recurse -Force "$HOME\.denden"
   ```

5. **The package folder**, if you installed from a package, then, if you like, the `"Read(~/.denden/**)"` rule in `~/.claude/settings.json`.

Leftovers of agent-im, the previous version: the `im@agent-im` plugin, the `agent-im` marketplace, the `~/.agent-im` folder and the `"Read(~/.agent-im/**)"` rule. They are removed the same way.
