# Amélioration de l'app Orchestrator

Tu travailles sur le code source de l'app Orchestrator elle-même, dans un worktree git qui lui est réservé :

- dossier : `{{DOSSIER}}`
- branche : `{{BRANCHE}}` (créée depuis le `main` du clone de l'app)

## Demande de l'utilisateur

{{DEMANDE}}

## Ce que tu dois faire

1. Lis le code concerné avant de modifier (`docs/spec.md` pour les décisions, `CLAUDE.md` s'il existe).
2. Fais la modification la plus petite qui répond à la demande. Garde le style du code autour.
3. Si tu changes un comportement, ajoute ou adapte un test (`e2e/unit/` pour l'interface).
4. Vérifie si tu peux : `npm run check` puis `npm test`.
5. Commite **uniquement les fichiers que tu as touchés**, en les nommant :
   `git commit -m "<message en français>" -- <fichier1> <fichier2>`
   (ou `git commit -F <fichier de message> -- <fichiers>`). Un ou plusieurs commits, tous sur la branche `{{BRANCHE}}`.
   Si le message contient le mot « push », écris-le dans un fichier et passe-le par `git commit -F <fichier> -- <fichiers>` : certaines commandes git qui contiennent « push » sont refusées. N'utilise pas `git -c` ni `git -C` (les commandes tournent déjà dans le worktree).
6. Termine avec un arbre propre : `git status --porcelain` ne doit rien afficher. Supprime tes fichiers temporaires, ne laisse rien de non commité.

## Périmètre

Tu ne peux modifier que des fichiers sous : {{DOSSIERS}}.
Tout fichier hors de ces dossiers (`package.json`, `package-lock.json`, `README.md`, fichiers à la racine…) fait **refuser** l'amélioration entière.

Une modification de `src-tauri/`, de `tauri.conf.json` ou du contrat entre l'interface et le cœur (`src/lib/core/bindings/`, `src-tauri/capabilities/default.json`, `src-tauri/build.rs`) n'est pas activée tout de suite : elle demande de recompiler l'exe. Préfère une solution dans l'interface (`src/`) quand c'est possible.

Le harnais de test est protégé : ne modifie ni ne supprime aucun test existant (`e2e/`, `src-tauri/tests/`), ni `tools/self-improve/`, `tools/contract-fingerprint.mjs` ou `src-tauri/src/selfupdate/`. Tu peux **ajouter** de nouveaux fichiers de test, qui s'exécuteront avec les autres. N'ajoute jamais `.skip(`, `.only(`, `.fixme(`, `test.fail` ou `describe.skip` dans un fichier de code JS/TS/Svelte, ni `#[ignore]` dans un `.rs` : l'amélioration serait refusée (la documentation, qui cite ces marqueurs, n'est pas concernée). Les tests sont relancés par le cœur sur une copie où le harnais d'origine est restauré.

Ne crée ni lien symbolique, ni sous-module, ni `.gitattributes`. Ne modifie pas la configuration git (`git config`, `git remote`, hooks).

## Interdits

- Ne pousse jamais rien (`git push` interdit) ; ne touche pas aux dépôts distants.
- Ne change pas de branche, ne crée pas d'autre branche, ne modifie pas `main`.
- Pas de `git rebase`, `git reset`, `git stash`, `git commit --amend`, `git checkout -- <fichier>`, `git clean`.
- Pas de `git add -A`, `git add .` ni `git commit -a` : nomme toujours les fichiers.
- N'installe ni ne mets à jour aucune dépendance (`npm install`, `cargo add`…).
- Ne sors pas du dossier du worktree ; n'écris rien dans `~/.claude` ni ailleurs sur la machine.
- Ne lance pas `npm run tauri build`, ni l'app, ni de serveur qui reste ouvert.

Tu travailles sans utilisateur pour répondre : ne pose pas de question. Si la demande est ambiguë, choisis l'interprétation la plus prudente et explique ton choix dans le message de commit. Si la demande est impossible dans ce périmètre, ne commite rien et explique pourquoi dans ta dernière réponse.

Après ta session, le cœur de l'app vérifie tout cela, lance les tests, construit l'interface, la recharge et fusionne la branche en fast-forward. Tu n'as pas à le faire toi-même.
