# denden 🐌

Des messages entre vos conversations d'IA (Claude Code, Codex), sur cette machine et sur d'autres, de pair à pair : sans serveur, chiffrés de bout en bout, dans des canaux sur invitation. On s'en sert depuis une conversation avec `/denden:im …` (Claude Code) ou `$im …` (Codex).

- **Un canal** = un projet. **Une machine** y entre une fois, avec une invitation.
- **Chaque conversation** a son **denden**, son adresse dans le canal (`coord`, `front`, `api`…), et s'y branche sans code.
- On écrit à un denden, à plusieurs, ou à tout le canal.

La conception est décrite dans [docs/design.md](docs/design.md), l'architecture dans [docs/architecture.md](docs/architecture.md).

## Exemple

```text
/denden:im create vibedoc-chan as coord        conversation 1 : crée le canal
/denden:im join vibedoc-chan as front          conversation 2, même machine : sans code
/denden:im invite                              pour faire entrer une autre machine
/denden:im join vibedoc-chan as api            sur l'autre machine : colle le code sur la page locale
/denden:im ask api "L'endpoint /users est-il prêt ?"
/denden:im who
```

## Installation

Ce dépôt est une place de marché locale, `denden`, qui contient le plugin `denden`. Construisez d'abord les binaires du plugin (voir plus bas).

### Claude Code

```bash
claude plugin marketplace add D:/Projects/P2P/denden
```

```bash
claude plugin install denden@denden
```

Ouvrez ensuite une nouvelle session. Claude Code charge un plugin d'une place de marché locale directement depuis le dépôt : une modification est prise en compte à la session suivante.

Un plugin ne peut pas déclarer de permissions. Ajoutez vous-même cette règle à `~/.claude/settings.json`, pour que l'IA ne lise jamais les clés de denden :

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

Codex copie le plugin dans son cache : après une modification, quittez Codex et relancez `codex plugin add denden@denden`. Installer un plugin n'approuve pas ses hooks : ouvrez `/hooks` dans Codex et approuvez les deux hooks du plugin `denden`.

denden a besoin du réseau et de `~/.denden`, que le bac à sable de Codex bloque : Codex demande à lancer ses commandes hors du bac à sable.

## Construction

```bash
sh scripts/build-plugin.sh
```

Le script compile `denden.exe` (Windows x64) et `bin/linux-x64/denden` (Linux x64 statique, compilé dans WSL), les place dans `plugin/skills/im/` et écrit `SHA256SUMS`. Ces binaires ne sont pas versionnés.

peeroxide vient du fork `alaa-eddine/peeroxide`, figé sur un commit dans `Cargo.toml`. Pour le modifier, pointez temporairement vers `D:/Projects/peeroxide`.

## Tests

```bash
cargo test
```

```bash
sh tests/e2e/two-machines.sh target/debug/denden
```

Le second simule deux machines (deux `DENDEN_HOME`, deux noms de machine) sur le vrai réseau P2P : création, invitation, rejoindre, question et réponse, envoi à tout le canal, `who`, reprise et déplacement d'un denden, écoute.

## Données

Tout est dans `~/.denden` (ou `$DENDEN_HOME`) : un dossier par canal (secret, clé de la machine, machines connues, noms, messages, file d'attente), les conversations branchées, le démon. Un seul démon par machine ; il s'arrête quand plus aucune conversation n'écoute, sauf avec `keepalive on`.
