# Recette du prototype 1 (jalon J12)

Date : 2026-11-16. Build `release` de `main` à `d236af4` (`npm run tauri build`), Windows 11, poste de développement (machine de référence du plan, section 1.11). `claude` 2.1.325, modèle haiku pour tous les essais réels.

Toutes les mesures de ce rapport sont prises **dans l'app compilée** (`src-tauri/target/release/orchestrator.exe`, donc dans WebView2), pilotée par son port de débogage : la variable `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9333` est posée au lancement, puis Playwright s'y connecte (`chromium.connectOverCDP`). Aucune touche n'est envoyée au bureau : les frappes et la molette sont des événements CDP injectés dans la webview. Les scripts sont dans `tools/recette/` ; leurs sorties (JSON, journaux, captures) dans `.claude-work/j12/`, non versionné.

| Script | Rôle |
| - | - |
| `tools/recette/lib.mjs` | lancement de l'app avec CDP, arbre de processus, mémoire (ensemble de travail privé), fenêtres visibles |
| `tools/recette/startup.mjs` | 5 démarrages : fenêtre peinte, `initialize`, mémoire au repos, fenêtres console, processus restants après fermeture normale, forcée et rechargement |
| `tools/recette/perf.mjs mock` | mesures de `e2e/scroll.spec.ts` sur `?mock=long` (3 000 lignes), dans la webview de l'app compilée |
| `tools/recette/perf.mjs t15` | `t15` rejouée **par le vrai cœur Rust** (faux `claude.exe` : `replay-claude.cmd`, trouvé par `ORCH_CLAUDE_PATH`) : mémoire, scroll, sauts, streaming pendant le scroll, ajout d'un message |
| `tools/recette/perf.mjs latency` | tours en streaming à 100 `text_delta`/s par le vrai cœur ; latence lue dans le pied de l'app (`t` de Rust jusqu'à la peinture) |
| `tools/recette/replay-claude.mjs` | le faux `claude.exe` : réponse à `initialize` de `t11`, puis les 10 505 lignes de `t15`, puis des tours en streaming cadencés |
| `tools/recette/e2e-live.mjs` | essais réels de bout en bout (section 3) |
| `tools/recette/installed.mjs` | démarrage de l'app installée par le NSIS |

## 1. Tableau de recette (section 1.11 du plan)

| Critère | Seuil | Mesure (app compilée) | Méthode | Conforme |
| - | - | - | - | - |
| Scroll | ≥ 58 i/s, < 1 % d'images > 33 ms, 10 s glisser + 10 s molette | `t15` par le vrai cœur : glisser 60 i/s, 0 %, image max 19,3 ms (66,9 écrans parcourus) ; molette 60 i/s, 0 %, max 23 ms. Simulé 3 000 lignes : glisser 60 i/s, 0 %, max 21,7 ms ; molette 60 i/s, 0 %, max 25,6 ms ; vue détaillée (Ctrl+O) : 60 i/s, 0 %, max 20,9 et 18,7 ms | intervalle entre deux `requestAnimationFrame` (`perf.mjs`) | oui |
| Saut en haut / en bas | < 100 ms | `t15` : max 17 ms sur 20 sauts ; simulé : max 29 ms (1re série), 19,5 ms (2e) | `scrollTop` puis ligne visible, mesuré après la peinture | oui |
| Streaming pendant le scroll | aucune image > 50 ms à 100 `text_delta`/s | `t15` + tour réel à 100 deltas/s : 0 image > 50 ms (max 19,3 ms) ; simulé : 0 (max 19,1 ms) | 5 s glisser + 5 s molette pendant le flux | oui |
| Latence d'un token | p50 < 25 ms, p95 < 50 ms | vrai cœur, 100 deltas/s, 2 tours de 10 s : **p50 27 ms puis 28 ms**, p95 43 ms ; simulé : **p50 25,1 et 26,3 ms**, p95 37,3 et 38,5 ms (500 échantillons) ; à titre de comparaison, Chromium sur Vite : p50 22 ms, p95 34,5 ms | `t` de chaque delta comparé à `performance.timeOrigin + now()` après la peinture (pied de l'app ; `__orch.latencies` en simulé) | **non** (p50 dépassé de 0 à 3 ms) ; p95 oui |
| Ajout d'un message utilisateur | < 50 ms après Entrée | `t15` par le vrai cœur : 16,8 / 25,4 / 22,2 ms ; simulé, 2e série : max 40,9 ms ; simulé, 1re série : 21 / 50 / 59 / 93 / **110 ms** | `keydown` Entrée jusqu'à la ligne peinte (observateur de mutations, puis image suivante) | **instable** : 2 séries sur 3 conformes |
| Démarrage : fenêtre peinte | < 1,5 s | 971, 1 085, 1 199, 1 115, **1 510** ms (5 démarrages ; médiane 1 115 ms) | lancement du processus jusqu'au `first-contentful-paint` de la webview | oui en médiane, 1 démarrage sur 5 à 1 510 ms |
| Démarrage : palette remplie (`initialize`) | < 5 s hors démarrage à froid de `claude.exe` | part de l'app (lancement jusqu'à « claude.exe lancé » du journal) : 1,06 à 1,60 s ; part de `claude.exe` (lancé jusqu'à la réponse) : 3,2 à 4,0 s ; total : 4,4 à 5,1 s | journal de l'app (`ORCH_LOG_DIR`) et saisie qui quitte « session non démarrée » | oui (hors `claude.exe`) ; total au-dessus de 5 s dans 2 démarrages sur 5 |
| Mémoire au repos | app + WebView2 < 250 Mo | 99,2 à 104,4 Mo (5 démarrages, 10 s après `initialize`, 7 processus : `orchestrator.exe` et 6 `msedgewebview2.exe`) | ensemble de travail privé (`Win32_PerfFormattedData_PerfProc_Process.WorkingSetPrivate`), hors `claude.exe` | oui |
| Mémoire après `t15` | < 450 Mo | `t15` par le vrai cœur : 213,1 Mo après le chargement, 179,3 Mo après scroll, 2 tours et 3 messages ; simulé 3 000 lignes : 177 et 185 Mo après le chargement, 304 et 305 Mo après environ 25 s de streaming (tas JS 12,8 Mo après GC) | même compteur | oui |
| Croissance sur 4 h | < 15 % | non mesurée | | non mesuré |
| Lecteur Rust | ≥ 50 000 lignes/s ; 0 panique | 203 362 lignes/s (`throughput_on_t15`, `cargo test --release`, 5 passes sur `t15`) ; `replay --check` : 17 fixtures à jour, 0 inconnu, 0 ligne illisible | `cargo test` + chronomètre | oui |
| Interruption | `Idle` < 2 s après Échap | 66 ms (vrai `claude.exe`, haiku, en plein streaming) | statut du pied, de l'Échap (CDP) jusqu'à `Idle` | oui |
| Fidélité | 100 % des listes de `docs/reference-tui/` validées par l'utilisateur | non fait : revue de l'utilisateur | revue manuelle | en attente |
| Installeur | MSI < 15 Mo | MSI 3 235 840 octets (3,09 Mio) ; NSIS 2 380 819 octets (2,27 Mio) | taille des fichiers | oui |
| Pas de fenêtre console | aucune | 0 fenêtre console visible sur 5 démarrages ; seules fenêtres visibles de l'arbre : `Tauri Window` et `Tao Thread Event Target` | énumération des fenêtres visibles de l'arbre de processus (`EnumWindows`) | oui |
| Aucun `claude.exe` restant | fermeture normale, forcée, rechargement | normale : tout l'arbre disparaît en 1,0 à 1,3 s ; forcée (`taskkill /F` sur `orchestrator.exe` seul) : 0,5 s (Job Object) ; rechargement : l'ancien `claude.exe` s'arrête 0,43 s après le lancement du nouveau, puis fermeture en 1,8 s ; aucun processus restant après tous les essais | arbre de processus avant fermeture, puis présence de chaque pid | oui |

### Installeur

- `npm run tauri build` (Cargo `-j 2`) : 3 min 36 s, code 0.
- **MSI** : un seul fichier, `orchestrator.exe` (7 917 056 octets). Vérifié par une extraction administrative (`msiexec /a`, sans installation) : `PFiles\Orchestrator\orchestrator.exe`, rien d'autre.
- **NSIS** : `orchestrator.exe` seul, plus les greffons NSIS de l'installeur lui-même (`$PLUGINSDIR`, non installés). Ni `fake_claude`, ni `probe`, ni `replay`.
- Pas de devtools : la dépendance `tauri` est compilée sans fonctionnalité (`features = []`, `src-tauri/Cargo.toml`).
- **Installation et démarrage**, comme le demande le plan : NSIS installé en silencieux dans un dossier de test (`/S /D=.claude-work\j12\nsis-install`, code 0, 0,9 s), puis l'app installée a démarré et affiché le sélecteur des projets (52 projets). Désinstallation silencieuse : code 0, dossier vide, clé de désinstallation retirée. Le MSI n'a pas été installé : il s'installe pour toute la machine, ce qui demande une élévation, impossible en l'absence de l'utilisateur.

## 2. Tests `#[ignore]` réels

| Test | Résultat |
| - | - |
| `supervisor_live` (`live_reload_leaves_no_claude_process`) | ok : arbre de 11 processus sous `claude.exe` 2.1.325, aucun restant après rechargement |
| `sessions_live` (`live_resume_shows_history_and_keeps_context`) | ok : 2 sessions listées en 3,1 ms, historique de 34 lignes relu, réponse après reprise « MANDARINE-42 » |
| `question_live` (`live_multi_select_and_skip_reasons`) | ok : réponse multiple « Rouge,Bleu » reçue par Claude ; `replied_in_chat` et `dismissed` donnent tous deux « The user did not answer the questions. » |
| `live_reader` (`live_turn_through_reader_and_writer`) | ok : réponse reçue, arrêt propre (code 0, non tué), 17 événements |
| `process_lifecycle` (`live_pipe_end_to_end`, `live_killing_host_kills_claude`) | ok (2 tests) : réponse à `initialize` en 2 749 ms, tour complet, arrêt code 0 ; l'hôte tué brutalement emporte `claude.exe` et ses 4 enfants |

Ces tests tournent dans leurs propres dossiers de travail (`.claude-work/j2-sandbox`, `j0-sandbox`, `j6-sandbox`, `j11-sandbox`, `finitions-sandbox`), codés dans les tests.

Résultat : 6 tests réels lancés, 6 réussis (`.claude-work/j12/live-tests.log`, `live-tests-2.log`). Les tests ignorés de `sessions_list` et `sessions_transcripts` ne lancent pas `claude.exe` (ils lisent `~/.claude/projects`) : ils n'ont pas été lancés.

## 3. Essais réels de bout en bout

App compilée lancée sur `.claude-work/j12/sandbox`, `claude.exe` réel en haiku (`ANTHROPIC_MODEL=haiku`), pilotée par CDP (`tools/recette/e2e-live.mjs`). Captures `e2e-01` à `e2e-12` dans `.claude-work/j12/` (locales : elles montrent des chemins personnels).

| Essai | Résultat | Capture |
| - | - | - |
| Palette `/` | ouverte au-dessus de la saisie avant tout message (données d'`initialize`), filtre « mod » : `/model` en tête, description alignée ; Échap ferme | `e2e-01-palette.png` |
| Message en streaming | statuts `Requesting` puis `Streaming` (premier texte à 1,17 s), réponse complète, fin de tour « Worked for 2s » | `e2e-02-streaming.png` |
| Interruption par Échap | pendant le comptage jusqu'à 300 : `Idle` 66 ms après Échap, ligne « Interrupted · What should Claude do instead? » | `e2e-03-interruption.png` |
| `/model` | sélecteur ouvert (14 modèles annoncés), haiku choisi au clavier, indication « Set model to Haiku 5.5 », sélecteur refermé | `e2e-04-model-picker.png`, `e2e-05-model-choisi.png` |
| AskUserQuestion | carte « ☐ Couleur », options Rouge / Bleu, « Type something. », « Chat about this » ; Entrée choisit Rouge ; Claude répond « Rouge » | `e2e-06-question.png`, `e2e-07-question-repondue.png` |
| 2 sous-agents | `agent-bleu` et `agent-rouge` lancés ensemble ; colonne : « 2 running · 2 agents » avec les couleurs, puis « Done » avec durée et tokens | `e2e-08-agents-en-cours.png`, `e2e-09-agents-fin.png` |
| Reprise avec historique | relance sans argument, sélecteur des projets filtré, sélecteur des sessions, reprise : 11 lignes d'historique (les tours précédents, l'interruption, la question, les agents) ; nouvelle question « Quelle couleur ai-je choisie ? » : réponse « Rouge » (contexte conservé) | `e2e-10-sessions.png`, `e2e-11-reprise-historique.png`, `e2e-12-reprise-reponse.png` |

Après chaque fermeture normale (après la 1re session et après la reprise), tout l'arbre de processus a disparu.

Remarque sur le script : il marque la palette « ok: false » dans `e2e-live.json`, car il comptait les entrées affichées (3, la fenêtre visible du menu) et non les entrées disponibles. La capture montre la palette remplie et filtrée : l'essai est réussi.

## 4. Limites de la recette

- **Session Windows** : l'utilisateur était absent et `LogonUI.exe` tournait, mais `query user` donnait la session active, `document.visibilityState` valait `visible` et les cadences mesurées sont de 60 i/s : le rendu n'a pas paru suspendu. Si l'écran était verrouillé, les cadences et les latences pourraient différer de celles d'une session déverrouillée ; à confirmer devant l'écran.
- **Événements injectés par CDP** : la molette et les touches sont des événements CDP, pas un vrai périphérique ; le glisser est simulé par `scrollTop` à chaque image (comme `e2e/scroll.spec.ts`).
- **Port de débogage** : `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` remplace les arguments que wry passe à WebView2. La webview mesurée peut donc différer un peu de celle d'un lancement normal.
- **`t15` par le vrai cœur** : le faux `claude.exe` est un script Node lancé par un `.cmd` (`cmd.exe` puis `node.exe` sous le Job Object). Les lignes de `t15` arrivent aussi vite que le tuyau les prend, sans les délais réels entre les tours. L'interface compte 2 721 lignes de rendu pour `t15`.
- **Latence par le vrai cœur** : elle couvre la lecture par Rust, les lots de 16 ms, le canal et la peinture, comme le définit le plan, mais pas l'écriture du faux CLI.
- **Démarrage** : les durées partent du lancement du processus par le script (pas d'un double-clic) ; le premier démarrage de la série n'était pas un démarrage à froid de Windows. La part de `claude.exe` (3,2 à 4,0 s) comprend les hooks et les serveurs MCP de la configuration utilisateur chargés dans le bac à sable.
- **Mémoire** : un instantané par étape, pas de suivi sur 4 h ; la croissance sur 4 h n'est pas mesurée.
- **MSI** : vérifié par extraction, pas installé (élévation impossible sans l'utilisateur) ; le NSIS a été installé, démarré et désinstallé.
- **Fidélité** : la liste de contrôle de `docs/reference-tui/README.md` reste à valider par l'utilisateur.
- **Emplacement du tableau** : le plan prévoyait le tableau des mesures dans `docs/reference-tui/README.md` ; il est dans ce rapport.
- **Transcripts** : la session des essais réels a écrit un transcript dans le dossier de projet du bac à sable, sous `~/.claude/projects/` (dossier encodé de `.claude-work/j12/sandbox`, un fichier `.jsonl` de 473 Ko). Il est laissé en place. `sessions_live` écrit aussi dans le dossier de projet de `j11-sandbox`.

## 5. Points restants

1. **Latence d'un token, p50** : 25 à 28 ms dans WebView2, pour un seuil de 25 ms (22 ms dans Chromium sur Vite). Cause probable : à 100 deltas/s, le flux n'est jamais « calme », donc chaque lot attend sa fenêtre de 16 ms (plan 1.5), puis l'image suivante à 60 Hz. Cela fait un plancher d'environ une image et demie, et WebView2 ajoute quelques millisecondes par rapport à Chromium. Pistes : envoyer tout de suite le premier delta d'un lot, ou raccourcir la fenêtre pendant un `text_delta`. Sinon, revoir le seuil.
2. **Ajout d'un message, instable en simulé** : une série de 5 envois en mode simulé est montée jusqu'à 110 ms, alors que la série suivante est restée sous 41 ms et que la série par le vrai cœur est restée sous 26 ms. Les envois de cette série se superposaient aux réponses simulées encore en cours de streaming (chaque envoi simulé lance une réponse). À remesurer devant l'écran avec une trace de performance, pour savoir si c'est du travail de mise en page ou une image retardée.
3. **Démarrage** : un démarrage sur cinq a peint sa fenêtre en 1 510 ms (seuil 1,5 s) ; la médiane est de 1 115 ms. À surveiller.
4. **`replied_in_chat`** : dans `question_live`, Claude reçoit le même texte que pour `dismissed` (« The user did not answer the questions. »). Le test passe, mais la nuance « répondu dans la conversation » n'atteint pas Claude. À confirmer avec ce que fait le TUI.
5. **Croissance mémoire sur 4 h** : non mesurée. La mémoire monte de 185 à 305 Mo après environ 25 s de streaming simulé, alors que le tas JS reste à 13 Mo : la hausse est côté moteur de rendu. Il faut une session longue réelle pour savoir si elle se stabilise.
6. **Vérification manuelle du plan (1.10, point 7)** : une session réelle d'une heure, un orchestrateur avec 4 agents, et la mort forcée de `claude.exe` depuis le Gestionnaire des tâches restent à faire par l'utilisateur. Les essais ci-dessus couvrent l'interruption, le changement de modèle, la question, la reprise et 2 agents.
7. **Fidélité** : validation par l'utilisateur de la liste de contrôle de `docs/reference-tui/README.md`.
8. **MSI** : installation réelle à faire par l'utilisateur (élévation).
9. **Script de mesure de `e2e/scroll.spec.ts`** : un `stop()` suivi d'un `start()` dans la même image laisse tourner deux boucles d'enregistrement. La mesure à la molette compte alors environ 1 200 images sur « 20 s » au lieu de 600 sur 10 s. La cadence calculée reste juste, mais le nombre d'images est doublé. Ce défaut est corrigé dans `tools/recette/perf.mjs` ; `e2e/` est hors du périmètre de cette recette.
