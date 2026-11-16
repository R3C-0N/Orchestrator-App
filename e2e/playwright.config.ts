/// <reference types="node" />
// Mesures de recette de l'interface (plan 1.10 point 6, 1.11) sur Vite en
// mode simulé. Chromium (≈ moteur de WebView2). Un seul worker : les mesures
// de cadence et de mémoire ne doivent pas se partager la machine.
import { tmpdir } from "node:os";
import { join } from "node:path";
import { defineConfig } from "@playwright/test";

const PORT = 1430;

export default defineConfig({
  testDir: ".",
  testMatch: "*.spec.ts",
  workers: 1,
  fullyParallel: false,
  timeout: 120_000,
  reporter: [["list"]],
  // Résultats hors du dépôt.
  outputDir: join(tmpdir(), "orchestrator-e2e"),
  use: {
    baseURL: `http://localhost:${PORT}`,
    browserName: "chromium",
    // Fenêtre de la taille du terminal de référence (≈ 120 × 40).
    viewport: { width: 1100, height: 760 },
  },
  webServer: {
    // Port distinct de celui de `tauri dev` (1420), pour cohabiter.
    command: `npx vite --port ${PORT} --strictPort`,
    cwd: "..",
    url: `http://localhost:${PORT}/?mock=demo`,
    // Réutilisé s'il tourne déjà (machine chargée : l'optimisation des
    // dépendances de Vite peut dépasser une minute).
    reuseExistingServer: true,
    timeout: 240_000,
  },
});
