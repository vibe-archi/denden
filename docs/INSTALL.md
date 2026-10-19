# Installer denden à la main

Ce paquet contient une place de marché locale, `denden`, et le plugin `denden` avec ses binaires :
- `plugin/skills/im/denden.exe` pour Windows x64 ;
- `plugin/skills/im/bin/linux-x64/denden` pour Linux x64.

macOS et Linux ARM ne sont pas encore pris en charge.

## 1. Extraire le paquet

Choisissez un dossier stable : Claude Code lit le plugin directement depuis ce dossier.

**Linux :**

```bash
mkdir -p ~/denden && tar -xzf denden-*.tar.gz -C ~/denden --strip-components=1
```

**Windows (PowerShell) :**

```powershell
Expand-Archive denden-*.zip -DestinationPath $HOME; Rename-Item $HOME\denden-package $HOME\denden -ErrorAction SilentlyContinue
```

Dans la suite, `<dossier>` désigne ce dossier, par exemple `~/denden` ou `C:\Users\vous\denden`.

## 2. Claude Code

```bash
claude plugin marketplace add <dossier>
```

```bash
claude plugin install denden@denden
```

Ouvrez ensuite une nouvelle session. Un plugin ne peut pas déclarer de permissions : ajoutez vous-même cette règle à `~/.claude/settings.json`, pour que l'IA ne lise jamais les clés de denden :

```json
{ "permissions": { "deny": ["Read(~/.denden/**)"] } }
```

## 3. Codex

Sur un serveur, installez Codex avec son installateur officiel (`curl -fsSL https://chatgpt.com/codex/install.sh | sh`) : le binaire seul ne démarre pas.

```bash
codex plugin marketplace add <dossier>
```

```bash
codex plugin add denden@denden
```

Au premier lancement de Codex :
1. **Hooks :** Codex demande de vérifier deux hooks du plugin `denden@denden`, `hook_prompt` et `hook_stop`. Approuvez-les, ou faites-le plus tard dans `/hooks`.
2. **Permissions :** si `/status` affiche « Workspace (granular) », passez `/permissions` sur « Ask for approval ». Sinon, Codex refuse de lancer denden hors de son bac à sable.
3. **Commandes :** à la première commande denden, Codex demande l'autorisation de la lancer hors du bac à sable. Choisissez l'option qui autorise ce préfixe une fois pour toutes.

## 4. Premier essai

Dans une conversation : `/denden:im create mon-projet as coord` (Claude) ou `$im create mon-projet as coord` (Codex).

Pour faire entrer une autre machine, lancez `/denden:im invite` sur une machine du canal. Sur la nouvelle machine, lancez `/denden:im join mon-projet as <nom>` dans une conversation. Sans navigateur, passez plutôt par un terminal, hors de l'IA :

```bash
<dossier>/plugin/skills/im/denden join mon-projet
```

## 5. Mettre à jour

Extrayez le nouveau paquet au même endroit.
- **Claude Code** le prend en compte à la session suivante.
- **Codex** en garde une copie : quittez Codex, relancez `codex plugin add denden@denden`, puis réapprouvez les hooks si Codex le demande.

## 6. Tout désinstaller, pour repartir de zéro

Dans cet ordre :

1. **Arrêter le démon**, tant que le binaire est encore là. Sous Linux :

   ```bash
   <dossier>/plugin/skills/im/denden stop
   ```

   Sous Windows (PowerShell) :

   ```powershell
   & "<dossier>\plugin\skills\im\denden.exe" stop
   ```

2. **Claude Code :**

   ```bash
   claude plugin uninstall denden@denden
   ```

   ```bash
   claude plugin marketplace remove denden
   ```

3. **Codex** (quittez Codex d'abord) :

   ```bash
   codex plugin remove denden@denden
   ```

   ```bash
   codex plugin marketplace remove denden
   ```

   Deux restes à supprimer à la main :
   - dans `~/.codex/config.toml`, les sections `[hooks.state."denden@denden:…"]` (deux lignes chacune). Sinon, une réinstallation retrouve les hooks déjà approuvés ;
   - le dossier `~/.codex/plugins/cache/denden`.

4. **Les données** : canaux, clés de machine, historique, conversations branchées. **C'est irréversible** : la machine devra être invitée à nouveau dans chaque canal. Sous Linux :

   ```bash
   rm -rf ~/.denden
   ```

   Sous Windows (PowerShell) :

   ```powershell
   Remove-Item -Recurse -Force "$HOME\.denden"
   ```

5. **Le dossier du paquet**, puis, si vous voulez, la règle `"Read(~/.denden/**)"` de `~/.claude/settings.json`.

Restes d'agent-im, l'ancienne version : le plugin `im@agent-im`, la place de marché `agent-im`, le dossier `~/.agent-im` et la règle `"Read(~/.agent-im/**)"`. Ils se suppriment de la même façon.
