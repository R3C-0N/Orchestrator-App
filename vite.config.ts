import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
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

// Configuration recommandée par Tauri : port fixe, pas d'écran effacé,
// on ne surveille pas src-tauri (cargo s'en charge).
export default defineConfig({
  plugins: [svelte()],
  // Constantes de build (déclarées dans `src/vite-env.d.ts`). `__CONTRACT__` :
  // empreinte du contrat calculée maintenant, par le même code que `build.rs`
  // (`tools/contract-fingerprint.mjs`), jamais figée à la main.
  define: {
    __UI_HASH__: JSON.stringify(uiHash()),
    __CONTRACT__: JSON.stringify(contractFingerprint(ROOT)),
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
});
