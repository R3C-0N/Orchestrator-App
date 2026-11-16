# capture-tui : captures de référence du TUI et fixtures stream-json (jalon J1)

Outils du jalon J1 du plan (`docs/plan-prototype-1.md`, section 1.7 et jalon J1). Rien ici n'est utilisé par l'application.

| Fichier | Rôle |
| - | - |
| `capture.ps1` | ouvre une fenêtre Windows Terminal, y lance Claude Code sur un scénario, attend l'état voulu, capture la fenêtre en PNG |
| `scenarios.json` | réglages communs et scénarios (prompt, fichiers, étapes) |
| `files/` | fichiers copiés dans le dossier d'un scénario (`notes.txt`, `config.py`) |
| `fixture-driver.mjs` | pilote stream-json qui produit les captures brutes `t13` et `t14` |
| `anonymize.mjs` | anonymise une capture brute avant de la copier dans `fixtures/stream-json/` |
| `gen-t15.mjs` | génère `fixtures/stream-json/t15.jsonl` (session longue) à partir des autres fixtures |

## Captures d'écran (`capture.ps1`)

Prérequis : Windows 11, PowerShell 7 (`pwsh`), Windows Terminal (`wt.exe`), Node (pour le hook `Stop`), Claude Code dans `%USERPROFILE%\.local\bin\claude.exe` (option `-Claude` sinon). Le script s'utilise depuis une session où l'on peut céder le clavier une minute : les touches (`/`, Échap, Ctrl+C…) partent par `SendKeys` vers la fenêtre ouverte par le script, **et seulement si elle est au premier plan** (sinon le script s'arrête au lieu d'envoyer la touche ailleurs).

```powershell
pwsh tools/capture-tui/capture.ps1              # les 10 captures de référence
pwsh tools/capture-tui/capture.ps1 -Only 05     # refaire une capture seule (05, ou 05,06, ou 03b)
pwsh tools/capture-tui/capture.ps1 -Scenario ctrlc,ctrlc-tour,echap-intervalles   # scénarios d'observation (hors des 10)
pwsh tools/capture-tui/capture.ps1 -List        # scénarios et captures, sans rien lancer
```

Sorties, jamais versionnées (`.claude-work/` est ignoré par git) :

- `.claude-work/j1/captures/NN-nom.png` : la fenêtre seule (`PrintWindow` avec `PW_RENDERFULLCONTENT`, sinon `CopyFromScreen` sur son rectangle si l'image obtenue est uniforme) ;
- `.claude-work/j1/captures/NN-nom.uia.txt` : le texte visible du terminal au même instant, lu par UI Automation (`TextPattern` du `TermControl`), avec les glyphes exacts ; les couleurs n'y sont pas ;
- `.claude-work/j1/sandbox/<scénario>/` : dossier de travail jetable du scénario, recréé à chaque passage.

Les PNG ne sont pas commités : ils montrent le chemin du bac à sable, qui contient le nom d'utilisateur, et une image ne se vérifie pas par `grep`.

### Déroulement d'un scénario

1. Recrée `.claude-work/j1/sandbox/<scénario>/`, y copie les fichiers de `files/`, y écrit :
   - `.claude-ref-settings.json`, passé à Claude par `--settings` : thème (`dark`), rendu (`tui`), hook `Stop` qui ajoute une ligne à `.turn-done` à chaque fin de tour, liste `permissions.allow` (pas de demande d'autorisation pendant la capture) ;
   - `run.cmd` : efface les variables d'environnement héritées (`CLAUDE*`, `ANTHROPIC_*`, `NO_COLOR`, `FORCE_COLOR`, `TERM`, `COLORTERM`, `CI` : `wt` transmet l'environnement de l'appelant, et sous Claude Code `NO_COLOR=1` supprime toutes les couleurs), puis lance `claude.exe --model haiku --settings … "<prompt>"`.
2. `wt.exe -w new --size 120,40 --pos 40,40 new-tab -p "Invite de commandes" --colorScheme Campbell --title claude-ref-<scénario>-<n> --suppressApplicationTitle -d <dossier> cmd.exe /c run.cmd` : une nouvelle fenêtre, profil sans police imposée (Cascadia Mono 12 pt par défaut), jeu de couleurs Campbell (celui de Windows Terminal par défaut). Aucun profil ni réglage de Windows Terminal n'est modifié.
3. Retrouve la fenêtre par son titre (classe `CASCADIA_HOSTING_WINDOW_CLASS`), puis corrige sa largeur jusqu'à exactement 120 colonnes (`--size` donne 132 colonnes sur ce poste à cause de la mise à l'échelle) en mesurant le texte visible.
4. Déroule les étapes du scénario :

   | Étape | Effet |
   | - | - |
   | `wait` `ms` | attente fixe |
   | `waitText` `pattern` `timeoutMs` | attend qu'une regex .NET apparaisse dans le texte visible |
   | `waitStop` `count` `timeoutMs` | attend la `count`-ième fin de tour (lignes de `.turn-done`) |
   | `keys` `send` | touches au format `SendKeys` (`{ESC}`, `^c`, `^o`, `{ENTER}`, `/`) |
   | `type` `text` | texte tapé tel quel (caractères spéciaux de `SendKeys` échappés) |
   | `capture` `id` `name` | écrit `id-name.png` et `id-name.uia.txt` (si `-Only` le demande) |

   Une attente qui expire affiche un avertissement et le scénario continue : la capture est alors à vérifier. Le journal est horodaté à la milliseconde : c'est lui qui donne l'intervalle réel entre deux touches (`echap-intervalles` mesure le double Échap à 0, 100, 200, 300, 500, 600, 700 et 800 ms, avec une capture avant et après chaque essai).

   Si la session Windows est verrouillée, la fenêtre ne peut pas passer au premier plan : le script s'arrête sans envoyer de touche.
5. Ferme la fenêtre (`WM_CLOSE`) et termine le `cmd.exe` de `run.cmd` et toute sa descendance (repérés par le chemin de `run.cmd` dans la ligne de commande), sans toucher aux autres processus `claude.exe`.

### Réglages (`scenarios.json`)

`cols`/`rows` (120 × 40), `wtProfile`, `colorScheme`, `model` (`haiku`, pour le coût), `theme` (`dark`), `tui` (`fullscreen` : le rendu réglé sur ce poste ; `default` pour le rendu classique), `allow` (outils autorisés sans demande). Un scénario porte `name`, `prompt` facultatif (passé en argument, donc soumis au lancement), `files`, `steps`, et `extra: true` pour les scénarios d'observation qui ne font pas partie des 10.

À savoir : si un lancement en rendu `fullscreen` est tué pendant son démarrage, le CLI note l'échec et le lancement suivant retombe sur le rendu classique (bandeau « Claude Code's fullscreen renderer didn't finish starting last time… »). Le script le signale par un avertissement : refaire la capture concernée avec `-Only`.

## Fixtures stream-json

```bash
node tools/capture-tui/fixture-driver.mjs t13 .claude-work/j1/raw/t13.raw.jsonl
node tools/capture-tui/anonymize.mjs .claude-work/j1/raw/t13.raw.jsonl fixtures/stream-json/t13.jsonl
node tools/capture-tui/fixture-driver.mjs t14 .claude-work/j1/raw/t14.raw.jsonl
node tools/capture-tui/anonymize.mjs .claude-work/j1/raw/t14.raw.jsonl fixtures/stream-json/t14.jsonl
node tools/capture-tui/gen-t15.mjs
```

- `fixture-driver.mjs` lance `claude.exe -p --input-format stream-json --output-format stream-json --verbose` + les options du scénario dans `.claude-work/j1/sandbox/<tNN>/`, avec un environnement débarrassé des variables `CLAUDE*`/`ANTHROPIC_*`, et écrit le même format que les fixtures `t1` à `t12` (`>>> ` envois, `STDERR `, `EXIT n`). La sortie brute n'est pas anonymisée.
- `anonymize.mjs` applique les règles de `fixtures/stream-json/README.md` (nom d'utilisateur → `<user>`, dossier temporaire de session → `<session>`, `signature` → `REDACTED`, `email`/`organization`/`subscriptionType` → `REDACTED`, fragments `input_json_delta` recollés quand une donnée à masquer est coupée), puis vérifie que chaque ligne passe `JSON.parse` et qu'il ne reste aucune trace du nom d'utilisateur (code de sortie 1 sinon).
- `gen-t15.mjs` est déterministe ; son en-tête décrit la construction.

Vérification avant commit :

```bash
grep -ci -E "<nom-utilisateur>|<domaine>" fixtures/stream-json/t13.jsonl fixtures/stream-json/t14.jsonl fixtures/stream-json/t15.jsonl   # 0 partout
```
