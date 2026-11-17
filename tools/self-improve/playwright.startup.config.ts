/// <reference types="node" />
// Sous-ensemble e2e « de démarrage » lancé par le cycle d'auto-amélioration
// (J14, décision 17 ; `src-tauri/src/selfupdate/pipeline.rs`, `NpmToolchain`).
// Port dédié lu dans `ORCH_E2E_PORT` (1431 par défaut) : ne partage ni le
// port de `tauri dev` (1420) ni celui des e2e de développement (1430), et ne
// réutilise jamais un serveur déjà ouvert (il pourrait servir un autre arbre).
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "@playwright/test";

const PORT = Number(process.env.ORCH_E2E_PORT ?? 1431);
const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");

export default defineConfig({
  testDir: join(ROOT, "e2e"),
  testMatch: ["startup.spec.ts"],
  workers: 1,
  fullyParallel: false,
  retries: 0,
  timeout: 120_000,
  reporter: [["line"]],
  outputDir: join(tmpdir(), `orchestrator-self-improve-e2e-${PORT}`),
  use: {
    baseURL: `http://localhost:${PORT}`,
    browserName: "chromium",
    viewport: { width: 1100, height: 760 },
  },
  webServer: {
    command: `node node_modules/vite/bin/vite.js --port ${PORT} --strictPort`,
    cwd: ROOT,
    url: `http://localhost:${PORT}/?mock=demo`,
    reuseExistingServer: false,
    timeout: 240_000,
  },
});
