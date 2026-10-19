# denden : architecture technique

Ce document traduit [design.md](design.md) en choix techniques. Il décrit ce que le code doit faire ; le document de conception reste la référence pour l'expérience utilisateur.

## 1. Choix par défaut pris pour démarrer

Points de la section 12 de design.md tranchés ici pour avancer. Ils restent révisables :
- **Langue des textes fixes : l'anglais.** C'est la langue des premiers exemples validés (`Received message from bob@project-xx`) et celle d'un plugin distribué. L'IA rapporte ces lignes dans la langue de l'utilisateur, en gardant la forme `> 🐌» (( … ))`.
- **Sorties sur plusieurs lignes** : chaque ligne est une citation `> 🐌» (( … ))`, la première sert de titre.
- **`DENDEN_AS` disparaît.** Une conversation est identifiée par sa session : `--session ${CLAUDE_SESSION_ID}` pour Claude, `CODEX_THREAD_ID` pour Codex.
- **Version du protocole : 3.** Les libellés de dérivation (`agent-im/v2/...`) ne changent pas ; ce qui change, ce sont les échanges, et la preuve d'ouverture annonce `v: 3`. Aucune compatibilité avec agent-im n'est recherchée.
- **Avis de nouvelle machine** : il va à toutes les conversations branchées sur le canal, sur les machines qui la voient arriver, sans les réveiller. Il leur est remis avec leurs prochains messages.
- **Commandes sensibles** : en plus de `invite`, `block`, `unblock` et `auto`, le skill interdit de lancer `join`, `detach`, `delete`, `leave`, `wakes` et `keepalive` à la demande d'un message.
- **Page d'invitation après la copie** : elle affiche la suite à faire, puis se ferme au bout de 60 secondes.
- **Doublon de nom de canal (question 33)** : le nom local d'un canal est celui que l'utilisateur tape dans `join`. `join vibedoc-paul as front` rejoint, sous ce nom local, le canal du code collé, quel que soit le nom que lui donne son créateur ; `join` sur un nom déjà présent branche la conversation sur ce canal-là.

## 2. Données sur le disque (`~/.denden`)

```text
~/.denden/
  daemon.json  daemon.lock  daemon.log   le démon de la machine
  bin/                                   copies vérifiées du binaire (Windows verrouille un exe lancé)
  conversations.json                     conversation -> canal, denden, application, mode, budget
  channels/<cid>/                        un dossier par canal ; <cid> = 16 premiers hex de l'id du canal
    channel.json                         secret, libellé local, graine de la clé machine, certificats, invitations
    members.json                         machines connues (clé -> nom, vue la dernière fois), clés bloquées
    names.json                           certificats de noms : denden -> machine propriétaire
    messages.jsonl                       messages reçus et envoyés, par denden local
    state.json                           ce qui a été lu, par denden local
    outbox.json                          messages en attente, par machine destinataire
```

- **Une clé de machine par canal**, pour qu'on ne puisse pas relier ses canaux entre eux. Son nom est le nom d'hôte nettoyé.
- **Le dossier d'un canal porte son identifiant**, pas son libellé, qui n'est qu'un nom local modifiable au moment de rejoindre.
- **Une conversation** se note `claude:<session>` ou `codex:<thread>`.

## 3. Protocole (version 3)

Dans chaque flux chiffré par Noise, des trames JSON séparées par des retours à la ligne :

| trame | rôle |
|---|---|
| `{t:'auth', v:3, proof}` | preuve de connaissance du secret, avant tout autre échange (inchangée) |
| `{t:'hello', machine, key, sig, chain?, inv?, needCert?, names}` | la machine se présente ; `names` porte les certificats de noms qu'elle connaît |
| `{t:'cert', chain}` | un membre remet un certificat d'appartenance à la machine qu'il vient d'admettre |
| `{t:'names', names}` | nouveaux noms, suppressions, abandons après conflit |
| `{t:'msg', id, from, to, text, ts, ask?, re?, waited}` | un message ; `to` est une liste de denden ou `"all"` |
| `{t:'ack', id}` / `{t:'nack', id, error}` | accusé de réception, ou refus |
| `{t:'error', error}` | refus de la connexion |

**Appartenance.** Une machine est admise par un jeton d'invitation à usage unique, puis reçoit un certificat signé par celle qui l'a invitée. Les autres l'acceptent si la chaîne de certificats remonte à une clé qu'ils connaissent. Les certificats désignent des clés de machine : deux machines peuvent porter le même nom.

**Certificats de noms.** `{n, k, t, s, d?, r?}` : le denden `n` appartient à la machine de clé `k`, créé à la date `t`, signé par `k`. `d` marque une suppression (le nom reste lié à la machine), `r` un abandon après conflit. Chaque machine les diffuse dans son `hello` et par `names`. Règles :
- un nom inconnu est accepté s'il est signé par une machine membre ;
- un nom déjà lié à une autre machine est refusé, sauf si celle-ci l'abandonne (`r`) ;
- un conflit qui touche un de nos noms : si l'autre machine l'a créé avant nous, on l'abandonne et on prévient la conversation ; sinon on le garde.

**Messages.**
- Une machine ne peut écrire qu'au nom de ses propres denden : l'émetteur `from` doit appartenir à la clé de la connexion.
- Le destinataire n'accepte que les noms qui lui appartiennent ; `"all"` vaut pour tous ses denden.
- Entre deux denden de la même machine, rien ne passe par le réseau.
- `waited` : le temps passé chez l'expéditeur avant cet envoi, pour mesurer le retard sans dépendre des horloges.

## 4. Le démon

- **Un seul démon par machine**, une API HTTP locale protégée par un jeton (`daemon.json`), une connexion d'essaim par canal et par époque, comme dans agent-im.
- **Routage** : un envoi est découpé par machine destinataire. Chaque part attend dans `outbox.json` jusqu'à son accusé de réception, et elle est renvoyée à chaque reconnexion. Au bout de 7 jours, elle est abandonnée, et le denden expéditeur reçoit un avis.
- **Lecture** : chaque denden a son propre état de lecture. Un message est remis à une conversation par l'écoute de Claude, par les hooks de Codex ou par `inbox`.
- **Retard** : à la remise, `waited + (remise - réception) >= 1 h` marque le message comme en retard.
- **Historique** : les messages de plus de 30 jours sont purgés au démarrage, puis une fois par jour.
- **Durée de vie** : le démon s'arrête après 10 minutes sans présence, sauf avec `keepalive on`. Comptent comme présence une écoute de Claude ouverte, un serveur MCP de Codex connecté, ou un hook reçu.
- **Réveils** : un budget par conversation, de 20 par heure par défaut. Pour Claude, l'écoute cesse d'afficher au-delà, annonce la limite une fois, et les messages attendent le prochain message de l'utilisateur (le hook de prompt les remet). Pour Codex, le budget s'applique aux avis mis en file.

## 5. La CLI

- Toutes les commandes passent par le démon, qu'elles démarrent au besoin.
- La conversation vient de `--session` (Claude), de `CODEX_THREAD_ID` (Codex, ramené à la conversation de l'utilisateur pour un fil lancé par Codex), ou n'existe pas (terminal).
- **Quand une décision revient à l'utilisateur**, la CLI n'attend pas de saisie. Elle écrit la question au format `🐌»  **…**` et sort avec le code 3. L'IA pose la question, puis relance la commande avec la réponse (`as <nom>`, `--move`, `--label <nom>`).
- **Commandes internes**, qui ne sont pas montrées à l'utilisateur : `inbox`, `watch`, `reply <id> <texte>`, `hook-prompt`, `hook-stop`, `mcp`, `stop`.

## 6. Phases

| # | Phase | Contenu | Fin de phase |
|---|---|---|---|
| 1 | Socle | dépôt, workspace, crate renommé, plugin sans Node | ça compile, tests unitaires verts |
| 2 | Modèle | invitations v3, certificats de machine et de nom, stockage par canal, registre des conversations | tests unitaires des règles (conflits, signatures, retard) |
| 3 | Démon | démon unique, protocole v3, routage, attente, purge, présence, `keepalive` | deux machines simulées (deux `DENDEN_HOME`) s'échangent des messages |
| 4 | CLI | toutes les commandes et leurs sorties 🐌, invitations 20 min et page 15 min, `denden join` masqué dans un terminal | le parcours de design.md rejoué à la main |
| 5 | Réveils | écoute Claude, file Codex, hooks, budget, messages en retard, présence MCP | réveil dans Claude et dans Codex |
| 6 | Plugin | SKILL.md réécrit, page, manifestes, script de construction des binaires, README | installation depuis les places de marché locales |
| 7 | Validation | tests de bout en bout, test avec l'utilisateur dans Claude et Codex | parcours complet validé |

## 7. Limites connues

### NAT strict

Mesuré le 19 octobre 2026 avec `tests/e2e/nat.sh`. Une machine se trouvait dans un espace réseau Linux derrière un NAT `nft masquerade fully-random`, qui donne un port différent pour chaque destination. L'autre machine se trouvait derrière une box ordinaire.

| Cas | Résultat |
|---|---|
| Machine publique ↔ NAT ordinaire | connecté en 15 à 17 s |
| NAT ordinaire ↔ NAT ordinaire | connecté en 16 à 23 s |
| NAT strict ↔ NAT ordinaire | connecté en général, en 24 à ~100 s ; quelques essais n'aboutissent pas dans les 90 s d'attente de `join` |
| NAT strict ↔ NAT strict | non testé ; impossible sans relais |

Le comportement de peeroxide (fork, commit `43d3de4`) :
- **Côté qui initie la connexion :** derrière un NAT strict, peeroxide ouvre 256 sockets de secours (attaque des anniversaires), et la sonde gagnante arrive sur l'une d'elles. Avant `43d3de4`, la connexion partait ensuite de la socket principale, que le NAT traduit vers un autre port, et elle n'aboutissait jamais. Elle part désormais de la socket gagnante, comme dans la version Node.
- **Côté qui reçoit, derrière un NAT strict : non pris en charge.** libudx n'attache une connexion en attente qu'à la socket principale, alors que les paquets arrivent sur une socket de secours. Une machine derrière un NAT strict ne peut donc être jointe par personne : il faut que ce soit elle qui ouvre la connexion. Comme chaque paire de machines tente les deux sens en même temps, cela suffit le plus souvent.
- **Deux NAT stricts :** aucun perçage ne peut aboutir. Il faudrait un relais aveugle : peeroxide sait le faire (`peeroxide relay`, `SwarmConfig::relay_through`), mais denden ne l'utilise pas encore. Ce relais ne verrait pas le contenu (chiffrement de bout en bout), seulement les adresses IP, les horaires et les volumes. Il faut l'héberger sur une machine joignable : peeroxide ne fournit pas de relais public.

Pistes, à trancher plus tard :
1. dans libudx, recevoir le premier paquet d'une connexion en attente sur n'importe quelle socket du perçage, comme le fait udx-native ;
2. dans denden, un relais aveugle de secours, hébergé par un membre du canal qui a une IP publique.
