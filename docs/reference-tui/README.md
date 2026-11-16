# Référence de fidélité : le TUI de Claude Code 2.1.325

Ce dossier décrit la référence visuelle du prototype 1 : décision 3 de la spec (`docs/spec.md`), section 1.7 et jalons J1, J5, J7 et J8 du plan (`docs/plan-prototype-1.md`). Les captures ont été faites au jalon J1 (commit `64d6717`) par `tools/capture-tui/capture.ps1`.

## Où sont les captures

Elles sont **locales et non versionnées**, dans `.claude-work/j1/captures/`, un dossier ignoré par git. Elles montrent le chemin du bac à sable de capture, qui contient le nom d'utilisateur, et une image ne se vérifie pas par `grep`.

Chaque capture existe en trois fichiers :

| Fichier | Contenu | Fait foi pour |
| - | - | - |
| `<id>-<nom>.png` | la fenêtre Windows Terminal seule | couleurs, fonds, gras, soulignés |
| `<id>-<nom>.uia.txt` | le texte visible au même instant, lu par UI Automation | glyphes et textes exacts, colonnes |
| `<id>-<nom>.txt` | la transcription, avec en tête les couleurs et styles relevés sur le PNG | couleurs nommées, interprétation |

Conditions de capture :

- Windows Terminal en 120 colonnes × 40 lignes, Cascadia Mono 12 pt, jeu de couleurs Campbell (fond `#0C0C0C`) ;
- thème `dark` de Claude Code, rendu **`fullscreen`** (la saisie est collée en bas de la fenêtre), modèle `haiku` ;
- mode de permission `auto` (« ⏵⏵ auto mode on »), et non `bypassPermissions` comme dans l'app.

Les éléments suivants sont propres au poste de capture et **ne font pas partie de la référence** :

- la ligne d'état personnalisée de l'utilisateur, avec ses emojis (une `statusLine` réglée sur le poste) ;
- le chemin du projet sous le logo ;
- la ligne « ⏵⏵ auto mode on ».

## Refaire une capture

```powershell
pwsh tools/capture-tui/capture.ps1 -Only <n>          # ex. -Only 05, -Only 05,06, -Only 03b
pwsh tools/capture-tui/capture.ps1                    # les 10 captures de référence (avec 03b et 09b)
pwsh tools/capture-tui/capture.ps1 -Scenario ctrlc,echap,ctrlc-tour   # captures d'observation c*, e*, k*
pwsh tools/capture-tui/capture.ps1 -List              # liste, sans rien lancer
```

Le script cède le clavier environ une minute par scénario : les touches partent vers la fenêtre ouverte, **et seulement si elle est au premier plan**. Détails et limites dans `tools/capture-tui/README.md`. Si un lancement en rendu `fullscreen` est interrompu, le suivant peut retomber sur le rendu classique (bandeau « fullscreen renderer didn't finish starting ») : refaire alors la capture avec `-Only`.

## Les 10 captures de référence et leurs variantes

| Id | Fichier | Ce qu'elle montre |
| - | - | - |
| 01 | `01-accueil` | logo `▐▛███▛█` orange saumon, « Claude Code v2.1.325 », modèle ; saisie vide `❯` entre deux règles `─` grises, texte d'exemple gris ; « ◐ medium · /effort » à droite au-dessus de la saisie |
| 02 | `02-markdown-code` | message utilisateur en blanc gras sur une bande gris foncé ; réponse avec puce `●` blanche et retrait de 2 ; titres en gras sans changement de taille ; listes ; code en ligne bleu lavande ; bloc de code coloré sans cadre ; « ✻ Crunched for 6s · done 12:20 » |
| 03 | `03-outils-tronque` | Read et Bash **regroupés** en une ligne grise « Read 1 file, ran 1 shell command », sans puce ni résultat |
| 03b | `03b-outils-detail-ctrl-o` | la même session en **vue détaillée (Ctrl+O)** : sortie complète, « ⎿  Allowed by auto mode », heure et modèle alignés à droite, barre « Showing detailed transcript · ctrl+o to toggle · … » |
| 04 | `04-edit-diff` | « Searched for 1 pattern, read 1 file » ; `●` vert + « Update(config.py) » ; « ⎿  Added 3 lines, removed 2 lines » ; diff numéroté, fonds rouge et vert, 3 lignes de contexte |
| 05 | `05-agents-en-cours` | « ● 2 background agents launched (↓ to manage) », sous-lignes `├` / `└`, nom de l'agent sur fond de sa couleur ; « ✻ Waiting for 2 background agents to finish » ; panneau des agents sous la ligne d'état (« ● main », « ◯ agent-bleu  description  6s · ↓ 39.0k tokens ») |
| 06 | `06-agents-termines` | « › Message from @agent-rouge (ctrl+o to expand) » ; « ● Agent "Ping rouge" finished · 42s » ; « ✻ Worked for 53s · done 12:31 » ; le panneau des agents a disparu |
| 07 | `07-question` | AskUserQuestion **sans encadré** : règle `─`, pastille « ☐ Langage », question en gras, `❯ 1. Rust` en bleu lavande, descriptions grises, « 4. Type something. », règle, « 5. Chat about this », pied « Enter to select · ↑/↓ to navigate · Esc to cancel » |
| 08 | `08-interruption` | Échap pendant un Bash : « No result recorded for 1 call », « ⎿  $ ping -n 90 127.0.0.1 », « ⎿  Interrupted · What should Claude do instead? » en **gris** |
| 09 | `09-erreurs` | Edit en erreur : `●` rouge + « Update(config.py) », « ⎿  Error editing file » rouge ; les 2 Bash en erreur regroupés en « Ran 2 shell commands » gris |
| 09b | `09b-erreurs-detail-ctrl-o` | les mêmes erreurs en vue détaillée : un appel par ligne, `●` rouges, « ⎿  Error: Exit code 1 » et stderr en rouge, chemin absolu de l'Edit |
| 10 | `10-menu-slash` | menu `/` **au-dessus** de la saisie, sans cadre ; entrée choisie `❯ /loop` en bleu lavande ; descriptions alignées en colonne, repliées puis tronquées par `…` |

Captures d'observation du clavier (hors des 10, scénarios `ctrlc`, `echap`, `ctrlc-tour`) :

| Id | Fichier | Ce qu'elle montre |
| - | - | - |
| c1 | `c1-saisie-avant-ctrlc` | état de départ : saisie « bonjour le monde » au repos |
| c2 | `c2-ctrlc-saisie-non-vide` | Ctrl+C au repos sur une saisie non vide : la saisie est **effacée** |
| c3 | `c3-ctrlc-saisie-vide-repos` | Ctrl+C au repos sur une saisie vide : « Press Ctrl-C again to exit » sous la saisie |
| c4 | `c4-ctrlc-saisie-non-vide-pendant-tour` | Ctrl+C pendant un tour avec « brouillon » dans la saisie : le tour est **interrompu** et la saisie **gardée** |
| c5 | `c5-ctrlc-saisie-vide-pendant-tour` | Ctrl+C pendant un tour, saisie vide : interruption, même rendu que 08 |
| c6 | `c6-ctrlc-apres` | état après la séquence : session utilisable, saisie vide |
| e1 | `e1-echap-saisie-non-vide` | Échap au repos sur une saisie non vide : « Esc again to clear » à droite, au-dessus de la saisie ; le texte reste |
| e2 | `e2-echap-echap-saisie-non-vide` | second Échap : la capture montre encore le texte et l'indication (voir « Incertain » plus bas) |
| e3 | `e3-echap-echap-saisie-vide` | double Échap sur une saisie vide juste après la séquence : rien de visible à cet instant |
| e4 | `e4-echap-apres` | état après la séquence |
| k1 | `k1-tour-en-cours-saisie-vide` | indicateur pendant un tour : « ✽ Choreographing… (8s · ↓ 255 tokens) » ; appel en cours « Running ping for 90 requests to localhost · 3s » et « ⎿  $ ping … » |
| k2 | `k2-ctrlc-saisie-vide-pendant-tour` | Ctrl+C pendant un tour : même résultat que 08 (interruption grise) |
| k3 | `k3-echap-echap-saisie-vide-apres-tour` | double Échap sur une saisie vide après un tour : **menu Rewind** (« Restore the code and/or conversation… ») ; hors du prototype 1 |
| k4 | `k4-echap-apres` | état après fermeture du menu Rewind |

Incertain : en `e2`, le second Échap n'a pas effacé la saisie au moment de la capture. Soit l'intervalle entre les deux touches envoyées par `SendKeys` dépassait la fenêtre d'environ 0,5 s, soit la capture est arrivée avant le rafraîchissement. La règle retenue (un second Échap rapproché efface la saisie) vient du comportement connu du TUI ; il faudra la confirmer en refaisant la capture `-Scenario echap` avec un intervalle plus court.

## Liste de contrôle de fidélité (jalon J5)

Pour la recette, l'app rejoue la même situation (fixture ou session réelle) en 120 colonnes, à côté du PNG. On compare les éléments, pas les pixels. Chaque ligne cochée par l'utilisateur fait avancer le critère « fidélité » du plan (section 1.11).

Grille et couleurs :

- [ ] Police mono, une ligne de texte par ligne de grille, aucune bulle, carte ni ombre (toutes).
- [ ] Fond noir `#0C0C0C`, texte blanc ; gris pour les éléments secondaires ; bleu lavande (≈ `#B1B9F9`) pour le code en ligne et les sélections (02, 07, 10).

Messages :

- [ ] Message utilisateur : `❯` gris, texte blanc gras sur une bande gris foncé (≈ `#373737`) pleine largeur, lignes suivantes en retrait de 2, markdown rendu (02).
- [ ] Réponse : puce `●` blanche en colonne 0, texte en retrait de 2 (02).
- [ ] Titres `##` et `###` en gras blanc, même taille, sans `#` (02).
- [ ] Listes `-` et `1.` telles quelles ; gras et italique ; code en ligne bleu lavande sans fond (02).
- [ ] Bloc de code sans cadre ni fond, retrait de 2, coloré : mots-clés bleus, fonctions jaunes, chaînes rouges, nombres verts (02).
- [ ] Fin de tour : « ✻ <Verbe> for <durée> · done <HH:MM> » en gris (02, 04, 06, 09).

Outils :

- [ ] Vue normale : appels regroupés en une ligne grise sans puce, comme « Read 1 file, ran 1 shell command », « Searched for 1 pattern, read 1 file » ou « Ran 2 shell commands » (03, 04, 09).
- [ ] Edit et Write restent sur leur propre ligne : `●` vert + « Update(<chemin relatif>) », « Update » en gras (04).
- [ ] Résultat : `  ⎿  ` (2 espaces, `⎿`, 2 espaces) en gris (04, 08, 09).
- [ ] Ctrl+O bascule vers la vue détaillée : un appel par ligne avec sa sortie complète, heure et modèle à droite, barre « Showing detailed transcript · ctrl+o to toggle… » ; Ctrl+O revient à la vue normale (03b, 09b).

Diff :

- [ ] « ⎿  Added N lines, removed M lines », avec les nombres en gras (04).
- [ ] Numéros alignés à droite sur 6 colonnes, puis espace, `-` ou `+` (04).
- [ ] Suppressions sur fond rouge sombre, numérotées dans l'**ancien** fichier ; ajouts sur fond vert sombre, numérotés dans le **nouveau** fichier (04).
- [ ] 3 lignes de contexte avant et après, en coloration syntaxique (04).

Erreurs et interruption :

- [ ] Outil en erreur sur sa ligne : `●` rouge (≈ `#FF6B80`), « ⎿  Error editing file » rouge (09) ; en vue détaillée, message d'erreur et stderr en rouge (09b).
- [ ] Interruption : « No result recorded for N call », « ⎿  $ <commande> », « ⎿  Interrupted · What should Claude do instead? », le tout en gris, pas en rouge (08).

Activité :

- [ ] Pendant le tour : « <glyphe animé> <Verbe>… (<durée> · ↓ <N> tokens) » au-dessus de la saisie (k1).
- [ ] Appel en cours : libellé gris avec sa durée, puis « ⎿  $ <commande> » (k1).

Sous-agents :

- [ ] « ● N background agents launched (↓ to manage) », puis une sous-ligne `├` / `└` par agent, nom sur fond de sa couleur (texte noir), description entre parenthèses (05).
- [ ] « ✻ Waiting for N background agents to finish » tant qu'ils tournent (05).
- [ ] Panneau des agents : « ● main », puis « ◯ <agent>  <description> … <durée> · ↓ <tokens> tokens » ; dans l'app, la colonne de droite reprend ce style (décision 4) (05).
- [ ] « › Message from @<agent> (ctrl+o to expand) » en gris ; « ● Agent "<description>" finished · <durée> » ; pas de « Done (N tool uses…) » (06).

Question :

- [ ] Pas d'encadré : règle `─`, pastille « ☐ <header> » (texte noir sur bleu lavande), question en gras (07).
- [ ] Options `❯ 1. …` : sélection en bleu lavande, descriptions grises en retrait de 5 ; puis « N. Type something. », règle, « N+1. Chat about this » (07).
- [ ] Pied « Enter to select · ↑/↓ to navigate · Esc to cancel » (07).

Saisie et menu `/` :

- [ ] Saisie : `❯` entre deux règles `─` pleine largeur, collée en bas (01).
- [ ] Menu `/` au-dessus de la saisie, sans cadre ; entrée choisie `❯` en bleu lavande ; descriptions alignées en colonne, repliées puis tronquées par `…` (10).
- [ ] Indications clavier : « Esc again to clear » à droite au-dessus de la saisie (e1) ; pas de « Press Ctrl-C again to exit » dans l'app (choix du prototype 1, décision 13).
