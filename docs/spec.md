# Spécification : interface de bureau pour Claude Code

Statut : prototype 1, spécification initiale. Version du CLI étudiée : `claude` 2.1.325 (natif, `C:\Users\<user>\.local\bin\claude.exe`), Windows 11. Les preuves sont dans `fixtures/stream-json/` (`t1` à `t12`, voir son README, qui donne la ligne de commande de chacune) et dans `.claude-work/reference/sdk.d.ts` (types du SDK 0.3.325).

## 1. But et problème d'origine

Remplacer l'interface terminal de Claude Code par une application de bureau qui pilote le CLI `claude` sans interface graphique, par le flux JSON (`stream-json`). L'application dessine elle-même la conversation, un panneau de sous-agents et, plus tard, des formulaires natifs.

Problème d'origine : l'interface terminal (TUI) de Claude Code sous Windows Terminal fige par moments et son défilement est inconfortable sur les longues sessions. Piloter le CLI par le flux JSON supprime la couche TUI : le rendu et le défilement deviennent ceux de notre application (liste virtualisée).

## 2. Décisions et conséquences

1. **Techno : Tauri 2, cœur Rust, interface web.** Le cœur Rust lance et supervise `claude.exe`, analyse le flux et expose des commandes et événements Tauri à l'interface. Conséquence : pas de Node obligatoire à l'exécution, donc pas de SDK TypeScript ; le protocole de contrôle est réimplémenté en Rust (voir section 5).
2. **Front : Svelte 5.** Conséquence : état de l'interface en runes (`$state`, `$derived`) ; un magasin par session reçoit les événements du cœur.
3. **Look : terminal pur.** Reproduire d'abord fidèlement le style terminal de Claude Code ; les modifications viendront après. Conséquence : police monospace, rendu par lignes et blocs, pas de bulles ni de cartes décoratives dans la conversation ; la fidélité au TUI est le critère de recette du prototype 1.
4. **Disposition : conversation + colonne des agents à droite**, elle aussi en style terminal : police mono, lignes de texte colorées par agent (bleu, vert, orange, violet, rose, cyan, jaune, rouge), d'après le `subagent_type` après `trim()`. Conséquence : une table couleur par nom d'agent (`agent-bleu` vers bleu, etc.) ; un agent inconnu reçoit une couleur neutre.
5. **Périmètre du prototype 1 : « utilisable au quotidien »** (détail en section 6). Les formulaires natifs n'en font pas partie ; les questions de Claude (`AskUserQuestion`) sont néanmoins affichées dans une carte minimale (décision 6).
6. **Formulaires par étapes.** Prototype 1 : les questions de Claude (`AskUserQuestion`) sont affichées dans une **carte minimale en style terminal** : la question et ses options en liste, choisies au clavier (↑↓, Entrée) ou à la souris, avec un champ « autre réponse » libre ; la réponse passe par `updatedInput.answers`, et la capacité `ask_user_question_unanswered_v1` sert à « ignorer ». Ce n'est pas un formulaire natif, et il n'y a pas d'aperçu HTML des options (reporté, voir section 7). Plus tard : un outil MCP « formulaire » fourni par l'app, qui remplacera le skill `formulaire-dynamique` en artefact. Conséquence : le prototype 1 doit gérer le `control_request` `can_use_tool` de `AskUserQuestion` (testé, section 4.5) ; l'outil MCP in-process nécessitera d'implémenter côté Rust le transport MCP sur le canal de contrôle (`mcp_message`), non étudié.
7. **Permissions : tout autoriser (`bypassPermissions`).** L'app reste branchée sur `--permission-prompt-tool=stdio` pour recevoir les `AskUserQuestion` et les éventuels `can_use_tool` restants. Conséquence : le gestionnaire de `control_request` existe dès le prototype 1, avec réponse `allow` par défaut pour tout ce qui n'est pas `AskUserQuestion`. Testé (`t8`) : le mode est accepté sans `--allow-dangerously-skip-permissions`, un Bash qui écrit dans le dossier de travail part sans `can_use_tool`, et `AskUserQuestion` arrive toujours en `control_request`. Les outils MCP marqués `requires_user_interaction` et certaines demandes (`default_to_no`, `safetyCheck`) peuvent rester soumis à demande (doc, non vérifié).
8. **Dépôt : `H:\<user>\Code\orchestrator`**, dépôt git local, branche `main`, sans remote.

## 3. Architecture

```
[ Interface Svelte 5 ]  <-- événements Tauri / commandes -->  [ Cœur Rust ]  <-- stdin/stdout NDJSON -->  [ claude.exe ]
 liste virtualisée, panneau agents,                           spawn, lecture ligne à ligne,              session longue,
 palette Ctrl+K, saisie                                       enums serde, corrélation des               outils, sous-agents,
                                                              control_request                            skills, hooks, MCP
```

- **Un processus `claude.exe` par session**, qui reste ouvert tant que stdin est ouvert. Le cœur Rust écrit un message utilisateur JSON par ligne sur stdin et lit un événement JSON par ligne sur stdout.
- **Cœur Rust** : lancement, lecture robuste (certaines lignes font plusieurs ko), désérialisation en enums, table des sous-agents (`tool_use_id`), table des `control_request` en attente (par `request_id`), envoi de `control_response` et de `control_request` (interrupt, set_model). Il transmet à l'interface des événements déjà normalisés.
- **Interface Svelte** : conversation en liste virtualisée alimentée par les deltas de texte, colonne des agents, palette de skills, sélecteur de sessions.
- Répertoire de travail du processus = dossier du projet ouvert ; `claude` y charge ses skills, agents, hooks et MCP comme en interactif (sauf `--bare`).

## 4. Référence du protocole

Source : tests réels de l'enquête (fixtures) et documentation https://code.claude.com/docs/en/headless, https://code.claude.com/docs/en/cli-reference, https://code.claude.com/docs/en/agent-sdk/user-input. Les types complets des messages de contrôle sont dans `sdk.d.ts` du SDK 0.3.325, copie locale non versionnée `.claude-work/reference/sdk.d.ts` (voir section 5). Aucune spécification de protocole de fil n'est publiée séparément : la référence de fait est le SDK.

### 4.1 Commande de lancement

```
claude.exe -p --input-format stream-json --output-format stream-json --verbose ^
  --include-partial-messages --forward-subagent-text ^
  --permission-mode bypassPermissions --permission-prompt-tool stdio
```

Options testées (une par une dans les fixtures) :

| Option | Rôle | Preuve |
| - | - | - |
| `--verbose` | requis avec `--output-format stream-json` | doc headless |
| `--include-partial-messages` | deltas `stream_event` (`text_delta`, `input_json_delta`) | `t1`, `t6` |
| `--replay-user-messages` | ré-émet les messages utilisateur envoyés sur stdin, `"isReplay":true`, avec `uuid` | `t1` |
| `--forward-subagent-text` | émet aussi texte et thinking des sous-agents (sinon seuls `tool_use` / `tool_result` en avant-plan) | `t3`, doc CLI |
| `--permission-mode` | `acceptEdits, auto, bypassPermissions, manual, dontAsk, plan` (liste de `claude --help` 2.1.325, `manual` y figure bien) ; `manual` est accepté : `t1` a été lancé avec `--permission-mode manual` (ligne de commande dans le README des fixtures ; la capture elle-même ne contient pas ses arguments), a abouti normalement, et `system/init` y rapporte `"permissionMode":"default"`, alors que `t6` et `t7`, lancés sans `--permission-mode`, rapportent `"auto"` (réglage utilisateur) ; le type SDK `PermissionMode` (`sdk.d.ts:2652`) ne connaît que `default`, pas `manual` (`t4`, `t5`, `t9` avec `default`, qui donne aussi `"default"`) | `t1`, `t4`, `t6`, `t8` |
| `--permission-prompt-tool stdio` | **flag non documenté** : active les `can_use_tool` et l'outil `AskUserQuestion` ; le SDK l'écrit `--permission-prompt-tool=stdio` (code de `sdk.mjs`) | `t4`, `t5`, `t8` |
| `--permission-prompts none` | **à ne pas utiliser** : retire `AskUserQuestion` (doc headless) et refuse tout ce qui demanderait une autorisation ; incompatible avec la décision 7 | doc headless |
| `--allow-dangerously-skip-permissions` | non nécessaire : `bypassPermissions` est accepté sans lui (`t8`), alors que `sdk.d.ts:2652` écrit « Bypass all permission checks (requires allowDangerouslySkipPermissions) » | `t8` |
| `--session-id <uuid>` | impose l'id de session | `t5` |
| `--resume <id>` | reprend une session ; garde le même `session_id` | `t7` |
| `--model <alias>` | alias vus dans `claude --help` : `fable`, `opus`, `sonnet` (plus `haiku`, accepté) ou nom complet. Tests : `--model haiku` dans `t3` à `t12` (économie de coût, facultatif) ; sans `--model`, le modèle par défaut est `claude-opus-5-5` (`t1`) ; `set_model` vers `sonnet` en cours de session (`t5`) | `t1`, `t3` à `t12` |

**Commande définitive du prototype 1** : celle ci-dessus, testée en entier dans `t8` (qui ajoute `--model haiku`, facultatif, et n'utilise pas `--replay-user-messages`, facultatif). Résultat : le mode `bypassPermissions` est accepté avec `--permission-prompt-tool stdio` et sans `--allow-dangerously-skip-permissions` (`init.permissionMode` = `bypassPermissions`) ; un Bash qui écrit dans le dossier de travail s'exécute sans `can_use_tool` ; `AskUserQuestion` figure dans `init.tools` et arrive en `control_request` `can_use_tool` (seul `control_request` du tour, `t8`). Le SDK documente `allowDangerouslySkipPermissions` comme requis ; le CLI brut ne l'exige pas pour ce test (une note de doc distincte du CLI est à surveiller aux mises à jour).

### 4.2 Message utilisateur (stdin)

Une ligne JSON par message :

```json
{"type":"user","message":{"role":"user","content":"Dis bonjour en un mot."}}
```

Un message commençant par `/nom-de-skill args` invoque le skill (testé avec `/orchestrateur`, `t5`). Le processus reste ouvert entre deux tours (testé, `t1`) ; il se termine à la fermeture de stdin.

### 4.3 Événements émis (stdout)

Séquence typique d'un tour (`t1`) : `system/hook_started`, `system/hook_response`, `system/init`, `system/status`, `user` (replay), `stream_event`*, `assistant`, `rate_limit_event`, `result`.

- `system/init` : `session_id`, `model`, `permissionMode`, `cwd`, `tools`, `mcp_servers` (`{name,status,source}`), `slash_commands`, `skills`, `agents`, `plugins`, `capabilities` (19 chaînes, test de fonctionnalités), `claude_code_version`. **Il n'est émis qu'après le premier message utilisateur** (`t12` : processus lancé sans message utilisateur, puis stdin fermé ; la capture ne contient que des `hook_*`, aucun `init` ; elle n'est pas horodatée, aucune durée n'est donc affirmée) **puis de nouveau à chaque tour** (`t1` : 2 `init` pour 2 `result` ; `t5` : 3 pour 3). Pour remplir la palette de skills (Ctrl+K) avant le premier message, deux sources : la requête de contrôle `initialize` (voir 4.7bis, testée, `t11`) ou, à défaut, une lecture de `~/.claude/skills`, `~/.claude/agents` et des plugins ; `init` complète ensuite (états MCP, outils, modèle).
- `system/status` : `{"status":"requesting"}` au début de chaque requête API.
- `stream_event` : enveloppe de l'API Messages, `event.type` parmi `message_start`, `content_block_start`, `content_block_delta` (`text_delta`, `input_json_delta` avec `partial_json`), `content_block_stop`, `message_delta`, `message_stop` ; porte `parent_tool_use_id` et `uuid`.
- `assistant` : message complet ; blocs `text`, `thinking` (contenu vide, signature seule), `tool_use` (`id`, `name`, `input`).
- `user` avec `tool_result` : `{"tool_use_id":"…","type":"tool_result","content":"…"}`.
- `result` : `subtype` (`success`, `error_during_execution`), `is_error`, `terminal_reason` (`completed`, `aborted_streaming`), `result` (texte), `total_cost_usd`, `duration_ms`, `duration_api_ms`, `num_turns`, `usage`, `modelUsage` (avec `contextWindow`), `permission_denials`, `subagent_stats`.
- `rate_limit_event` : `rate_limit_info` (statut, fenêtres d'utilisation).
- Autres `system/*` observés : `thinking_tokens`, `task_started`, `task_progress`, `task_updated`, `task_notification`, `background_tasks_changed`. Décrits par la doc ou le SDK, non capturés : `api_retry`, `permission_denied`, `plugin_install`, `control_request_progress`, `prompt_suggestion`.

### 4.4 Sous-agents (`t3`)

- L'appel : un `assistant` du fil principal avec un `tool_use` de nom **`Agent`** (`input.subagent_type`, `description`, `prompt`, `run_in_background`). Vérifié : dans `t3`, les 2 `tool_use` ont `"name":"Agent"`, alors que `init.tools` (`t1`, `t3`, `t5`, `t8`) liste **`Task`** et jamais `Agent`. `sdk.d.ts` ne liste que `Agent` dans `BUILTIN_TOOL_NAMES` : `Task` est l'ancien nom, resté dans la liste d'outils de l'`init`. L'app doit reconnaître **les deux noms** (`Agent` et `Task`) dans un `tool_use` ; seul `Agent` a été observé dans le flux.
- `system/task_started` : `task_id`, `run_id`, **`tool_use_id`** (= id du `tool_use` Agent, clé de liaison), `description`, `subagent_type`, `is_backgrounded`, `spawn_depth`, `task_type`, `prompt`.
- Messages du sous-agent : `assistant` et `user` avec `parent_tool_use_id` = id du `tool_use` Agent (les messages du fil principal ont `null`). Les sous-agents imbriqués portent l'id de l'Agent ou du Skill parent (doc, non testé).
- `system/task_progress` : `description`, `usage:{total_tokens,tool_uses,duration_ms}`, `last_tool_name` (source de « outil en cours »).
- Fin : `system/task_updated` (`patch:{status:"completed",end_time}`) puis `system/task_notification` (`status`, `summary`, `output_file`, `usage`). Un sous-agent d'avant-plan renvoie en plus un `tool_result` au fil principal.
- Arrière-plan : `run_in_background:true` ; `tool_result` immédiat « Async agent launched » ; `system/background_tasks_changed` donne la liste des tâches actives (`tasks:[]` à la fin).
- Parallélisme : plusieurs sous-agents s'entrelacent dans le flux ; on les distingue par `parent_tool_use_id` / `tool_use_id`.
- Skill forké lancé par `/skill` : messages avec `parent_tool_use_id` préfixé `forked-command-`, arrivant groupés après coup (doc, non testé).
- Couleur : `subagent_type.trim()` (voir pièges).

### 4.5 Protocole de contrôle

Enveloppes (types : `SDKControlRequest`, `SDKControlResponse` dans `.claude-work/reference/sdk.d.ts`) :

```json
{"type":"control_request","request_id":"<id>","request":{"subtype":"…"}}
{"type":"control_response","response":{"subtype":"success","request_id":"<id>","response":{…}}}
```

Le CLI et l'app peuvent chacun émettre des `control_request`, chacun répond par un `control_response` au même `request_id`. Un `control_cancel_request` annule une demande en attente (par exemple un `can_use_tool` après un interrupt).

**Permission, CLI vers app (`t4`)**

```json
{"type":"control_request","request_id":"bda218c7-…","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"echo salut > b.txt","description":"…"},"description":"…","permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"echo salut *"}],"behavior":"allow","destination":"localSettings"}],"blocked_path":"…","tool_use_id":"toolu_019Sj…"}}
```

Réponse (testée, accepte) :

```json
{"type":"control_response","response":{"subtype":"success","request_id":"bda218c7-…","response":{"behavior":"allow","updatedInput":{"command":"echo salut > b.txt","description":"…"}}}}
```

Refus (capturé, `t9`, mode `default`) : réponse `{"behavior":"deny","message":"Refusé par l'utilisateur (test)"}`. Claude reçoit un `tool_result` d'erreur portant ce message (`{"type":"tool_result","content":"Refusé par l'utilisateur (test)","is_error":true,"tool_use_id":"toolu_013u…"}`), l'outil ne s'exécute pas (le fichier n'est pas créé), le tour continue (`result` `success`, texte « refusé ») et `result.permission_denials` liste l'appel : `[{"tool_name":"Bash","tool_use_id":"toolu_013u…","tool_input":{"command":"echo deux > d.txt",…}}]`. Champ optionnel `interrupt` du refus défini dans `sdk.d.ts` (`PermissionResult`), non testé. « Toujours autoriser » : renvoyer une suggestion dans `updatedPermissions`. Champs utiles de la demande : `tool_use_id`, `agent_id`, `decision_reason`, `requires_user_interaction`, `default_to_no`.

**AskUserQuestion, CLI vers app (`t4`)** : même `can_use_tool` avec `tool_name:"AskUserQuestion"` et `requires_user_interaction:true`.

```json
"input":{"questions":[{"question":"Quelle est ta couleur préférée ?","header":"Couleur","options":[{"label":"Rouge","description":"Rouge"},{"label":"Bleu","description":"Bleu"}],"multiSelect":false}]}
```

Réponse (testée) : `allow` avec `updatedInput` = `questions` recopiées + `answers` (clé = texte de la question, valeur = label choisi) :

```json
{"behavior":"allow","updatedInput":{"questions":[…],"answers":{"Quelle est ta couleur préférée ?":"Bleu"}}}
```

Selon la doc : multi-sélection = tableau de labels (ou labels joints par `", "`) ; texte libre (champ « autre réponse ») = le texte tapé devient la valeur ; `response` optionnel pour une réponse libre globale ; 1 à 4 questions, 2 à 4 options, `header` de 12 caractères au plus ; `preview` par option seulement si l'option SDK `toolConfig.askUserQuestion.previewFormat` est réglée (hors prototype 1, voir section 7). Non disponible dans les sous-agents. L'outil n'est offert à Claude qu'avec `--permission-prompt-tool stdio` (absent de `init.tools` sans hôte, présent dans `t5` et `t8`).

**« Ignorer » une question (`t10`, testé).** Capacité `ask_user_question_unanswered_v1` (dans `init.capabilities`) : la réponse peut porter `annotations[<texte de la question>].unanswered` parmi `skipped`, `replied_in_chat`, `dismissed` (`sdk.d.ts`). À n'envoyer qu'à un CLI qui annonce la capacité. Réponse testée :

```json
{"behavior":"allow","updatedInput":{"questions":[…],"answers":{},"annotations":{"Quelle est ta couleur préférée ?":{"unanswered":"skipped"}}}}
```

Claude reçoit `{"type":"tool_result","content":"The user did not answer the questions."}` et le dit dans sa réponse.

**Interrompre (`t6`)**

```json
{"type":"control_request","request_id":"int-2","request":{"subtype":"interrupt"}}
```

Réponse : `{"subtype":"success","request_id":"int-2","response":{"still_queued":[]}}`. Le tour se termine par un `result` `error_during_execution` / `aborted_streaming` et un message `user` « [Request interrupted by user] » ; la session reste utilisable. Option `cancel_queued:true`.

**Changer de modèle (`t5`)**

```json
{"type":"control_request","request_id":"mdl-1","request":{"subtype":"set_model","model":"sonnet"}}
```

Réponse : `{"response":{"model":"claude-sonnet-5-5"}}`. Le tour suivant utilise le nouveau modèle.

**Autres requêtes de contrôle** (définies dans `sdk.d.ts`, non testées) : `set_permission_mode`, `set_max_thinking_tokens`, `stop_task` (arrêter un sous-agent), `get_context_usage`, `get_session_cost`, `mcp_status`, `mcp_toggle`, `rewind_files`, `rename_session`, `reload_plugins`, `reload_skills`, `initialize`, `background_tasks`, `request_user_dialog`, `mcp_message`, `hook_callback`.

### 4.6 Reprise de session

`--resume <session_id>` (aussi un nom ou le chemin d'un `.jsonl`) recharge le contexte (`t7`) et garde le même `session_id` ; `--fork-session` en crée un nouveau ; `--session-id <uuid>` impose l'id. La liste des sessions du prototype 1 se lit dans les transcripts du CLI (voir questions ouvertes).

### 4.7 Skills, agents, hooks, MCP

Chargés en `-p` comme en interactif, sauf avec `--bare` ou `--safe-mode` (doc headless, et `t1` : skills personnels, agents `agent-*` et hook SessionStart présents). Les serveurs MCP apparaissent dans `init.mcp_servers` avec un statut (`pending`, `connected`, `needs-auth`, `failed`) ; la plupart sont `pending` à l'init.

### 4.7bis Requête `initialize` avant le premier message (`t11`)

Envoyée juste après le lancement, sans message utilisateur : `{"type":"control_request","request_id":"init-1","request":{"subtype":"initialize"}}`. Le CLI répond (testé) par un `control_response` `success` dont `response` contient notamment `commands` (108, avec `name`, `description`, `argumentHint`), `agents` (22), `models` (`value`, `resolvedModel`, `displayName`, `supportedEffortLevels`…), `output_style`, `account`, `claude_code_version`, `current_permission_mode`, `session_state`, `capabilities`, `pid`. Aucun `system/init` n'est émis à ce stade. Cela donne la palette de skills et la liste des modèles sans attendre le premier tour. Les champs `account.email` et `account.organization` sont masqués dans la fixture.

### 4.8 Pièges

- **`subagent_type` entouré de U+00A0** : le flux donne `"\u00a0agent-bleu\u00a0"` (octets `C2 A0` autour du nom, relevés avec `od -c` dans `t3` ; la liste `init.agents` a la même forme). La source est le frontmatter des agents (`name: "\u00a0agent-bleu\u00a0"` dans `~/.claude/agents/agent-bleu.md`) : ce ne sont donc pas des espaces ordinaires. Toujours `trim()` avant comparaison ou couleur : `str::trim` de Rust et `String.prototype.trim` de JavaScript retirent U+00A0 (propriété Unicode White_Space). Ne pas utiliser un `trim` ASCII. (Dans l'`input` du `tool_use`, la valeur est `agent-bleu`, sans ces caractères.)
- **`system/init` n'arrive qu'après le premier message, puis à chaque tour** (voir 4.3) : ne pas réinitialiser l'interface à chaque `init`, et ne pas attendre un `init` pour afficher la palette (utiliser `initialize`).
- **`--permission-prompts none` est incompatible avec la décision 7** : il retire `AskUserQuestion` (doc headless) et refuse tout ce qui demanderait une autorisation. Ne pas l'utiliser.
- **Blocs `thinking` vides** : seule `signature` est présente ; rien à afficher.
- **`--forward-subagent-text`** nécessaire pour reconstruire le texte des sous-agents d'avant-plan.
- **Pas de SIGINT sous Windows** : interrompre par le `control_request` `interrupt`, pas par un signal. SIGTERM laisse le tour inachevé (doc headless).
- **Flag non documenté** : `--permission-prompt-tool stdio` n'est ni dans `claude --help` ni dans la doc CLI (celle-ci ne décrit que l'outil MCP) ; il est lu dans le code du SDK et peut changer. Épingler la version du CLI et tester les `capabilities`.
- **Flux verbeux** : une ligne par fragment, y compris les fragments `partial_json` des outils ; lignes pouvant faire plusieurs ko (sortie du hook SessionStart) : lecteur de lignes sans limite fixe.
- **Coût du premier tour** : environ 0,17 $ sur le premier tour (création de cache de 20 000 tokens environ à cause des skills, hooks, MCP) ; `--bare` l'évite mais désactive skills et hooks.
- **Les outils lents du harnais** : un `sleep 40` en avant-plan est refusé par Claude Code, qui le relance en arrière-plan ; à ne pas confondre avec un blocage.

## 5. Choix d'implémentation : CLI brut, pas de SDK

Le SDK TypeScript (`@anthropic-ai/claude-agent-sdk` 0.3.325) n'est qu'une enveloppe : il lance le même binaire avec `--output-format stream-json --verbose --input-format stream-json` et parle le même protocole de contrôle. Il n'apporte rien d'indispensable pour ce périmètre. Il apporte des types, les callbacks `canUseTool` et hooks, des serveurs MCP in-process et la gestion de sessions ; les deux derniers devront être réimplémentés en Rust si le besoin apparaît (notamment pour l'outil MCP « formulaire », étape 2 de la décision 6). `.claude-work/reference/sdk.d.ts` sert de référence des types.

Licence : le paquet est sous « © Anthropic PBC. All rights reserved » avec renvoi aux conditions légales. Pour cette raison, `sdk.d.ts` n'est **pas versionné** : il reste en local dans `.claude-work/reference/sdk.d.ts` (dossier ignoré par git). Pour le retrouver : `npm pack @anthropic-ai/claude-agent-sdk@0.3.325`, puis extraire `package/sdk.d.ts`. Les références `sdk.d.ts:NNNN` de cette spec visent cette version.

## 6. Périmètre du prototype 1 : « utilisable au quotidien »

Inclus :
- conversation au fil des tokens, avec saisie, et défilement virtualisé fluide sur de longues sessions ;
- traitement des permissions (`bypassPermissions` + gestionnaire `can_use_tool` branché sur `--permission-prompt-tool stdio`) ;
- panneau des agents : couleur, état, durée, outil en cours (`task_progress.last_tool_name`) ; **bonus** : fil déroulable au clic ;
- carte minimale en style terminal pour les questions de Claude (`AskUserQuestion`) : liste des options choisies au clavier (↑↓, Entrée) ou à la souris, champ « autre réponse » libre, action « ignorer » (`annotations[…].unanswered`) ; réponse par `updatedInput.answers` ;
- palette de skills (Ctrl+K) alimentée d'abord par la requête `initialize` (ou à défaut une lecture de `~/.claude/skills`, `~/.claude/agents` et des plugins), complétée par `system/init` après le premier message ;
- interruption et changement de modèle ;
- reprise de session sous forme de liste simple.

Exclus : formulaires natifs (l'outil MCP « formulaire » est l'étape 2 de la décision 6), aperçu HTML des options d'`AskUserQuestion` (voir section 7), personnalisations de look au-delà de la fidélité terminal, multi-session simultanée (non décidée).

Le projet versionne `Cargo.lock` (application, pas bibliothèque) ; `target/` et `node_modules/` restent ignorés.

Le gestionnaire de `control_request` répond `allow` par défaut à tout `can_use_tool` qui n'est pas `AskUserQuestion`, et affiche la carte pour `AskUserQuestion` : aucun refus automatique des questions.

## 7. Questions ouvertes (cas non testés)

- Résolu : refus de permission (`behavior:"deny"`), capturé en `t9` (voir 4.5).
- Résolu : sous `bypassPermissions`, le mode est accepté avec `--permission-prompt-tool stdio` et sans `--allow-dangerously-skip-permissions` ; seul `AskUserQuestion` arrive en `can_use_tool` dans le test `t8` (un Bash en écriture locale n'en génère pas). Reste à vérifier : ce qui arrive pour des chemins hors du dossier de travail, `default_to_no`, `safetyCheck`, et les outils MCP `requires_user_interaction`.
- Aperçu HTML des options d'`AskUserQuestion` (retiré du prototype 1) : `toolConfig.askUserQuestion.previewFormat` est une option du SDK ; le chemin exact vers le CLI brut (paramètre d'`initialize` ou autre) n'est pas étudié.
- `stop_task` (arrêt d'un sous-agent), interruption d'un sous-agent d'avant-plan.
- Sous-agents imbriqués et skills forkés : décrits dans la doc, non reproduits.
- Sous-agent d'arrière-plan qui survit au tour avec stdin ouvert : dans `t3` la fin arrive avant le `result`.
- `request_user_dialog`, `hook_callback` et `mcp_message` (transport MCP in-process pour l'outil formulaire).
- Lister les sessions pour la reprise : emplacement exact des transcripts `.jsonl` (la doc renvoie à https://code.claude.com/docs/en/sessions.md#where-transcripts-are-stored), à étudier.
- Format exact de `rate_limit_event` (une seule observation) et des événements non capturés (`api_retry`, `permission_denied`).
- Résolu : les « espaces » des noms d'agents sont des U+00A0 présents dans le frontmatter des agents (voir 4.8).
- Un `/orchestrateur` ou autre skill lancé par l'app apparaît-il comme message `user` développé ? Non examiné en détail (`t5`).
