# Plan d'implémentation : prototype 1 « utilisable au quotidien »

Statut : plan révisé après les décisions 9 à 13 de la spec, puis aligné sur le TUI réel capturé au jalon J1 (commit `64d6717`, section 1.7, `docs/reference-tui/README.md`) et sur le contrat tel que codé au jalon J0 (commit `90f7691`, section 1.5 : le code fait foi). **Avancement : prototype 1 terminé, jalons J0 à J12 faits** ; dernière passe de correctifs `c0f4c07` ; mesures de recette dans `docs/recette-prototype-1.md` ; décisions prises sans l'utilisateur : spec section 8 ; reste à faire : section 4 de ce plan. Commits : J0 (`90f7691`, `6a50634`), J1 (`64d6717`, `0bc9faa`), J2 (`e917ede`, `50deefa`), J3 (`f242437`, `9f5cae3`), J4 (`5c9f2f0`), J5 (`ebe39d9`, `bf3d6cb`), J6 (`5c09322`, `3fc9372`), J7 (`fda656c`, `6b6169d`, `64378f5`), J8 (`a3efe05`), J9 (`a8f5728`, branché par `32869dc`), J10 (`827a73f`), J11 (`4ea516b`, `99513a2`, branché par `b63eb7d`), finitions (`d5bda56`, `82e444c`, `d236af4`), J12 (`e034742`), passe de relecture (`64c3943`, `e5d2eef`, `4c41fd9`, `bafea38`, `c0f4c07`) (voir les jalons en section 2 pour les écarts). La spec fait foi : `docs/spec.md` (décisions en section 2, protocole en section 4, périmètre en section 6). Ce plan ne redit pas le protocole : il renvoie aux sections de la spec et aux fixtures `fixtures/stream-json/t1` à `t12`.

Vérifications faites sur la machine pour ce plan (lecture seule, 2026-11-16) :

- outils installés : `rustc 1.94.1`, `cargo 1.94.1`, `node v24.4.1`, `npm 11.7.0` ; `pnpm` absent ; `claude` dans `C:\Users\<user>\.local\bin\claude` (2.1.325 d'après `t8`, `t11`) ;
- transcripts : `C:\Users\<user>\.claude\projects\H--<user>-Code-orchestrator\<session_id>.jsonl` existe (voir section 1.8) ;
- `initialize` (`t11`) annonce 6 `capabilities`, alors que `system/init` (`t8`) en annonce 19 ; `ask_user_question_unanswered_v1` n'est **que** dans `system/init` (voir section 1.9) ;
- `--resume` ne réémet pas l'historique sur stdout (`t7` : `hook_*`, `init`, puis directement la réponse au nouveau message) : l'app doit relire le transcript pour afficher les tours passés ;
- aucune fixture ne contient d'appel `Edit` : le rendu des diffs demande une nouvelle capture (`t13`, jalon J1). `t12` existe déjà : lancement sans message, qui ne contient que des `hook_*` (aucun `system/init`) ; les nouvelles captures commencent donc à `t13`.

---

## 1. Démarrage technique

### 1.1 Arborescence du dépôt

```
orchestrator/
├─ docs/
│  ├─ spec.md                      spécification (fait foi)
│  ├─ plan-prototype-1.md          ce plan
│  └─ reference-tui/               README : liste des captures du TUI, méthode, liste de contrôle (PNG locaux non versionnés)
├─ fixtures/
│  ├─ stream-json/                 captures brutes du CLI (t1, t3..t12 existantes, + t13 Edit, t14 erreurs, t15 longue session)
│  ├─ ui-events/                   événements normalisés générés par `replay` (entrée des tests de l'interface)
│  └─ transcripts/                 extraits anonymisés de ~/.claude/projects/... (tests de la reprise)
├─ package.json  package-lock.json  vite.config.ts  svelte.config.js  tsconfig.json  index.html
├─ src/                            interface Svelte 5
│  ├─ main.ts  App.svelte  app.css
│  └─ lib/
│     ├─ core/                     client.ts (invoke + Channel), mock.ts (rejoue fixtures/ui-events), bindings/ (types générés depuis Rust, versionnés)
│     ├─ state/                    session.svelte.ts, agents.svelte.ts, palette.svelte.ts, sessions.svelte.ts
│     ├─ render/                   markdown.ts, highlight.ts, diff.ts, glyphs.ts, colors.ts, truncate.ts
│     └─ components/               Conversation, rows/*, Prompt, StatusLine, AgentPanel, QuestionCard, Palette, ProjectPicker, SessionPicker, Banner
├─ e2e/                            playwright.config.ts ; Playwright : scroll, collage, latence, rendu (sur Vite + IPC simulé)
│  └─ unit/                        tests Vitest de l'interface (*.test.ts, `npm test`)
├─ tools/capture-tui/              capture.ps1 (lance le TUI dans Windows Terminal et capture la fenêtre), scenarios.json (décision 10)
└─ src-tauri/
   ├─ Cargo.toml  Cargo.lock  build.rs  tauri.conf.json  capabilities/default.json  icons/
   ├─ src/
   │  ├─ main.rs  lib.rs
   │  ├─ api/                      commands.rs (commandes Tauri), events.rs (CoreEvent), types.rs  ← le contrat
   │  ├─ process/                  locate.rs (claude.exe), spawn.rs (args, Job Object, sans console), reader.rs, writer.rs, supervisor.rs
   │  ├─ protocol/                 inbound.rs (enums serde du flux), outbound.rs (user, control_request/response), control.rs (corrélation)
   │  ├─ state/                    reducer.rs (flux brut → CoreEvent), agents.rs, pending.rs, threads.rs, batcher.rs
   │  ├─ sessions/                 paths.rs (encodage du dossier), list.rs, transcript.rs (historique → CoreEvent)
   │  ├─ colors.rs  errors.rs  logging.rs
   │  └─ bin/replay.rs             fixture .jsonl → fixtures/ui-events/*.json (outil de dev et de test)
   └─ tests/                       fixtures_replay.rs, transcripts.rs, snapshots/ (insta)
```

Zones indépendantes une fois le contrat figé (jalon J0) : `src-tauri/src/{process,protocol,state}` (cœur), `src-tauri/src/sessions` (reprise), `src/lib/{render,components}` (rendu), `src/lib/state` (magasins). Le seul fichier partagé est le contrat : `src-tauri/src/api/*.rs` et sa projection générée `src/lib/core/bindings/`.

### 1.2 Versions et gestionnaire de paquets

| Élément | Version visée | Remarque |
| - | - | - |
| Rust | 1.94.1 (installé), édition 2021, `src-tauri/rust-toolchain.toml` épinglé | cible `x86_64-pc-windows-msvc` |
| Tauri | 2.x, dernière mineure stable au jalon J0 ; `@tauri-apps/cli` et `@tauri-apps/api` de la même mineure | WebView2 fourni par Windows 11 |
| Svelte | 5.x (runes), avec Vite et `@sveltejs/vite-plugin-svelte` ; **pas de SvelteKit** | une seule fenêtre, pas de routage |
| Node | 24.4.1 (installé, LTS), pour le build seulement | `"engines"` dans `package.json` |
| Gestionnaire | **npm 11** (installé) avec `package-lock.json` versionné | alternative : pnpm (plus rapide, mais à installer) |
| claude.exe | 2.1.325 épinglé (spec 4.8) | autre version : avertissement, voir 1.9 |

Les numéros exacts des dépendances sont fixés au jalon J0 (dernières stables du jour) et figés par `Cargo.lock` et `package-lock.json`.

### 1.3 Commandes

| But | Commande |
| - | - |
| Installer les dépendances front | `npm ci` |
| Développement (Vite + fenêtre Tauri, rechargement) | `npm run tauri dev` |
| Interface seule sur données rejouées (sans Rust) | `npm run dev:mock` (Vite, `?mock=t3` choisit la fixture) |
| Build de l'installeur | `npm run tauri build` (MSI/NSIS dans `src-tauri/target/release/bundle/`) |
| Tests Rust | `cargo test --manifest-path src-tauri/Cargo.toml` |
| Revue des instantanés | `cargo insta review --manifest-path src-tauri/Cargo.toml` |
| Régénérer les événements d'interface | `cargo run --manifest-path src-tauri/Cargo.toml --bin replay -- fixtures/stream-json/t3.jsonl > fixtures/ui-events/t3.json` (script `npm run fixtures`) |
| Régénérer les types TS | `cargo test --manifest-path src-tauri/Cargo.toml export_bindings` (ts-rs) |
| Tests de l'interface | `npm test` (Vitest) |
| Mesures de recette | `npm run e2e` (Playwright, Chromium ≈ moteur de WebView2) |
| Qualité | `cargo clippy -- -D warnings`, `cargo fmt --check`, `npm run check` (svelte-check), `npm run lint` |

### 1.4 Dépendances clés

| Besoin | Choix | Pourquoi | Alternative |
| - | - | - | - |
| Processus et E/S | **tokio** (`process`, `io-util`, `sync`, `time`) | Tauri tourne déjà sur tokio (`tauri::async_runtime`) ; lecture stdout, stderr, écriture stdin et minuteries dans une seule boucle `select!` ; `kill_on_drop` | `std::process` + 3 threads et `crossbeam-channel` : plus simple à lire, mais minuteries et annulation à la main |
| Mort de l'arbre de processus | **Job Object** `KILL_ON_JOB_CLOSE` via `windows-sys` | si l'app plante, `claude.exe` et ses enfants (bash, serveurs MCP) meurent aussi | crate `win32job` (plus court, une dépendance de plus) |
| Lancement sans console | `CREATE_NO_WINDOW` (`std::os::windows::process::CommandExt`) | évite une fenêtre noire à chaque session | aucune utile |
| JSON | **serde** + **serde_json** ; enums `#[serde(tag = "type")]` avec variante de repli `Unknown(serde_json::Value)` | typage fort sans casser sur un événement inconnu (spec 4.3 en liste plusieurs non capturés) | `serde_json::Value` partout : tolérant mais sans aide du compilateur |
| Types partagés Rust → TS | **ts-rs** | génère `src/lib/core/bindings/*.ts` depuis `api/` ; le contrat ne peut pas diverger | `tauri-specta` (génère aussi les appels, mais encore en RC en v2 à ma connaissance, à vérifier) |
| Erreurs, journaux | `thiserror`, `tracing` + `tracing-appender` | erreurs typées vers l'interface ; journal tournant dans `%LOCALAPPDATA%` | `anyhow` + `log`/`env_logger` |
| Identifiants | `uuid` (v4) | `request_id` des `control_request` sortants | compteur préfixé (`int-1`, comme les fixtures) |
| Dossiers | `dirs` | `home_dir()` pour `~/.claude/projects` | `std::env::var("USERPROFILE")` |
| Tests Rust | `insta` (instantanés JSON), `pretty_assertions` | la sortie normalisée d'une fixture se relit en diff | assertions écrites à la main |
| Liste virtualisée | **virtua** (`virtua/svelte`) | hauteurs variables mesurées, maintien de position quand on ajoute en bas ou quand une ligne grandit (cas du streaming), API Svelte 5 | `@tanstack/virtual-core` avec un composant maison de 150 lignes (plus de contrôle, plus de code) |
| Markdown | **marked** (+ renderer maison sans HTML brut) | rapide, synchrone, extensible ; on ne réanalyse que le bloc en cours | `markdown-it` (plus strict CommonMark, un peu plus lent) |
| Assainissement | **DOMPurify** sur la sortie HTML | du texte de modèle injecté en `{@html}` | renderer qui échappe tout et n'émet que des balises connues (sans dépendance, plus de code) |
| Coloration du code | **highlight.js** (noyau + ~25 langages déclarés) | synchrone, léger, thème ANSI facile à calquer sur le TUI | **shiki** (grammaires VS Code, plus fidèle mais asynchrone et ~1 Mo de WASM/grammaires) |
| Diffs d'`Edit` | **diff** (jsdiff, `structuredPatch`) côté interface | calcul à partir de `input.old_string` / `new_string`, sans attendre le format de `tool_use_result` | crate `similar` côté Rust (diff envoyé déjà calculé) |
| Police mono | **Cascadia Mono**, fichiers `.woff2` embarqués (licence OFL) | police par défaut de Windows Terminal, donc celle des captures de référence ; embarquée pour un rendu identique sur toute machine | JetBrains Mono (OFL) ; ou police système seule `Consolas` (rien à embarquer, rendu différent du terminal) |
| Tests de l'interface | **Vitest** + `@testing-library/svelte` + `@tauri-apps/api/mocks` | composants et magasins testés sans fenêtre | Playwright component testing |
| Mesures | **Playwright** (Chromium) | trace de performance, FPS, mémoire JS, captures | mesures à la main dans les DevTools de WebView2 |

### 1.5 Contrat entre le cœur Rust et l'interface

Principes :

- Le cœur **normalise** : l'interface ne voit jamais le JSON brut du CLI. Elle reçoit des `CoreEvent` déjà corrélés (fil, bloc, agent, requête).
- Les événements passent par un **`tauri::ipc::Channel<Vec<CoreEvent>>`** ouvert par `start_session` : ordre garanti, plus rapide que `emit`. Le cœur **regroupe** les événements par lot : un lot part dès qu'il y a 16 ms d'accumulation ou 256 événements, et immédiatement au bord avant d'une rafale : aucun lot depuis 16 ms, ou événement précédent à 5 ms ou plus (ajout de la recette J12, `4c41fd9` : à 100 `text_delta`/s, chaque delta part à son arrivée). L'écho d'un message tapé (`user_message` `Typed`) part toujours tout de suite.
- Chaque événement porte `seq: u64` (ordre, détection de trou) et `t: u64` (ms depuis l'epoch à la lecture de la ligne, pour mesurer la latence).
- Prototype 1 : une session à la fois (spec section 6), mais chaque commande et événement porte `session: SessionKey` (identifiant local à l'app) pour ne pas casser le contrat le jour où le multi-session arrivera.
- Les noms ci-dessous sont en Rust (`snake_case`) ; ts-rs produit les types TS équivalents.
- **Règles d'état** (codées en J0, `reducer.rs` et `supervisor.rs`) : `Interrupting` n'est émis que pendant un tour (`Requesting` ou `Streaming`), jamais au repos ; `Exited` est terminal : `set_state` ignore tout changement après lui, donc plus aucun `Interrupting` ni retour en arrière.
- **Capabilities** : les commandes sont restreintes par capability (`build.rs`, `AppManifest` ; `src-tauri/capabilities/default.json`). Aujourd'hui seules `allow-get-launch-info`, `allow-start-session` et `allow-send-message` y sont. **Tout jalon qui appelle depuis l'interface une commande nouvelle doit ajouter `allow-<commande>` (tirets) dans `default.json`**, sinon l'appel est refusé. Les fichiers `src-tauri/permissions/autogenerated/` sont ignorés par git et régénérés à chaque build (J2).
- **Écarts de J0 intégrés** (le code de `src-tauri/src/api/` fait foi, il est testé) : `get_launch_info` remplace `get_launch_dir` ; `ExitInfo` porte `killed` ; l'événement `init` porte `version` ; `turn_result` porte `result` (texte final) ; le champ `kind` de `rate_limit` s'appelle `limit_kind` ; la notice de version a le code `unexpected_version` et vient du réducteur ; `skip_question` émet pour l'instant `question_closed { Answered }` ; `rust-toolchain.toml` est dans `src-tauri/`.

#### Commandes Tauri

| Commande | Signature | Effet côté CLI | Erreurs |
| - | - | - | - |
| `start_session` | `(cwd: String, resume: Option<String>, model: Option<String>, events: Channel<Vec<CoreEvent>>) -> Result<SessionStarted, CoreError>` ; `SessionStarted { session: SessionKey, pid: u32, claude_path: String }` | lance la commande de la spec 4.1 (+ `--resume`, `--model`), puis envoie `initialize` (spec 4.7bis) ; **`resume` n'accepte que l'UUID** de session (règle de l'app, `validate_launch_options` ; le CLI accepte aussi un nom ou un chemin `.jsonl`, voir spec 4.6) | `ClaudeNotFound`, `SpawnFailed`, `BadCwd`, `InvalidArgument` (`resume` qui n'est pas un UUID, `model` vide, commençant par `-` ou avec espace ou caractère de contrôle) |
| `send_message` | `(session, text: String) -> Result<(), CoreError>` | écrit `{"type":"user",…}` (spec 4.2) ; les `/skill args` passent tels quels | `NotRunning`, `QuestionPending` |
| `interrupt` | `(session, cancel_queued: bool) -> Result<InterruptAck, CoreError>` ; `InterruptAck { still_queued: Vec<String> }` | `control_request` `interrupt` (spec 4.5, `t6`) ; délai 5 s | `Timeout`, `NotRunning` |
| `set_model` | `(session, model: String) -> Result<String, CoreError>` (modèle résolu, ex. `claude-sonnet-5-5`) | `control_request` `set_model` (`t5`) | `Timeout`, `Rejected(msg)` |
| `answer_question` | `(session, request_id: String, answers: BTreeMap<String, Answer>, response: Option<String>) -> Result<(), CoreError>` ; `Answer = One(String) \| Many(Vec<String>)` | `control_response` `allow` + `updatedInput { questions, answers }` (`t4`) | `UnknownRequest` |
| `skip_question` | `(session, request_id: String, reason: SkipReason) -> Result<(), CoreError>` ; `SkipReason = Skipped \| ReplyInChat \| Dismissed` (sérialisés `skipped`, `replied_in_chat`, `dismissed`) | `answers:{}` + `annotations[q].unanswered` (`t10`) ; refusé si la capacité manque ; émet `question_closed { Skipped }` depuis J8 | `Unsupported` |
| `list_sessions` | `(cwd: String) -> Result<Vec<SessionSummary>, CoreError>` ; `SessionSummary { id, title: Option<String>, last_prompt: Option<String>, modified: u64, size_bytes: u64, entrypoint: Option<String> }` | aucun (lecture de fichiers, section 1.8) | `Io` |
| `load_history` | `(session, session_id: String) -> Result<(), CoreError>` | aucun ; relit le transcript et pousse des `CoreEvent` marqués `history: true` dans le canal | `Io`, `TranscriptFormat` |
| `stop_session` | `(session) -> Result<ExitInfo, CoreError>` ; `ExitInfo { code: Option<i32>, killed: bool }` (`killed` : tué par le Job Object faute de sortie dans le délai) | ferme stdin, attend 3 s, puis tue le Job Object | — |
| `get_launch_info` | `() -> LaunchInfo` ; `LaunchInfo { cwd: String, claude_path: Option<String>, expected_version: String }` ; `cwd` = premier argument de l'app (`<app> .`, `<app> <chemin>`), sinon dossier courant dans le code de J0 | aucun (décision 11) | — |
| `list_projects` (livré en J11) | `() -> Result<Vec<ProjectSummary>, CoreError>` ; `ProjectSummary { encoded: String, path: Option<String>, exists: bool, last_used: u64, session_count: u32 }` | aucun ; lit `~/.claude/projects` (sections 1.8 et décision 11) | `Io` |
| `get_diagnostics` | `() -> Diagnostics` ; `Diagnostics { claude_version, expected_version, claude_path, capabilities, log_path, stderr_tail }` ; J6 ajoute `unknown_count` et renseigne `log_path` | aucun | — |

Ouvrir l'app = `get_launch_info` ; si aucun dossier n'a été passé en argument, l'interface montre le sélecteur de `list_projects`, puis appelle `start_session` sur le dossier choisi. Dans le code de J0, `cwd` retombe sur le dossier courant sans argument : J11 ajoute à `LaunchInfo` l'information « dossier passé en argument ou non » pour savoir quand ouvrir le sélecteur (décision 11). Reprendre une session = `stop_session` puis `start_session(cwd, Some(id), …)` puis `load_history`. Le cœur garde l'id local (`SessionKey`) inchangé côté interface.

#### Événements `CoreEvent` (tag `kind`)

`ThreadId` vaut `"main"` ou l'id du `tool_use` Agent (`parent_tool_use_id`, spec 4.4). `MsgKey` = `message.id` de l'API (`msg_…`), présent dans `stream_event/message_start` et dans `assistant`.

| `kind` | Champs | Source dans le flux |
| - | - | - |
| `starting` | `pid`, `cwd`, `args` | spawn |
| `ready` | `version`, `capabilities`, `commands[] {name, description, argument_hint}`, `agents[]`, `models[] {value, resolved, display_name}`, `permission_mode` | réponse à `initialize` (`t11`) |
| `init` | `session_id`, `version`, `model`, `permission_mode`, `capabilities`, `tools`, `mcp_servers[] {name, status}`, `skills`, `slash_commands` | `system/init`, à chaque tour : l'interface **fusionne**, elle ne réinitialise pas (spec 4.8) |
| `status` | `state: Idle \| Requesting \| Streaming \| Interrupting \| Exited` | `system/status`, `stream_event`, `result`, sortie du processus |
| `user_message` | `thread`, `uuid?`, `text`, `origin: Typed \| Replay \| Interrupted \| History` | stdin envoyé, `user` texte, « [Request interrupted by user] » |
| `block_start` | `thread`, `msg`, `index`, `block: Text \| Thinking \| ToolUse { tool_use_id, name }` | `content_block_start` |
| `text_delta` | `thread`, `msg`, `index`, `text` | `content_block_delta/text_delta` (les `input_json_delta` restent dans le cœur) |
| `block_done` | `thread`, `msg`, `index`, `block: Text { text } \| Thinking { empty: bool } \| ToolUse { tool_use_id, name, input }` | message `assistant` complet : **fait foi** et remplace le texte accumulé |
| `tool_result` | `thread`, `tool_use_id`, `content: Vec<ResultPart>`, `is_error`, `meta: Option<Value>` (`tool_use_result` brut) | `user` avec `tool_result` |
| `agent` | `tool_use_id`, `task_id?`, `agent_type` (après `trim()`), `color`, `description`, `status: Starting \| Running \| Completed \| Failed \| Stopped`, `background`, `started_at`, `ended_at?`, `last_tool?`, `tokens?`, `tool_uses?`, `summary?` | `tool_use` Agent/Task, `task_started`, `task_progress`, `task_updated`, `task_notification`, `background_tasks_changed` |
| `question` | `request_id`, `tool_use_id`, `questions[] {question, header, options[] {label, description}, multi_select}`, `can_skip` | `can_use_tool` `AskUserQuestion` |
| `question_closed` | `request_id`, `reason: Answered \| Cancelled` (J8 ajoute `Skipped`) | notre réponse ou `control_cancel_request` |
| `turn_result` | `subtype`, `is_error`, `result` (texte final), `terminal_reason`, `duration_ms`, `cost_usd`, `num_turns`, `context_used?`, `context_window?`, `permission_denials` | `result` |
| `rate_limit` | `status`, `resets_at`, `limit_kind`, `utilization?` | `rate_limit_event` |
| `notice` | `level: Info \| Warn \| Error`, `code`, `message`, `detail?` | version inattendue (`unexpected_version`, émise par le réducteur, une fois, sur `ready` ou `init`), échec de hook, JSON illisible ou trop longue, requête de contrôle en échec |
| `stderr` | `line` | stderr du processus |
| `exited` | `code: Option<i32>`, `during_turn: bool`, `stderr_tail: Vec<String>` | fin du processus |

Les `can_use_tool` autres qu'`AskUserQuestion` reçoivent `allow` + `updatedInput = input` directement dans le cœur (décision 7, spec 6) et produisent un `notice` de niveau `Info` (pour voir ce qui passe sous `bypassPermissions`).

### 1.6 Modèle d'état

**Côté Rust** (`state/`), le cœur ne garde **que ce qui sert à corréler**, pas l'historique (l'interface le possède ; éviter un double stockage de sessions longues) :

```
Session {
  key, session_id: Option<String>, cwd, model, version, capabilities: HashSet<String>,
  proc: Starting | Running { pid } | Exited { code },
  turn: Idle | Requesting | Streaming | Interrupting,
  open_blocks: HashMap<(ThreadId, MsgKey, u32), OpenBlock { kind, text_len, partial_json: String }>,
  agents: IndexMap<ToolUseId, Agent>          // registre, clé = tool_use_id du tool_use Agent/Task
  task_to_tool: HashMap<TaskId, ToolUseId>,   // task_started relie task_id et tool_use_id
  pending_in:  HashMap<RequestId, Incoming { tool_use_id, input, questions }>,   // can_use_tool en attente
  pending_out: HashMap<RequestId, Outgoing { subtype, reply: oneshot::Sender<Result<Value>>, deadline }>,
  stderr_tail: VecDeque<String> (200 lignes), seq: u64,
}
```

- Le réducteur est une **fonction pure** `fn reduce(&mut Session, Inbound) -> Vec<CoreEvent>` (aucune E/S), ce qui permet de rejouer les fixtures en test sans processus.
- Les réponses automatiques (`allow`) sont des `Effect` renvoyés par le réducteur et exécutées par le superviseur.

**Côté Svelte** (`src/lib/state/`), un magasin par session, classe avec runes :

```
SessionStore {
  rows: $state.raw<Row[]>          // fil principal à plat, une ligne virtualisée par Row
  threads: Map<ToolUseId, Row[]>   // fils des sous-agents (bonus : déroulés dans le panneau)
  live: $state<Row | null>         // la ligne en cours de streaming, seule réactive en profondeur
  byToolUse: Map<ToolUseId, rowIndex>   // pour rattacher tool_result et agent à leur appel
  agents: $state<Agent[]>, question: $state<Question | null>,
  meta: $state<{ version, model, models, commands, capabilities, mcp }>,
  status: $state<Status>, banners: $state<Notice[]>
}
Row = User | AssistantText | ToolCall (+ résultat attaché) | AgentCall | TurnResult | Notice | Interrupted
```

- `rows` est en `$state.raw` : on remplace le tableau (ou la dernière case) au lieu de rendre réactif chaque objet ; c'est ce qui garde le coût constant sur une longue session. Seule `live` est réactive en profondeur.
- Les lots du canal sont appliqués dans **un seul `requestAnimationFrame`** par image.
- Un message de sous-agent (`thread != "main"`) va dans `threads`, jamais dans `rows`. Dans le fil principal, l'appel Agent est une seule ligne mise à jour par les événements `agent` (comme le TUI).

### 1.7 Règles de rendu « style terminal »

**Référence de fidélité** : le TUI de Claude Code 2.1.325 dans Windows Terminal (120 × 40, Cascadia Mono 12 pt, jeu Campbell, thème `dark`, rendu `fullscreen`), capturé par script au jalon J1 (décision 10). La **vue normale** fait foi ; la vue détaillée (Ctrl+O) est reproduite aussi, au jalon J5. Liste des captures, emplacement local (non versionné), commande pour les refaire et liste de contrôle : `docs/reference-tui/README.md`. Chaque règle du tableau cite la capture qui la prouve (`NN-nom.txt` = transcription annotée des couleurs ; `NN-nom.uia.txt` = texte exact lu à l'écran).

La recette compare côte à côte la capture et l'app sur la même situation, en 120 colonnes. Pas de comparaison pixel à pixel (polices et anticrénelage diffèrent) : on suit la liste de contrôle (glyphes, couleurs, retraits, regroupements).

| Élément | Rendu | Preuve |
| - | - | - |
| Grille | police mono, `line-height` fixe, largeur en colonnes (`ch`) ; fond `#0C0C0C` ; aucune bulle, carte ni ombre (décision 3) | toutes |
| Message utilisateur | `❯` gris, puis texte **blanc gras sur une bande gris foncé** (≈ `#373737`) pleine largeur ; lignes suivantes en retrait de 2 ; le markdown du message est rendu | `02-markdown-code.txt` |
| Texte de l'assistant | puce **`●` (U+25CF) blanche** en colonne 0, texte en retrait de 2 | `02-markdown-code.txt` |
| Markdown | titres en gras blanc, **même taille**, sans `#` ; listes `-` et `1.` telles quelles ; gras, italique ; code en ligne **bleu lavande** (≈ `#B1B9F9`), sans fond ; blocs de code **sans cadre ni fond**, retrait de 2, colorés (mots-clés bleus, fonctions jaunes, chaînes rouges, nombres verts) | `02-markdown-code.txt` |
| Fin de tour | « ✻ <Verbe> for <durée> · done <H:MM> » en gris, heure sans zéro initial (« done 8:08 », `w1`) (verbe tiré au hasard : Crunched, Worked, Churned…) | `02-markdown-code.txt`, `06-agents-termines.txt` |
| Indicateur pendant le tour | « <glyphe animé> <Verbe>… (<durée> · ↓ <N> tokens) » au-dessus de la saisie (glyphe observé : `✽`, il change pendant l'animation) | `k1-tour-en-cours-saisie-vide.txt` |
| Appels d'outils, vue normale | les appels de lecture, de recherche et de commande sont **regroupés en une ligne grise**, en retrait de 2, sans puce ni résultat : « Read 1 file, ran 1 shell command », « Searched for 1 pattern, read 1 file », « Ran 2 shell commands » ; une commande en cours s'affiche avec son libellé et sa durée (« Running ping for 90 requests to localhost · 3s ») puis `  ⎿  $ <commande>` | `03-outils-tronque.txt`, `04-edit-diff.txt`, `09-erreurs.txt`, `k1-tour-en-cours-saisie-vide.txt` |
| Appels qui restent sur leur ligne | Edit : puce `●` **verte** + « Update(<chemin relatif>) », « Update » en gras ; puce **rouge** (≈ `#FF6B80`) en erreur. **Write** (nouveau fichier, capturé au J13 sur le CLI 2.1.326) : puce `●` verte + « Write(<chemin relatif>) », « Write » en gras, chemin souligné en pointillé ; `⎿  Wrote <N> lines to <chemin>` (« Wrote » et « lines to » en couleur du texte, nombre et chemin en gras blanc vif, chemin souligné en pointillé) ; puis le contenu, numéro **gris foncé** (`#666666`) aligné à droite sur (chiffres + 1) colonnes après le retrait de 5, une espace, la ligne en couleur du texte ; vue normale : **10 lignes** puis « … +N lines » gris **sans** « (ctrl+o to expand) » ; vue détaillée : chemin absolu, tout le contenu, pas de « Allowed by auto mode » | `04-edit-diff.txt`, `09-erreurs.txt`, `w1-write-normal.txt`, `w1b-write-detail-ctrl-o.txt` |
| Ligne de résultat | `  ⎿  ` (2 espaces, `⎿`, 2 espaces) en gris, puis le résumé | `04-edit-diff.txt`, `08-interruption.txt` |
| Vue détaillée (Ctrl+O) | bascule toute la conversation ; `03b` montre la vue entière : un appel par ligne et un seul (`● Read(<chemin absolu>)` puis `⎿  Read 8 lines`, `● Bash(<commande>)` puis sa sortie complète sous `⎿`, puis `⎿  Allowed by auto mode`), heure et modèle alignés à droite avant la réponse (`01:13 PM claude-haiku-5-5`) ; chemin absolu aussi pour Update (`09b`) ; barre du bas « Showing detailed transcript · ctrl+o to toggle · … » ; Ctrl+O revient à la vue normale | `03b-outils-detail-ctrl-o.txt`, `09b-erreurs-detail-ctrl-o.txt` |
| Diff d'`Edit` | « ⎿  Added N lines, removed M lines » (nombres en gras) ; numéros alignés à droite sur 6 colonnes, puis espace, `-` ou `+` ; suppressions sur fond rouge sombre, **numérotées dans l'ancien fichier** ; ajouts sur fond vert sombre, **numérotés dans le nouveau** ; 3 lignes de contexte avant et après, en coloration syntaxique ; calcul depuis `old_string`/`new_string` (forme de `tool_use_result` dans `t13`) | `04-edit-diff.txt` |
| Thinking | blocs vides (signature seule, spec 4.8) : **rien** dans le fil | — |
| Erreurs | outil sur sa ligne : `●` rouge, « ⎿  Error editing file » en rouge ; en vue normale, les Bash en erreur restent dans la ligne regroupée ; en vue détaillée, « ⎿  Error: Exit code N » et le stderr en rouge ; `notice` Warn/Error de l'app : bandeau en tête de la conversation | `09-erreurs.txt`, `09b-erreurs-detail-ctrl-o.txt` |
| Interruption | « No result recorded for N call », « ⎿  $ <commande> », « ⎿  Interrupted · What should Claude do instead? », **en gris**, pas en rouge ; même rendu par Échap et par Ctrl+C | `08-interruption.txt`, `c4-ctrlc-saisie-non-vide-pendant-tour.txt`, `k2-ctrlc-saisie-vide-pendant-tour.txt` |
| Sous-agents (fil principal) | « ● N background agents launched (↓ to manage) » (puce verte), puis une sous-ligne par agent, `├` / `└` gris en retrait de 3, **nom sur fond de sa couleur** (texte noir), « (description) » en blanc ; « ✻ Waiting for N background agents to finish » ; au retour, « › Message from @<agent> (ctrl+o to expand) » gris et « ● Agent "<description>" finished · <durée> » ; **pas** de « Done (N tool uses…) » | `05-agents-en-cours.txt`, `06-agents-termines.txt` |
| Panneau des agents | dans le TUI, sous la ligne d'état : « ● main » en gras, puis « ◯ <agent>  <description> … <durée> · ↓ <tokens> tokens » en gris, durée vivante ; il disparaît quand tout est fini. Dans l'app, la **colonne de droite** (décision 4) reprend ce style, avec la couleur de l'agent (`trim()` U+00A0) et `last_tool` en plus ; agent inconnu en gris ; bonus : clic → fil de l'agent | `05-agents-en-cours.txt`, `06-agents-termines.txt` |
| Carte `AskUserQuestion` | **pas d'encadré** : règle `─` grise, pastille « ☐ <header> » (texte noir sur bleu lavande), question en gras, options `❯ 1. <label>` (sélection en bleu lavande, autres en blanc) avec description grise en retrait de 5, puis « N. Type something. » (réponse libre), règle, « N+1. Chat about this » ; pied gris « Enter to select · ↑/↓ to navigate · Esc to cancel » ; la carte prend la place de la saisie | `07-question.txt` |
| Saisie | `❯` entre deux règles `─` grises pleine largeur, collée en bas ; indication « Esc again to clear » à droite au-dessus de la saisie après un premier Échap, sans rien effacer ; second Échap à moins d'environ 0,8 s : la saisie est effacée (`a000` à `a800`, `n000` à `n800`) | `01-accueil.txt`, `n000-echap-echap-0ms-saisie-non-vide.txt` à `n800-…` |
| Menu `/` (palette) | **au-dessus** de la saisie, sans cadre ; entrée choisie `❯ /nom` et sa description en bleu lavande, autres en gris ; descriptions alignées en colonne, repliées puis tronquées par `…` | `10-menu-slash.txt` |

Hors référence : la ligne d'état personnalisée du poste de capture (emojis, `statusLine`), le chemin du projet sous le logo et « ⏵⏵ auto mode on » (les captures tournaient en mode `auto`). Le menu Rewind, ouvert par deux Échap sur une saisie vide, après un tour normal comme après un tour interrompu (`v1`, `v4`), et refermé par un troisième appui (`v2`, `v5`), est hors du prototype 1.

Rendu en flux : pendant le streaming, la ligne `live` réanalyse son markdown au plus 10 fois par seconde (texte brut ajouté entre deux analyses) ; au `block_done`, analyse finale puis HTML mis en cache par `(msg, index)`.

### 1.8 Transcripts pour la reprise de session

Constaté en lecture seule le 2026-11-16 :

- Dossier : `C:\Users\<user>\.claude\projects\<cwd encodé>\` ; encodage = chaque caractère hors `[A-Za-z0-9]` remplacé par `-`. Exemple réel : `H:\<user>\Code\orchestrator` → `H--<user>-Code-orchestrator`.
- Un fichier `<session_id>.jsonl` par session (ex. `<session>.jsonl`, 2 Mo), et un dossier `<session_id>/` avec `subagents/agent-<id>.jsonl` + `agent-<id>.meta.json` (`agentType`, `description`, `toolUseId`) et `tool-results/`.
- Les sessions lancées en `-p` y sont aussi : le dossier de la session de test des fixtures contient `11111111-2222-4333-8444-555555555555.jsonl` (l'id imposé par `--session-id` dans `t5`), `entrypoint: "sdk-cli"`.
- Types de lignes rencontrés : `user`, `assistant`, `system`, `attachment`, `last-prompt` (`lastPrompt`), `ai-title` (`aiTitle`), `queue-operation`, `file-history-snapshot`, `mode`, `permission-mode`, et d'autres internes. Les lignes `user`/`assistant` ont `uuid`, `parentUuid`, `isSidechain`, `timestamp`, `message` (même forme que le flux).
- Une session en `-p` n'a pas d'`ai-title` (constaté) : le titre retombe sur le dernier `lastPrompt`.

Règles (décisions 9 et 11 de la spec) :

- `list_projects` liste les sous-dossiers de `~/.claude/projects`, triés par date du transcript le plus récent. L'encodage n'est pas réversible (`H:\a-b` et `H:\a\b` donnent le même nom) : le chemin réel est lu dans le champ `cwd` des premières lignes `user`/`assistant` du transcript le plus récent ; un dossier qui n'existe plus est grisé dans le sélecteur.
- `list_sessions` lit le **dossier** (date et taille sans ouvrir), puis seulement la **fin** de chaque fichier (derniers 64 ko) pour `ai-title` et `last-prompt` ; tri par date décroissante ; 200 au plus.
- `load_history` lit les lignes `user`/`assistant` avec `isSidechain: false`, et les passe dans le **même réducteur** que le flux (marquées `history`) : un seul code de rendu.
- **Tout l'historique** est affiché à la reprise (décision 9), y compris les appels d'outils et leurs résultats ; les sous-agents passés sont reconstruits depuis `<session_id>/subagents/*.meta.json` (type, description, `toolUseId`) pour le panneau, sans relire leurs fils (bonus).
- Le lecteur est **isolé** dans `src-tauri/src/sessions/transcript.rs`, sans dépendance vers le reste du cœur sauf le réducteur. Le format est **interne et non documenté** : lecture tolérante (ligne illisible ou type inconnu ignoré et compté, jamais bloquant), tests sur des extraits anonymisés dans `fixtures/transcripts/`, et repli sur « reprise sans historique affiché » avec un `notice` si la lecture échoue.
- **À revérifier à chaque nouvelle version du CLI** : `cargo test transcripts` sur les extraits, plus un extrait frais de la nouvelle version ajouté dans `fixtures/transcripts/<version>/`.

### 1.9 Gestion des erreurs

| Cas | Détection | Comportement |
| - | - | - |
| `claude.exe` introuvable | `locate.rs` : variable `ORCH_CLAUDE_PATH`, puis `%USERPROFILE%\.local\bin\claude.exe`, puis `PATH` | écran d'accueil avec le chemin cherché et un bouton « choisir l'exécutable » |
| Échec de démarrage | erreur de spawn, ou sortie avant la réponse à `initialize`, ou pas de réponse en 20 s | `start_session` renvoie `SpawnFailed` avec les dernières lignes de stderr ; bouton « réessayer » |
| Mort en cours de session | `select!` sur `child.wait()` (et non la fin de stdout, qu'un enfant peut garder ouverte), puis `job.terminate()` (J6) | `exited { code, during_turn, stderr_tail }` ; bandeau rouge, saisie désactivée, bouton « relancer » = `start_session` avec `--resume <session_id>` |
| Sortie non nulle | `code != 0` | idem, `notice` Error avec code ; code 0 après fermeture de stdin = arrêt normal, pas d'alerte |
| stderr | lu en continu, ligne à ligne | 200 lignes en mémoire, toutes dans le journal ; `stderr` vers l'interface (panneau diagnostics), jamais dans le fil |
| Ligne JSON illisible ou trop longue | `serde_json` échoue, ou ligne au-delà du plafond de 64 Mio (`MAX_LINE_BYTES`, **livré en J2** : la ligne est ignorée, la lecture continue) | ignorée ; la `notice` Warn `line_too_long` (1 par minute au plus) et la ligne brute au journal restent à faire en J6 |
| Événement de type inconnu | variante `Unknown` (y compris sous-types `system/*` et types de blocs inconnus) | ignoré, compté, journalisé ; aucun `notice` (le flux en ajoute sans prévenir, décision 12) ; le compteur `unknown_count` du réducteur existe depuis J2 ; reste à faire en J6 : l'exposer dans `Diagnostics` (`get_diagnostics`) |
| Requête de contrôle sans réponse | `deadline` dans `pending_out` | `Timeout` à l'appelant après 5 s (`interrupt`, `set_model`) ou 20 s (`initialize`) |
| `control_cancel_request` | réducteur | `question_closed { Cancelled }`, la carte disparaît |
| Version du CLI inattendue | le **réducteur** compare `version` de `ready` (réponse à `initialize`) et de `init` à `"2.1.325"` | une seule `notice` Warn `unexpected_version` (« version non testée »), affichée en bandeau ; l'app continue, sans blocage (décision 12) ; les mises à jour automatiques de Claude Code restent actives, donc ce cas est normal et non une erreur |
| Capacités | `ready.capabilities` (initialize) puis `init.capabilities` (`system/init`) | `can_skip` de la carte = `ask_user_question_unanswered_v1` présent dans `init.capabilities` (absent de la réponse à `initialize`, constaté dans `t11`) ; une capacité manquante masque la fonction, ne bloque pas |
| Fermeture de l'app | `RunEvent::Exit` | fermer stdin, attendre 3 s, puis fermer le Job Object (tue l'arbre) |

Journal : `%LOCALAPPDATA%\orchestrator\logs\` (rotation quotidienne, 7 jours) ; le flux brut de chaque session y est copié en NDJSON pour fabriquer de nouvelles fixtures à partir d'un bogue réel. Il contient la conversation : il reste local et n'est jamais versionné.

### 1.10 Stratégie de test

1. **Lecteur et réducteur Rust, sur les fixtures** (`src-tauri/tests/fixtures_replay.rs`) : pour chaque `tN.jsonl`, on ignore les lignes `>>> `, `STDERR`, `EXIT` du pilote, on désérialise chaque ligne (0 échec, 0 `Unknown` sur les types déjà connus), on la passe au réducteur, et on compare la suite de `CoreEvent` à un instantané `insta`. Assertions ciblées en plus : `t3` (2 agents, couleurs bleu et rouge après `trim()` de U+00A0, état final `Completed`), `t4`/`t8` (une `question`, `allow` automatique pour Bash), `t6` (`turn_result` `aborted_streaming` puis tour normal), `t10` (réponse « ignorer » sérialisée à l'octet près), `t11` (`ready` avec 108 commandes et 22 agents).
2. **Sérialisation sortante** : chaque `control_response` et `control_request` produit par le cœur est comparé aux lignes `>>> ` des fixtures.
3. **Superviseur** avec un faux `claude.exe` (petit binaire de test `fake_claude` qui rejoue une fixture sur stdout, puis meurt, écrit sur stderr ou sort avec un code) : mort en cours de tour, code non nul, pas de réponse à `initialize`.
4. **Transcripts** : `list_sessions` et `load_history` sur `fixtures/transcripts/` (encodage du chemin, fichier tronqué, ligne illisible).
5. **Interface (Vitest)** : magasins alimentés par `fixtures/ui-events/*.json` ; rendu des lignes (texte, outil, diff, erreur, agent) ; carte de question au clavier ; palette (filtre, Entrée insère `/nom `).
6. **Mesures (Playwright)** sur `npm run dev:mock` : scroll, latence, mémoire (section 1.11).
7. **Vérification manuelle** sur l'app compilée, liste de contrôle dans `docs/reference-tui/README.md` : une vraie session d'une heure de travail, un orchestrateur avec 4 agents, une interruption, un changement de modèle, une question, une reprise, la mort forcée de `claude.exe` (Gestionnaire des tâches).

### 1.11 Critères de recette chiffrés

Machine de référence : le poste de développement, build `release`. Session longue de référence `t15` : générée en concaténant et renumérotant les fixtures jusqu'à **3 000 lignes de rendu** (≈ 1 000 tours, 600 appels d'outils, 80 diffs, 40 sous-agents, ≈ 3 Mo de texte), plus une vraie session d'une heure capturée dans le journal.

| Critère | Seuil | Mesure |
| - | - | - |
| Scroll | ≥ 58 i/s en moyenne et < 1 % d'images au-delà de 33 ms, pendant 10 s de défilement continu (molette et glisser) sur `t15` | trace Playwright / DevTools |
| Saut en haut / en bas | affichage complet en < 100 ms | trace |
| Streaming pendant le scroll | aucune image > 50 ms avec 100 `text_delta`/s entrants | rejeu temporisé |
| Latence d'un token | p95 < 50 ms, p50 < 25 ms entre la lecture de la ligne par Rust (`t`) et sa peinture | horodatage `t` comparé à `performance.timeOrigin + now()` après rAF |
| Ajout d'un message utilisateur | visible en < 50 ms après Entrée | trace |
| Démarrage | fenêtre peinte < 1,5 s ; palette remplie (réponse à `initialize`) < 5 s, hors démarrage à froid de `claude.exe` mesuré à part | horodatages du journal |
| Mémoire | app + WebView2 (hors `claude.exe`) < 250 Mo au repos, < 450 Mo après `t15` ; croissance < 15 % sur 4 h d'usage | Gestionnaire des tâches (ensemble de travail privé) |
| Lecteur Rust | ≥ 50 000 lignes/s rejouées ; 0 panique sur toutes les fixtures | `cargo test` + chronomètre |
| Interruption | état `Idle` dans l'interface < 2 s après Échap | horodatages |
| Fidélité | 100 % des points des listes de contrôle de `docs/reference-tui/` validés par l'utilisateur | revue manuelle |
| Installeur | < 15 Mo | taille du MSI |

---

## 2. Jalons

Légende des estimations : j = journée d'agent (une session de travail continue avec tests). Dépendances notées « après Jx ». Les jalons d'une même vague touchent des zones de fichiers disjointes et peuvent être confiés à des agents en parallèle.

### Vague 0 : lever le risque principal

**J0. Squelette et tuyau de bout en bout, contrat figé** — **fait** (commits `90f7691`, puis `6a50634` : correctifs de la relecture sur interrupt, arguments, CSP, ACL et questions) — 1 j — aucune dépendance.

- Objectif : prouver la chaîne complète la plus risquée : Tauri lance `claude.exe` sans console, dans un Job Object, envoie `initialize` puis un message, et l'interface affiche les tokens au fil de l'eau par un `Channel` avec lots ; figer le contrat de la section 1.5 dans le code.
- Fichiers créés : `package.json`, `vite.config.ts`, `svelte.config.js`, `tsconfig.json`, `index.html`, `src/main.ts`, `src/App.svelte`, `src/lib/core/client.ts`, `src-tauri/` (`Cargo.toml`, `tauri.conf.json`, `capabilities/default.json`, `src/main.rs`, `src/lib.rs`, `src/api/{commands,events,types}.rs` avec des corps provisoires, `src/process/{locate,spawn}.rs`), `src/lib/core/bindings/*.ts` générés, `src-tauri/rust-toolchain.toml`.
- C'est fini quand : `npm run tauri dev` ouvre une fenêtre ; « Dis bonjour en un mot » s'affiche token par token ; aucune fenêtre console n'apparaît ; tuer l'app dans le Gestionnaire des tâches tue aussi `claude.exe` ; la latence p50 d'un token mesurée à la main est < 50 ms ; `export_bindings` produit les types TS de **tous** les événements et commandes de la section 1.5 ; l'utilisateur valide le contrat.

### Vague 1 : en parallèle, après J0 (J1 peut démarrer avec J0)

**J1. Captures de référence (par script) et nouvelles fixtures** — **fait** (commits `64d6717` : script, captures et fixtures, puis `0bc9faa` : capture 03b lisible en entier et mesure du double Échap ; captures dans `.claude-work/j1/captures/`, non versionnées ; inventaire dans `docs/reference-tui/README.md`) — 1 j — sans dépendance de code : peut démarrer en même temps que J0. Zone : `tools/capture-tui/`, `docs/reference-tui/`, `fixtures/stream-json/`.

- Objectif : produire la référence de fidélité (section 1.7, décision 10) **par un agent**, et capturer en stream-json `t13` (un `Edit` puis un `Write`, avec `tool_use_result`), `t14` (outil en erreur, commande Bash à code non nul, `result` en erreur), puis générer `t15` (session longue, section 1.11).
- Méthode de capture (`tools/capture-tui/capture.ps1`, PowerShell 7) :
  1. Créer un dossier de scénario jetable dans le scratchpad, avec les fichiers dont le scénario a besoin (fichier à lire, à modifier).
  2. Lancer `wt.exe -w new --size 120,40 -p "Claude-Ref" -d <dossier> claude --model haiku "<prompt du scénario>"`. Le profil `Claude-Ref` fixe Cascadia Mono 12 pt et le thème. Le prompt passé en argument évite de simuler la frappe ; les touches restantes (Échap, `/`, flèches) sont envoyées par `SendKeys` à la fenêtre ramenée au premier plan.
  3. Attendre l'état voulu : délai fixé par scénario dans `scenarios.json`, avec une marge ; pour les états finaux, attendre le fichier de fin de tour écrit par un hook `Stop` déclaré dans le `.claude/settings.json` du dossier de scénario.
  4. Retrouver la fenêtre (`GetWindowRect` par P/Invoke `user32`, sur le handle du processus `WindowsTerminal` lancé), la capturer seule avec `PrintWindow`, sinon `Graphics.CopyFromScreen` sur son rectangle, puis écrire `.claude-work/j1/captures/NN-nom.png` (non commité).
  5. Fermer la fenêtre, puis passer au scénario suivant. `capture.ps1 -Only 05` refait une seule capture.
- Les 10 captures (PNG dans `.claude-work/j1/captures/`, non commités, avec leurs `.txt` et `.uia.txt`) ; plus `03b` et `09b` (vue détaillée Ctrl+O), `c*`, `k*` et la mesure du double Échap (`a000` à `a800`, `n000` à `n800`, `v1` à `v5`, scénario `echap-intervalles`) :

  | N° | Fichier | État capturé |
  | - | - | - |
  | 01 | `01-accueil.png` | écran de démarrage, ligne de saisie vide |
  | 02 | `02-markdown-code.png` | réponse avec titres, listes, gras, code en ligne et bloc de code coloré |
  | 03 | `03-outils-tronque.png` | appels `Read` et `Bash` **regroupés** en une ligne grise « Read 1 file, ran 1 shell command », sans puce ni résultat (le détail est en 03b) |
  | 04 | `04-edit-diff.png` | `Update(chemin)` avec diff numéroté, ajouts et suppressions |
  | 05 | `05-agents-en-cours.png` | 2 sous-agents parallèles colorés en cours, indicateur d'activité visible |
  | 06 | `06-agents-termines.png` | les mêmes agents terminés : « › Message from @agent-rouge (ctrl+o to expand) », « ● Agent "Ping rouge" finished · 42s » (pas de `Done (…)`), puis la réponse finale |
  | 07 | `07-question.png` | une question `AskUserQuestion` avec ses options |
  | 08 | `08-interruption.png` | tour interrompu par Échap : « No result recorded for 1 call », « ⎿  $ ping -n 90 127.0.0.1 », « ⎿  Interrupted · What should Claude do instead? » |
  | 09 | `09-erreurs.png` | outil en erreur, Bash à code non nul |
  | 10 | `10-menu-slash.png` | menu ouvert par `/` en début de saisie |

- Validation par l'utilisateur (**point d'attente** : J5 ne démarre sa revue de fidélité qu'une fois les captures validées) : `docs/reference-tui/README.md` contient un tableau par capture (scénario, version, statut `à valider` / `validée` / `à refaire`, remarque) et la liste de contrôle. L'utilisateur parcourt les 10 PNG et marque chaque capture. L'agent refait celles qui sont `à refaire` avec `-Only`, jusqu'à 10 `validée`.
- Fichiers créés : `tools/capture-tui/{capture.ps1,scenarios.json,README.md}`, captures `NN-*.png`, `.uia.txt` et `.txt` dans `.claude-work/j1/captures/` (non versionnées : elles montrent des chemins personnels), `docs/reference-tui/README.md`, `fixtures/stream-json/t13.jsonl`, `t14.jsonl`, `t15.jsonl`, mise à jour du README des fixtures.
- C'est fini quand : `capture.ps1` rejoue les 10 scénarios sans intervention ; les 10 captures sont `validée` par l'utilisateur ; chaque capture a sa liste de contrôle ; `t13` montre la forme de `tool_use_result` d'Edit ; les captures ne contiennent ni nom d'utilisateur ni chemin personnel (dossier de scénario neutre, invite sans chemin complet).

**J2. Lecteur et protocole Rust** — **fait** (commit `e917ede`) — 1 j — après J0. Zone : `src-tauri/src/protocol/`, `src-tauri/src/process/{reader,writer}.rs`, `src-tauri/tests/`.

- Objectif : enums serde de tout le flux (spec 4.3 à 4.7bis), lecteur de lignes sans limite de taille, sérialisation sortante.
- Fichiers créés : `protocol/{inbound,outbound,control}.rs`, `process/{reader,writer}.rs`, `tests/fixtures_replay.rs` (partie lecture).
- Écarts livrés (`e917ede`) :
  - le **plafond de taille de ligne de 64 Mio** (`MAX_LINE_BYTES`, ligne ignorée, lecture continue) est livré dès J2, et non en J6 ;
  - deux chemins de décodage équivalents : `Inbound::from_value` et le décodeur rapide `Inbound::from_line` (sans `Value` intermédiaire) ; fichiers en plus : `protocol/{fixture,lenient}.rs`, `tests/line_reader.rs`, `tests/live_reader.rs` ;
  - les fichiers `src-tauri/permissions/autogenerated/` sont **ignorés par git** (retirés de l'index) et régénérés à chaque build ;
  - il reste pour **J6** la notice `line_too_long` (avec la limite à 1 par minute) et l'exposition de `unknown_count` dans `Diagnostics` (le compteur existe dans le réducteur).
- C'est fini quand : les 11 fixtures se désérialisent sans erreur ni `Unknown` sur les types listés en spec 4.3 ; les sorties sérialisées égalent les lignes `>>> ` de `t4`, `t5`, `t6`, `t10`, `t11` ; ≥ 50 000 lignes/s.

**J3. Interface : conversation virtualisée sur données simulées** — **fait** (commits `f242437`, puis `9f5cae3` : correctifs de la relecture sur collage, bandeaux, lot robuste et mesures) — 1,5 j — après J0. Zone : `src/lib/state/session.svelte.ts`, `src/lib/components/Conversation.svelte`, `src/lib/core/mock.ts`, `src/App.svelte`, `e2e/` (dont `e2e/unit/` et `e2e/playwright.config.ts`).

- Objectif : liste virtualisée collée en bas, application des lots par image, `live` en streaming ; données venant d'événements écrits à la main conformes au contrat (en attendant `replay`).
- Fichiers créés (tels que livrés) : `state/session.svelte.ts`, `components/Conversation.svelte`, `core/mock.ts`, `e2e/scroll.spec.ts`, `e2e/sticky.spec.ts`, `e2e/playwright.config.ts`, `e2e/unit/session.test.ts`.
- Écarts livrés :
  - les tests **Vitest** sont dans `e2e/unit/` (et non `tests/`), et `playwright.config.ts` dans `e2e/` ;
  - `mock.ts` est chargé par **`import()` dynamique** depuis `App.svelte` (hors de Tauri ou avec `?mock=`) ;
  - le **rendu provisoire** (texte mono et puce) est dans `Conversation.svelte`, sans `rows/` : J5 l'en extrait en `rows/*` ;
  - les seuils mesurés **sous charge processeur** sont consignés dans le rapport J3 (mesures `npm run e2e`) ; les critères chiffrés de la section 1.11 restent ceux de la recette J12.
- C'est fini quand : sur 3 000 lignes simulées, le scroll tient ≥ 58 i/s ; le défilement reste collé en bas pendant le streaming et ne saute pas quand on lit plus haut ; Vitest vert.

### Vague 2 : après J2 (cœur), J3 (interface) et les fixtures `t13`, `t14` de J1

**J4. Réducteur d'état Rust et outil `replay`** — **fait** (commit `5c9f2f0`) — 1,5 j — après J2 et après les fixtures `t13` et `t14` de J1 (son critère les exige ; la validation des PNG n'est pas requise). Zone : `src-tauri/src/state/`, `src-tauri/src/bin/replay.rs`, `src-tauri/src/colors.rs`.

- Objectif : `reduce()` pur produisant tous les `CoreEvent` : fils, blocs, agents (`tool_use_id`, `task_id`, couleurs), `pending_in`/`pending_out`, `allow` automatique, `turn_result`, batcher.
- Fichiers créés : `state/{reducer,agents,pending,threads,batcher}.rs`, `colors.rs`, `bin/replay.rs`, `tests/snapshots/*.snap`, `fixtures/ui-events/*.json`.
- C'est fini quand : instantanés validés pour les 13 fixtures ; assertions ciblées de la section 1.10 vertes ; `npm run fixtures` régénère `fixtures/ui-events/`.

**J5. Rendu terminal des blocs, vue détaillée Ctrl+O** — **fait** (commits `ebe39d9`, puis `bf3d6cb` : correctifs de la relecture sur sécurité, diffs, fidélité et ligne d'état ; Write, codé sans capture, a été capturé dans le TUI au J13 et son rendu recalé, section 1.7) — 2,5 j — après J3 et après la **validation par l'utilisateur** des captures de J1 ; peut commencer sur les données simulées avant la fin de J4. Zone : `src/lib/render/`, `src/lib/components/rows/`, `app.css`, polices.

- Objectif : règles de la section 1.7, d'après les captures de J1 (Write capturé après coup, au J13 : `w1`, `w1b`, section 1.7) : messages, markdown, code coloré, **regroupement des appels d'outils** en une ligne grise (vue normale), lignes Edit/Write, diffs, erreurs, interruption grise, lignes des sous-agents en arrière-plan, indicateur de tour et ligne de fin de tour. **Ctrl+O** (vue détaillée de `03b` : chaque appel sur sa ligne avec sa sortie complète, puis retour à la vue normale) entre dans le prototype 1 avec ce jalon.
- Fichiers créés : `render/{markdown,highlight,diff,glyphs,colors,group}.ts` (`group.ts` : regroupement du type « Read 1 file, ran 1 shell command »), `rows/{ToolGroup,ToolCall,ToolResult,EditDiff,AgentLaunch,AgentFinished,TurnResult,Interrupted}.svelte`, `components/StatusLine.svelte`, `src/assets/fonts/CascadiaMono*.woff2`, `e2e/unit/render.test.ts`. J5 extrait aussi le rendu provisoire de `Conversation.svelte` vers `rows/`.
- C'est fini quand : `t1`, `t3`, `t6`, `t13`, `t14` rejoués dans `dev:mock` cochent la liste de contrôle de `docs/reference-tui/README.md` (revue de l'utilisateur ; captures 02, 03, 03b, 04, 05, 06, 08, 09, 09b, k1) ; Ctrl+O bascule entre la vue normale et la vue détaillée sans perdre la position de lecture ; le scroll de J3 reste ≥ 58 i/s avec le rendu complet, dans les deux vues.

**J6. Supervision du processus, robustesse et erreurs** — **fait** (commits `5c09322`, puis `3fc9372` : correctifs de la relecture sur interrupt, installeur, fin de session et flux brut) — 1,5 j — après J2 ; en parallèle de J4 (fichiers disjoints). Zone : `src-tauri/src/process/supervisor.rs`, `errors.rs`, `logging.rs`, `tests/fake_claude`.

- Objectif : boucle `select!` stdout/stderr/stdin/minuteries, délais des requêtes, arrêt propre, cas de la section 1.9, journal. La notice de version (`unexpected_version`) est déjà émise par le réducteur depuis J0 (sur `ready` et `init`) : J6 n'a pas à la déplacer, il ajoute seulement le bandeau qui l'affiche.
- Points de robustesse reportés de J0 :
  - `exited` ne dépend pas de la fin de stdout : `select!` sur `child.wait()`, puis `job.terminate()` pour finir l'arbre ;
  - une session orpheline après un rechargement de la webview (canal fermé, interface relancée) est détectée et fermée ;
  - `Diagnostics` expose `unknown_count`, et `log_path` est renseigné ;
  - plafond de taille de ligne sur stdout : le plafond de 64 Mio et l'ignorance de la ligne sont livrés en J2 ; reste la `notice` Warn `line_too_long` (1 par minute au plus) et le journal ;
  - tests du lecteur : lignes très longues, UTF-8 coupé entre deux lectures, fins de ligne CRLF, stderr abondant.
- Fichiers créés : `process/supervisor.rs` (complété), `errors.rs`, `logging.rs`, `src-tauri/tests/bin/fake_claude.rs`, `tests/supervisor.rs`, `src/lib/components/Banner.svelte`.
- C'est fini quand : les scénarios du faux CLI (mort en plein tour, code 1, rien sur stdout, stdout gardé ouvert par un enfant après la mort du processus, stderr abondant, ligne géante, UTF-8 coupé, CRLF) produisent les bons `exited`/`notice` ; fermer l'app ou recharger la webview ne laisse aucun `claude.exe` (vérifié avec `tasklist`) ; un faux CLI qui annonce `claude_code_version: "9.9.9"` et émet des types inconnus donne un seul bandeau « version non testée », une session qui fonctionne, un `unknown_count` non nul et un `log_path` renseigné dans `get_diagnostics`, sans aucun blocage.

**J11. Projets, liste et reprise de sessions avec historique** — **fait** (commits `4ea516b`, puis `99513a2` : correctifs de la relecture (custom-title, plafonds de l'historique) ; branché dans l'écran principal par `b63eb7d`) — 2 j. Zone : `src-tauri/src/sessions/` (dont `transcript.rs` isolé), `components/{ProjectPicker,SessionPicker}.svelte`, `state/sessions.svelte.ts`, `fixtures/transcripts/`. Vague 2 : après J4 (le réducteur sert aussi à l'historique), en parallèle de J5 et J6 ; l'affichage de l'historique réutilise les lignes de rendu de J5 dès qu'elles existent.

- Dossier de projet (décision 11) : `<app> .` ou `<app> <chemin>` ouvre directement ce dossier (`get_launch_info`, complété par l'information « dossier passé en argument ») ; sans argument, sélecteur des projets récents (`list_projects`, section 1.8), chemin réel lu dans le `cwd` du transcript, dossiers disparus grisés.
- Reprise (décision 9) : tout l'historique relu dans le transcript par le lecteur isolé et tolérant, passé au même réducteur que le flux.
- Capabilities : ajouter `allow-list-projects`, `allow-list-sessions`, `allow-load-history` et `allow-stop-session` à `src-tauri/capabilities/default.json` (section 1.5) ; `list_projects` n'existe pas encore dans le code, il est créé ici (le contrat de J0 ne le contient que sur le papier).
- C'est fini quand : `<app> .` lancé depuis un dossier ouvre une session dans ce dossier ; sans argument, le sélecteur liste les projets de `~/.claude/projects` avec leur vrai chemin ; la liste des sessions d'un dossier s'affiche en < 300 ms pour 200 sessions (titre, date) ; reprendre une session affiche **tout** son historique (texte, outils, résultats) et le tour suivant garde le contexte (comme `t7`) ; un transcript avec des lignes inconnues ou abîmées se charge quand même (lignes ignorées comptées), et un transcript illisible donne une reprise sans historique avec un avertissement ; les tests `transcripts` passent sur les extraits anonymisés.

### Vague 3 : fonctions, en parallèle après J4 + J5 + J6

J7 à J10 touchent chacun un composant et une commande distincts ; le seul point commun est `api/commands.rs`, déjà figé en J0 (corps à remplir, signatures inchangées).

**J7. Saisie, interruption, modèle, raccourcis** — **fait** (commits `fda656c`, `6b6169d` : tests adaptés à la saisie multiligne, puis `64378f5` : correctifs de la relecture sur IME, Échap répété, historique, interruption et /model) — 1 j. Zone : `components/Prompt.svelte`, `src/lib/state/keys.svelte.ts` (raccourcis globaux), corps de `send_message`, `interrupt`, `set_model`.

- Raccourcis d'après le comportement observé du TUI 2.1.325 (décision 13, captures `c*`, `a*`, `n*`, `v*`, `k*` de `docs/reference-tui/README.md`) :
  - **Ctrl+C** : avec une sélection (conversation ou saisie), copie ; pendant un tour, **interrompt et garde la saisie** (`c4`, `k2`) ; au repos avec une saisie non vide, efface la saisie (`c2`) ; au repos avec une saisie vide, **ne fait rien** : le TUI affiche « Press Ctrl-C again to exit » (`c3`), mais l'app ne se quitte pas au clavier dans le prototype 1, et ne rien faire est le plus simple.
  - **Échap** : palette ou sélecteur ouvert, il les ferme d'abord ; carte de question ouverte, il l'ignore si `can_skip` (« Esc to cancel », `07`), sinon il ne fait rien ; pendant un tour, il interrompt (`08`) ; au repos avec une saisie non vide, premier appui → indication « Esc again to clear » **sans rien effacer**, second appui à moins d'environ 0,8 s → la saisie est effacée (mesuré : effacée à 736 ms, gardée à 843 ms ; `a000` à `a800`, `n000` à `n800`) ; saisie vide : deux appuis ouvrent le menu Rewind du TUI, après un tour normal (`v1`) comme après un tour interrompu (`v4`), un appui de plus le referme (`v2`, `v5`) ; **Rewind est hors du prototype 1** : un double Échap sur une saisie vide ne fait rien.
  - **`/`** en début de saisie et **Ctrl+K** ouvrent la palette (J10) ; **Ctrl+O** bascule la vue détaillée (J5).
- Capabilities : `send_message` est déjà autorisée ; ajouter `allow-interrupt` et `allow-set-model` à `src-tauri/capabilities/default.json` (section 1.5).
- C'est fini quand : Entrée envoie, Maj+Entrée fait un saut de ligne, historique ↑↓ ; Échap pendant un tour interrompt (`t6` reproduit en vrai, état `Idle` < 2 s, rendu de `08`) ; Ctrl+C pendant un tour interrompt et laisse le brouillon dans la saisie ; chaque cas des deux listes ci-dessus a son test Vitest (sélection, tour en cours, saisie vide ou non, double Échap rapproché (< 0,8 s) ou espacé (> 0,8 s), double Échap sur saisie vide (sans effet), palette ouverte, carte ignorable ou non) ; un sélecteur alimenté par `ready.models` change le modèle et le tour suivant l'utilise (`init.model`).

**J8. Carte `AskUserQuestion`** — **fait** (commit `a3efe05` ; finitions `d5bda56` : raison `replied_in_chat` et multi-sélection vérifiée en réel) — 1 j. Zone : `components/QuestionCard.svelte`, corps de `answer_question` et `skip_question`, `api/events.rs` (raison `Skipped`).

- Rendu d'après `07-question.txt` (section 1.7) : pas d'encadré ; règle, pastille « ☐ <header> », question en gras, `❯ 1. <label>` et descriptions, « N. Type something. » pour la réponse libre, règle, « N+1. Chat about this », pied « Enter to select · ↑/↓ to navigate · Esc to cancel ». La carte prend la place de la saisie.
- « Chat about this » : proposition à valider en revue, `skip_question` avec la raison `replied_in_chat` (valeur d'`annotations[…].unanswered` prévue par `sdk.d.ts`), puis focus dans la saisie.
- Contrat : `skip_question` émet aujourd'hui `question_closed { Answered }` ; J8 ajoute la raison `Skipped` pour que l'interface distingue une question ignorée d'une question répondue.
- Capabilities : ajouter `allow-answer-question` et `allow-skip-question` à `src-tauri/capabilities/default.json` (section 1.5).
- C'est fini quand : en vrai, une question simple, une multi-sélection, une réponse libre (« Type something. ») et un « ignorer » par Échap aboutissent, et Claude cite la réponse ; les réponses envoyées égalent `t4` et `t10` ; `question_closed` porte `Skipped` après un « ignorer » ; tout se fait au clavier (↑↓, chiffres, Entrée, Échap) ; la carte coche les points « Question » de la liste de contrôle.

**J9. Panneau des agents** — **fait** (commit `a8f5728` : composant autonome et démo simulée ; branché à droite de l'écran principal par `32869dc`) — 1 j (+ 0,5 j pour le bonus). Zone : `components/AgentPanel.svelte`, `AgentLine.svelte`, `state/agents.svelte.ts`.

- Capabilities : aucune commande nouvelle (le panneau lit les événements `agent`) ; si un jalon ajoute `stop_task`, il ajoutera sa `allow-…`.
- C'est fini quand : `t3` rejoué montre 2 lignes bleu et rouge dans le style du panneau du TUI (`◯ <agent>  <description> … <durée> · ↓ <tokens> tokens`, `05-agents-en-cours.txt`), état, durée qui avance puis se fige, `last_tool` ; en vrai, `/orchestrateur` avec 4 agents parallèles reste lisible ; bonus : clic → fil de l'agent.

**J10. Palette (Ctrl+K et `/`)** — **fait** (commit `827a73f` ; finitions `82e444c`, `d236af4`) — 0,5 j. Zone : `components/Palette.svelte`, `state/palette.svelte.ts`.

- Deux entrées (décision 13) : Ctrl+K ouvre la palette n'importe où ; `/` tapé **en début de saisie** l'ouvre **au-dessus** de la ligne de saisie, comme le menu du TUI (`10-menu-slash.txt`), et les caractères suivants filtrent la liste. Un `/` au milieu du texte reste un caractère normal.
- Capabilities : aucune commande nouvelle (la palette lit `ready` et `init`).
- C'est fini quand : Ctrl+K et `/` ouvrent la palette avant tout message (données de `ready`, `t11`) ; filtre flou sur nom et description ; Entrée insère `/nom ` dans la saisie ; Échap ferme sans rien insérer ; `init.skills` complète la liste sans la vider ; le rendu ressemble à la capture `10-menu-slash.png`.

### Vague 4 : recette

**J12. Performance, robustesse, build** — **fait** (commit `e034742` : recette, mesures dans l'app compilée, installeur ; remesure après correctifs `4c41fd9`, `bafea38`, `c0f4c07`, voir `docs/recette-prototype-1.md`) — 1,5 j — après tous. Zone : `e2e/`, `docs/reference-tui/README.md`, réglages.

- Objectif : mesurer les seuils de la section 1.11, corriger, produire l'installeur.
- C'est fini quand : tous les seuils chiffrés sont atteints et consignés (tableau des mesures dans `docs/reference-tui/README.md`) ; la vérification manuelle de la section 1.10 (point 7) est passée ; `npm run tauri build` produit un MSI < 15 Mo qui s'installe et démarre.

### Vague 5 : auto-amélioration (après le prototype 1)

**J14. L'app s'améliore elle-même : interface rechargée à chaud, cycle git dans son propre clone** — **fait** (contrat `aa79d93` ; sessions rattachables `9d91aea`, `f3adfbf` ; hébergement de l'interface `9653e78`, `6c227c4` ; clone et cycle git `7f18b4a`, `d018918` ; interface `e84a1c3`, `d1e8dd8` ; intégration `e719ffb`, `2d2a96a`, `248a91a` ; recette `4e111e3`, tests J11 `7adfdbc` ; mesures dans `docs/recette-prototype-1.md`, section J14) — ≈ 9,5 j d'agent, ≈ 5 j calendaires avec 4 agents — après J12. Décisions 15 à 19 de la spec. Zone : `src-tauri/src/ui/`, `src-tauri/src/selfupdate/`, `src-tauri/src/process/supervisor.rs`, `src-tauri/src/api/`, `src-tauri/src/lib.rs`, `src/App.svelte`, `src/lib/core/`, `src/lib/state/`, `src/lib/components/`, `tools/self-improve/`, `e2e/`.

- Objectif : sur demande (`/ameliorer <demande>`), l'app modifie son interface dans une branche de son propre clone, la teste, la commite, la recharge sans couper la session `claude.exe` et la fusionne en fast-forward ; en cas d'échec, elle revient seule à la version précédente. Une modification du cœur Rust est fusionnée et marquée « relance requise ».
- Essai préalable (app Tauri 2.12.5 jetable, release avec `custom-protocol`, 2026-11-17) : (a) `Context::set_assets` sert un dossier disque en release, avec repli sur les assets embarqués ; (b) `Channel::send` vers une page rechargée ou remplacée par `navigate` renvoie toujours `Ok` (74 envois sur 74 vers le canal de la première page après 4 rechargements, aucune `Err`) et la nouvelle page ne reçoit pas ces messages : la fermeture sur `Err` de `channel_sink` ne se déclenche jamais au rechargement, seul `on_page_load` le signale ; (c) aucun cache : `reload()` relit `index.html` et les scripts, y compris un script modifié sous le même nom.
- Point de départ dans le code : aujourd'hui un rechargement **ferme** les sessions (`lib.rs`, `on_page_load` → `AppState::close_orphans` ; `channel_sink` ferme la session si le canal refuse un lot, `api/commands.rs`), et aucune commande ne renvoie l'état d'une session : l'interface reconstruit ses lignes à partir des `CoreEvent` numérotés (`session.svelte.ts`, détection de trou sur `seq`), le réducteur Rust ne garde que la corrélation. Rejouer le journal des événements reconstruit donc exactement l'état de l'interface.
- Étapes :
  0. **Contrat** (seul, avant les autres, 0,5 j) : types `UiInfo` (hash actif, hash embarqué, empreinte du contrat, `restart_required`, raison si `/ameliorer` est désactivé), `SessionResync { key, cwd, pid, history, events }`, événements `ImproveStatus` ; commandes `attach_sessions`, `ui_ready`, `get_ui_info`, `improve_start`, `improve_cancel`, `ui_rollback` à corps provisoires ; liaisons TS ; `build.rs` et `capabilities/default.json` (`allow-*`). Fichiers : `api/types.rs`, `api/events.rs`, `build.rs`, `capabilities/default.json`, `src/lib/core/bindings/*`, `lib.rs`.
  1. **Sessions rattachables** (1,5 j) : journal des `CoreEvent` par `Session` et historique repris gardé en cache au premier `load_history` ; canal remplaçable, lié à un numéro de génération de page incrémenté par `on_page_load` (seule source de vérité : l'essai de J14 montre que l'envoi sur un canal de l'ancienne page réussit sans erreur) ; `attach_sessions` renvoie journal et historique et branche le nouveau canal ; une session sans rattachement depuis 60 s est fermée. Fichiers : `process/supervisor.rs`, `api/commands.rs`, `tests/supervisor_live.rs`, `devtools/tests/supervisor_fake.rs`.
  2. **Hébergement de l'interface** (1,5 j) : fournisseur d'assets disque (`Context::set_assets`) avec repli sur l'embarqué **en bloc** (version entière valide, sinon toute l'interface embarquée : pas de repli fichier par fichier, qui mélangerait deux versions ; un fichier absent des deux est remplacé par `index.html` par Tauri), chemins réduits à des composants normaux (ni `..`, ni chemin absolu, ni `\`), manifeste SHA-256, `ui\active.json` et `ui\state.json` (versions `bad`), watchdog 5 s à la bascule et au démarrage, bascule par `reload()` (ou `navigate` vers `http://tauri.localhost/?ui=<hash>` pour tracer la version dans l'URL ; l'essai montre que les deux relisent `index.html` et les scripts), nettoyage (active, précédente, 5 plus récentes). Fichiers : nouveaux `src-tauri/src/ui/{mod,assets,store,watchdog}.rs`.
  3. **Clone et cycle git** (2,5 j) : premier clone SSH (dossier d'installation s'il est inscriptible, sinon `%LOCALAPPDATA%\orchestrator\repo`), `checkout` du commit de compilation (hash injecté par `build.rs`), URL compilée remplaçable par `config.json`, raison de désactivation si git, Node, npm ou la clé manquent ; worktree `auto/<AAAAMMJJ-HHMM>-<sujet>` dans `%LOCALAPPDATA%\orchestrator\work\`, jonction `node_modules` si `package-lock.json` est identique (sinon `npm ci`), `CARGO_TARGET_DIR` partagé ; session `claude.exe` dédiée (même `Session::start`) avec le prompt `tools/self-improve/prompt.md` ; contrôles (arbre propre, commits sur la branche, fichiers dans la liste autorisée) ; tests (`npm run check`, `npm test`, e2e de démarrage sur un port dédié ; `cargo test` si `src-tauri/` est touché) ; `vite build` vers `ui\<hash>.tmp` puis renommage ; détection « relance requise » (décision 18) ; `git merge --ff-only` dans le `main` du clone ; retour arrière par `git revert` dans un worktree `auto/…-revert-<hash>`. Tests sur un dépôt git temporaire avec un build simulé. Fichiers : nouveaux `src-tauri/src/selfupdate/{mod,repo,git,pipeline}.rs`, `tools/self-improve/prompt.md`.
  4. **Interface** (1,5 j) : `attach_sessions` au montage avant `get_launch_info`, puis `ui_ready` ; saisie, historique ↑↓ et agents dépliés dans `sessionStorage` ; entrée `/ameliorer` de la palette (grisée avec la raison si désactivée) ; bandeau d'avancement (session dédiée, tests, bascule, échec) ; menu « revenir à la version précédente » ; hash dans la ligne d'état ; constantes `__UI_HASH__` et `__CONTRACT__` (`vite.config.ts`) ; mode simulé et tests unitaires du rejeu. Fichiers : `src/App.svelte`, `src/lib/core/{client,mock}.ts`, `src/lib/state/{session,sessions}.svelte.ts`, `src/lib/components/StatusLine.svelte`, nouveau `src/lib/components/ImproveBanner.svelte`, `vite.config.ts`, `e2e/unit/resync.test.ts`, `e2e/improve.spec.ts`.
  5. **Intégration** (1 j) : brancher `ui/` et `selfupdate/` dans `lib.rs` (`set_assets`, watchdog, remplacement de `close_orphans`), commandes réelles dans un nouveau `api/self_update_commands.rs`, reprise `--resume` au lancement suivant (`%LOCALAPPDATA%\orchestrator\resume.json` écrit à la sortie). Fichiers : `lib.rs`, `api/self_update_commands.rs`, `sessions/mod.rs`.
  6. **Recette et docs** (1 j) : e2e sur l'exe compilé (WebView2 avec `--remote-debugging-port` par `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`, Playwright `connectOverCDP`), cas « interface cassée », mesures ; spec et plan à jour. Fichiers : `e2e/selfupdate.cdp.ts`, `docs/*.md`.
- Étape 0 faite : contrat figé (voir le commit de l'étape 0 dans le journal git). Signatures publiques : `CoreError::NotImplemented(String)` ; types `SessionRole`, `SessionResync`, `UiSource`, `RepoStatus` (`unknown`, `cloning`, `ready`, `disabled { reason }`), `UiInfo`, `ImproveStep`, `ImproveStatus`, constante `IMPROVE_EVENT = "improve_status"` (`api/types.rs`) ; commandes `attach_sessions(events) -> Vec<SessionResync>` (`api/commands.rs`, provisoire : liste vide), `get_ui_info() -> UiInfo`, `ui_ready(hash) -> bool`, `improve_start(request) -> ImproveStatus`, `improve_cancel(id)`, `ui_rollback() -> UiInfo` (`api/self_update.rs`, provisoires : `NotImplemented` pour les trois dernières) ; `build_info::{BUILD_COMMIT, CONTRACT}` injectés par `build.rs`, même empreinte que `tools/contract-fingerprint.mjs` (test `node_fingerprint_matches`) ; `ui::UiHost` (`new`, `default_root`, `active`, `assets`, `arm_watchdog`, `ready`, `install`, `activate`, `rollback`, `cleanup`, `READY_TIMEOUT`, `KEEP_RECENT`) et `selfupdate::{RepoConfig::load, ensure_clone, Pipeline::{new, request, run, cancel, revert}, PipelineHost, SelfUpdate, StatusSink, ImproveRequest, ImproveOutcome}`, gérés par Tauri (`app.manage`) sans être branchés ; `MockSelfUpdate` dans `mock.ts`, fonctions `attachSessions`, `uiReady`, `getUiInfo`, `improveStart`, `improveCancel`, `uiRollback` dans `client.ts` ; dépendance `sha2` ajoutée à `Cargo.toml`.
- Étapes 1 à 6 faites (voir les messages des commits cités en tête). Intégration (étape 5), écarts au plan :
  - Commandes réelles dans `api/self_update.rs` (pas de nouveau `api/self_update_commands.rs`) ; `PipelineHost` réel (`AppHost`) et toolchain `LockingToolchain` (verrou de build tenu pendant le `vite build`) au même endroit.
  - La session d'amélioration est protégée de la fermeture différée de 60 s tant que le cycle la tient (`AppState::protect`), lancée avec `--disallowedTools` (git push, remote, config, submodule, worktree, update-ref, branch -f, et `AskUserQuestion`) et un git sans SSH, sans invite ni hooks ; modèle : clé `improve_model` de `config.json`.
  - Le cœur garde au plus une session principale : `start_session` ferme les autres sessions `main` vivantes (`close_other_mains`).
  - L'empreinte du contrat est vérifiée deux fois : dans la sortie de Vite avant d'activer (`switch_ui`), et au démarrage pour la version active (`leave_incompatible_ui`, constat de la recette : un exe réinstallé servait sinon une interface d'un autre contrat).
  - **Non fait** : reprise `--resume` au lancement suivant (`resume.json`, critère 5) ; elle demande un champ du contrat (`LaunchInfo`) et l'interface. Voir la section 4.
- **Envoi réservé au propriétaire** (après la recette ; app multi-utilisateurs, spec décisions 8 et 16) : réglage `"push": true` de `config.json`, jamais dans l'interface ; droit d'écriture vérifié sur GitHub par `git push --dry-run` vers une référence sonde (au démarrage, clone prêt, et avant chaque envoi) ; sans droit, envoi désactivé avec la raison (`UiInfo.push`). Le pipeline (`selfupdate/push.rs`), jamais la session, envoie après la fusion la seule branche `auto/…` (cycle normal, relance requise, retour arrière) : refspec `<commit>:refs/heads/auto/…`, sans force ni suppression, vers l'URL réelle en argument (le `pushurl` du clone reste `no-push://`), environnement de l'app, durcissements de `git.rs`. Échec : fusion acquise, `ImproveStatus.push` = `failed { reason }`, branche dans `work\push-pending.json`, renvoyée au cycle suivant et au démarrage. Contrat : `PushStatus`, `PushResult`, champs `UiInfo.push` et `ImproveStatus.push` (empreinte du contrat changée). Relecture : envoi seulement vers le dépôt canonique `DEFAULT_ORIGIN` (`repo_url` et `remote.origin.url` du clone vérifiés, formes `git@…:` et `ssh://git@…/` seulement), sonde refaite avant chaque envoi et après une sonde impossible, fusion mise en attente sauf refus définitif, `is_denied` sur des formes précises, réglages git refusés élargis et relus à la sonde et à l'envoi, `config.json` figé au lancement et liste des envois en attente tenue en mémoire (cycle refusé si la session les modifie ; contrôle aussi avant la session et après une session en échec ou annulée). Seconde relecture : sonde et envoi sans `~/.gitconfig` ni configuration système (`GIT_CONFIG_GLOBAL` vide, `GIT_CONFIG_NOSYSTEM`, variables git héritées retirées), ssh sans `~/.ssh/config` (`-F` fichier vide, hôte, utilisateur, port imposés, sans mandataire, `StrictHostKeyChecking=yes`), `IdentityFile` relevés au lancement. Bandeau : « poussé sur GitHub : auto/… » ou « fusionné localement, envoi échoué : … », rien pour les autres utilisateurs. Tests : `selfupdate_git` (dépôts nus locaux joints par un faux ssh qui imite GitHub), `e2e/unit/selfupdate.mock.test.ts`, `e2e/improve.spec.ts` (`?push=pushed|failed`) ; recette dans `docs/recette-prototype-1.md`, section J14.
- Découpage en agents (après l'étape 0). **Aucun fichier commun** : un agent qui a besoin d'un fichier hors de sa liste le signale dans son rapport et ne le modifie pas. Fichiers figés jusqu'à l'étape 5 (intégration seule) : `src-tauri/src/lib.rs`, `src-tauri/src/api/self_update.rs`, `src-tauri/src/api/types.rs`, `src-tauri/src/api/events.rs`, `src-tauri/src/api/mod.rs`, `src-tauri/src/build_info.rs`, `src-tauri/build.rs`, `src-tauri/capabilities/default.json`, `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`, `src/lib/core/bindings/*`, `tools/contract-fingerprint.mjs`, `docs/*`. Tranché : `api/commands.rs` est à **A** seul (dont `attach_sessions`, `AppState`, `channel_sink`) ; `src/App.svelte` et `src/lib/state/session.svelte.ts` sont à **D** seul ; `lib.rs` n'est modifié par personne avant l'étape 5, donc A garde le point d'entrée `AppState::close_orphans()` appelé par `on_page_load` (même signature, nouveau sens : détacher au lieu de fermer).
  - **A, sessions rattachables (étape 1)** : `src-tauri/src/api/commands.rs`, `src-tauri/src/process/supervisor.rs`, `src-tauri/src/state/batcher.rs`, `src-tauri/tests/supervisor_live.rs`, `src-tauri/tests/sessions_live.rs`, `src-tauri/devtools/tests/supervisor_fake.rs`, nouveau `src-tauri/tests/attach_sessions.rs`. À fournir pour l'étape 5 : démarrage d'une session de rôle `Improve` sans canal d'interface (événements vers le canal de `attach_sessions`), et attente de la fin du tour de cette session.
  - **B, hébergement de l'interface (étape 2)** : `src-tauri/src/ui/**` (dont `mod.rs` et les nouveaux `assets.rs`, `store.rs`, `watchdog.rs`), nouveau `src-tauri/tests/ui_host.rs`. Signatures publiques de `ui/mod.rs` à garder (ajouts permis).
  - **C, clone et cycle git (étape 3)** : `src-tauri/src/selfupdate/**` (dont `mod.rs` et les nouveaux `repo.rs`, `git.rs`, `pipeline.rs`), `tools/self-improve/**` (dont `prompt.md`), nouveau `src-tauri/tests/selfupdate_git.rs`. Signatures publiques de `selfupdate/mod.rs` à garder (ajouts permis) ; Tauri et `AppState` restent hors du module (passage par `PipelineHost`).
  - **D, interface (étape 4)** : `src/App.svelte`, `src/main.ts`, `src/lib/core/client.ts`, `src/lib/core/mock.ts`, `src/lib/state/session.svelte.ts`, `src/lib/state/sessions.svelte.ts`, `src/lib/components/StatusLine.svelte`, `src/lib/components/Palette.svelte`, nouveaux `src/lib/components/ImproveBanner.svelte` et `src/vite-env.d.ts`, `vite.config.ts`, `e2e/unit/selfupdate.mock.test.ts`, nouveaux `e2e/unit/resync.test.ts` et `e2e/improve.spec.ts`.
  - Chacun commite par `git commit -F <message> -- <fichiers>` (index partagé), après `git diff --cached --name-only`. Un seul agent fait ensuite les étapes 5 et 6.
- Capabilities : `allow-attach-sessions`, `allow-ui-ready`, `allow-get-ui-info`, `allow-improve-start`, `allow-improve-cancel`, `allow-ui-rollback`. L'origine reste `http://tauri.localhost` : CSP et ACL inchangées.
- Risques : journal d'une longue session trop gros (mesurer sur la fixture longue ; au-delà de 50 Mo sur 4 h, compacter les deltas des blocs terminés) ; CSP : un script en ligne dans un `index.html` lu sur disque est bloqué (pas d'empreinte), la sortie de Vite n'en a pas, à garder ainsi ; un `throw` après le signal « prêt » passe le watchdog (tests obligatoires avant bascule, retour arrière en un clic) ; clone sans réseau ou sans clé (fonction désactivée avec la raison) ; port e2e partagé (port dédié pour le cycle) ; build Rust concurrent (`CARGO_TARGET_DIR` partagé, verrou de cargo).
- C'est fini quand : (1) une bascule pendant une session active garde le même pid `claude.exe`, le même nombre de lignes affichées et aucun trou de `seq`, et le rattachement de 10 000 événements prend moins de 1 s ; (2) une interface qui lève une erreur au montage est remplacée par la précédente en 6 s ou moins après la bascule, la branche reste non fusionnée (`git branch --no-merged main`) et `main` du clone ne bouge pas ; (3) un cycle réussi avance `main` du clone des seuls commits de la branche, en fast-forward, et le hash affiché égale `git rev-parse --short main` ; (4) le retour arrière par le menu réactive la version précédente en 5 s ou moins et crée un commit `Revert "…"` ; (5) une modification de `src-tauri/` n'est pas activée, est fusionnée après `cargo test` et affiche « relance requise », et le lancement suivant reprend la session ; (6) premier lancement sans réseau ou sans clé : `/ameliorer` est grisé avec la raison, le reste de l'app fonctionne ; (7) le fournisseur disque refuse `..`, `%2e%2e` et un chemin absolu, et repasse à l'interface embarquée si le dossier manque ou si le manifeste est faux (tests unitaires) ; (8) après 8 cycles, au plus 7 dossiers dans `ui\`, et aucune violation CSP dans la console.

### Récapitulatif des dépendances

```
J1 (captures + t13, t14 ; en parallèle de J0) ─┬─ fixtures t13, t14 ──────────────► J4
                                                 └─ validation des PNG par l'utilisateur ► J5
J0 ──┬─ J2 (lecteur) ─┬─ J4 (réducteur) ── J11 (projets, sessions, historique) ─┐   vague 2
     │                └─ J6 (supervision, bandeau de version) ──────────────────┤   vague 2
     └─ J3 (liste virtualisée) ── J5 (rendu) ───────────────────────────────────┤   vague 2
                                  après J4 + J5 + J6 : J7 · J8 · J9 · J10 ───────┴── J12 (recette)
```

Total ≈ 17 j d'agent (J1 : 1 j pour le script de capture ; J5 : 2,5 j avec la vue détaillée Ctrl+O ; J6 : 1,5 j avec les points de robustesse reportés de J0 ; J11 : 2 j pour le sélecteur de projets et l'historique complet) ; avec 3 agents en parallèle, environ 7 à 8 jours calendaires.

---

## 3. Risques restants

Les 5 questions de la première version de ce plan sont tranchées par les décisions 9 à 13 de la spec (reprise avec historique, captures par un agent, dossier de projet, version du CLI, raccourcis). Il ne reste aucune question ouverte pour démarrer ; restent ces risques :

1. **Format du transcript** (décision 9) : interne et non documenté, il peut changer à chaque version du CLI. Parade : lecteur isolé et tolérant, extraits de test par version, repli « reprise sans historique ».
2. **Mises à jour automatiques actives** (décision 12) : le CLI peut changer sous l'app, y compris le flag non documenté `--permission-prompt-tool stdio` (spec 4.8). Parade : bandeau « version non testée », types inconnus ignorés, fixtures rejouées à chaque nouvelle version.
3. **Captures par script** (décision 10) : fenêtre au premier plan, délais et `SendKeys` sont fragiles ; un état intermédiaire (agents en cours, interruption) peut être manqué. Parade : délais par scénario, refaire une capture avec `-Only`, validation par l'utilisateur.

---

## 4. Reste à faire

Le prototype 1 est terminé et la recette J12 est faite (`docs/recette-prototype-1.md`). Ce qui n'a pas pu être fait en l'absence de l'utilisateur, ou qui reste à nettoyer :

(Fait depuis, au J13, commit `6d49010` : capture de l'outil Write dans le TUI (`w1`, `w1b`) et rendu recalé, section 1.7 ; suppression de l'ancien scénario `echap` de `tools/capture-tui/scenarios.json`.)

- **Installation du MSI** : elle demande une élévation (installation pour toute la machine). Seul l'installeur NSIS a été installé et désinstallé en silencieux pendant la recette.
- **Vérification manuelle d'une heure de session réelle avec 4 agents** (section 1.10, point 7).
- **Croissance mémoire sur 4 h** (critère de la section 1.11, « < 15 % sur 4 h d'usage ») : non mesurée.
- **Revue de fidélité par l'utilisateur**, avec les comparaisons de `.claude-work/j5`, `.claude-work/j8-j10`, `.claude-work/j9` et `.claude-work/j12` (liste de contrôle de `docs/reference-tui/README.md`, critère « 100 % validé »).
- **Image de retard due à virtua** : la nouvelle ligne n'est insérée dans le DOM qu'à l'événement `scroll` du recollage, à l'image suivante ; elle ne se retire pas côté application (`docs/recette-prototype-1.md`).
- **Transcripts de test laissés dans `~/.claude/projects`** : dossiers `j0-sandbox`, `j11-sandbox`, `j12-sandbox` et autres signalés par les agents (essais réels) ; à supprimer à la main après vérification.
- **Serveur Vite resté ouvert sur le port 1430** : à fermer.
- **Décisions à valider** : spec section 8.
- **J14, auto-amélioration** : fait (section 2, `docs/recette-prototype-1.md` section J14). Restent :
  - **Reprise au lancement suivant** après une « relance requise » (`%LOCALAPPDATA%\orchestrator\resume.json` écrit à la sortie, critère 5 de J14) : non faite, reportée à J15 (spec, décision 18).
  - **Envoi réservé au propriétaire** : vérifié sur des dépôts nus locaux et par une sonde `--dry-run` refusée par GitHub sur un dépôt sans droit ; **pas encore d'envoi réel vers GitHub** (à faire par le propriétaire : `"push": true` dans `config.json`, puis un cycle). Non testé : HTTPS avec le gestionnaire d'identifiants, sonde acceptée sur le vrai dépôt. Non contrôlés : clés ssh, `known_hosts`, gestionnaire d'identifiants, `gh` (spec, décision 17).
  - **Push depuis la session d'amélioration** (relectures finales) : promesse limitée à « pas de push involontaire, ni par les formes testées » (spec, décision 17). La session garde les droits Windows de l'utilisateur (clés SSH, gestionnaire d'identifiants, `gh`, réseau) : une session qui chercherait délibérément à pousser le pourrait. Un clone jetable sans remote ne l'empêche pas. **À trancher par l'utilisateur** : exécution sous un autre compte Windows ou dans un bac à sable sans ses identifiants. Non testé : `git credential-manager` par un autre programme, `ssh -i`, HTTP pur, `VAR=x git push`.
  - **Premier clone depuis GitHub** : le clonage SSH fonctionne (≈ 10 s), mais le commit de compilation doit être sur `origin` ; tant que les commits du dépôt de développement ne sont pas poussés, `/ameliorer` est grisé avec la raison « commit de compilation … introuvable ». La recette a utilisé un clone local (`config.json`, `repo_url` = `file:///` du dépôt de développement).
  - **Changement Rust → relance requise** et **8 cycles → au plus 7 dossiers** : couverts par les tests (`selfupdate_git`, `ui_host`), pas mesurés sur l'exe (un `cargo test` complet dans le clone demande plusieurs Go et plusieurs minutes).
  - Transcripts laissés par la recette dans `~/.claude/projects` : dossier encodé de `.claude-work/j14/sandbox` (session principale) et ceux des worktrees `%LOCALAPPDATA%\orchestrator\work\auto\…` (sessions d'amélioration), à supprimer à la main.
- **J15, réglages appliqués à chaud** (après J14, décision 18 de la spec) : thème, raccourcis, palette et skills affichés lus dans des fichiers de réglages surveillés et appliqués sans recharger l'interface. Aujourd'hui ce sont du code (`src/lib/render/colors.ts`, `src/lib/state/keys.svelte.ts`) : après J14 ils passent déjà par le cycle d'amélioration. Reste à définir le schéma des fichiers, la surveillance (dépendance `notify` côté Rust), l'ordre de priorité et l'affichage des erreurs de réglage.
