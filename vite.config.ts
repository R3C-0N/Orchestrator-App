/// <reference types="vitest/config" />
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { contractFingerprint } from "./tools/contract-fingerprint.mjs";

/** Racine du dépôt (dossier de ce fichier). */
const ROOT = fileURLToPath(new URL(".", import.meta.url));

/**
 * Hash de l'interface (J14, décision 15) : `ORCH_UI_HASH` (posé par le cycle
 * d'amélioration, = commit de la branche), sinon le commit courant du dépôt,
 * sinon `dev`. Calculé au lancement de Vite (build ou serveur).
 */
function uiHash(): string {
  const env = process.env.ORCH_UI_HASH?.trim();
  if (env) return env;
  try {
    const h = execFileSync("git", ["rev-parse", "--short", "HEAD"], {
      cwd: ROOT,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
      timeout: 5000,
    }).trim();
    if (/^[0-9a-f]{4,40}$/.test(h)) return h;
  } catch {
    // Pas de git ou pas de dépôt : version de développement.
  }
  return "dev";
}

/**
 * `contract.json` à la racine du build : `{ contract, commit }` (J14,
 * relecture R2). Le cœur le lit avant d'activer une interface (empreinte
 * du contrat de l'exe), avec en repli la recherche de `__CONTRACT__` dans
 * les scripts.
 */
function contractFile(contract: string, commit: string): Plugin {
  return {
    name: "orchestrator-contract",
    apply: "build",
    generateBundle() {
      this.emitFile({
        type: "asset",
        fileName: "contract.json",
        source: JSON.stringify({ contract, commit }) + "\n",
      });
    },
  };
}

const CONTRACT = contractFingerprint(ROOT);
const UI_HASH = uiHash();

// Configuration recommandée par Tauri : port fixe, pas d'écran effacé,
// on ne surveille pas src-tauri (cargo s'en charge).
export default defineConfig({
  plugins: [svelte(), contractFile(CONTRACT, UI_HASH)],
  // Constantes de build (déclarées dans `src/vite-env.d.ts`). `__CONTRACT__` :
  // empreinte du contrat calculée maintenant, par le même code que `build.rs`
  // (`tools/contract-fingerprint.mjs`), jamais figée à la main.
  define: {
    __UI_HASH__: JSON.stringify(UI_HASH),
    __CONTRACT__: JSON.stringify(CONTRACT),
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    target: "es2022",
  },
  // Tests unitaires (`npm test`). Le premier import d'un composant Svelte
  // dans un arbre neuf (worktree de tests de `/ameliorer`, sans cache Vite)
  // compile à froid : mesuré à 4,8-4,9 s sur main machine au repos, au ras
  // des 5 s par défaut, et 14,6 s avec les 12 cœurs occupés (cargo en
  // parallèle). Borne relevée, pas supprimée : un vrai blocage échoue encore.
  test: {
    testTimeout: 60_000,
    hookTimeout: 60_000,
  },
});
