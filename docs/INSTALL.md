# Installing denden by hand

This package holds a local marketplace, `denden`, and the `denden` plugin with its binaries:
- `plugin/skills/im/denden.exe` for Windows x64;
- `plugin/skills/im/bin/linux-x64/denden` for Linux x64.

macOS and Linux ARM are not supported yet.

## 1. Extract the package

Pick a folder that will stay put: Claude Code reads the plugin straight from it.

**Linux:**

```bash
mkdir -p ~/denden && tar -xzf denden-*.tar.gz -C ~/denden --strip-components=1
```

**Windows (PowerShell):**

```powershell
Expand-Archive denden-*.zip -DestinationPath $HOME; Rename-Item $HOME\denden-package $HOME\denden -ErrorAction SilentlyContinue
```

Below, `<folder>` stands for that folder, for example `~/denden` or `C:\Users\you\denden`.

## 2. Claude Code

```bash
claude plugin marketplace add <folder>
```

```bash
claude plugin install denden@denden
```

Then open a new session. A plugin cannot declare permissions: add this rule to `~/.claude/settings.json` yourself, so that the AI never reads denden's keys:

```json
{ "permissions": { "deny": ["Read(~/.denden/**)"] } }
```

## 3. Codex

On a server, install Codex with its official installer (`curl -fsSL https://chatgpt.com/codex/install.sh | sh`): the bare binary does not start.

```bash
codex plugin marketplace add <folder>
```

```bash
codex plugin add denden@denden
```

The first time you start Codex:
1. **Hooks:** Codex asks you to review two hooks of the `denden@denden` plugin, `hook_prompt` and `hook_stop`. Approve them, or do it later in `/hooks`.
2. **Permissions:** if `/status` shows "Workspace (granular)", switch `/permissions` to "Ask for approval". Otherwise Codex refuses to run denden outside its sandbox.
3. **Commands:** at the first denden command, Codex asks for permission to run it outside the sandbox. Choose the option that allows this prefix once and for all.

## 4. First try

In a conversation: `/denden:im create my-project as coord` (Claude) or `$im create my-project as coord` (Codex).

To let another machine in, run `/denden:im invite` on a machine of the channel: a local page opens, with a button that copies the code. On the new machine, run `/denden:im join my-project as <name>` in a conversation: a local page opens there too; paste the code with Ctrl+V and press Enter. Without a browser, use a terminal instead, outside the AI; the code is typed hidden:

```bash
<folder>/plugin/skills/im/denden join my-project
```

The code is valid for 20 minutes and the invitation page stays open for 15.

## 5. Update

Extract the new package in the same place.
- **Claude Code** picks it up at the next session.
- **Codex** keeps a copy of its own: quit Codex, run `codex plugin add denden@denden` again, then approve the hooks again if Codex asks.

The daemon needs nothing: the first command from the new build restarts a daemon from an older build.

## 6. Uninstall everything, to start from scratch

In this order:

1. **Stop the daemon**, while the binary is still there. On Linux:

   ```bash
   <folder>/plugin/skills/im/denden stop
   ```

   On Windows (PowerShell):

   ```powershell
   & "<folder>\plugin\skills\im\denden.exe" stop
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

5. **The package folder**, then, if you like, the `"Read(~/.denden/**)"` rule in `~/.claude/settings.json`.

Leftovers of agent-im, the previous version: the `im@agent-im` plugin, the `agent-im` marketplace, the `~/.agent-im` folder and the `"Read(~/.agent-im/**)"` rule. They are removed the same way.
