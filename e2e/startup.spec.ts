/// <reference types="node" />
// Démarrage de l'app entière (J11 branché, finitions du prototype 1) :
// sélecteurs de projet et de session, nouvelle session, reprise avec
// historique, échec du démarrage (réessayer, revenir aux sélecteurs), et
// Ctrl+O sans effet sur un sélecteur. `App.svelte` est monté tel quel, l'IPC
// Tauri est simulé (`startup.harness.ts`).
import { expect, test, type Page } from "@playwright/test";

const HARNESS = "/__startup-harness.html";
const PROMPT = 'textarea[aria-label="Message"]';

async function open(page: Page, cfg: Record<string, unknown>) {
  await page.route(`**${HARNESS}`, (route) =>
    route.fulfill({
      contentType: "text/html",
      body: `<!doctype html><html><head><meta charset="utf-8"></head><body><div id="app"></div>
        <script>window.__cfg=${JSON.stringify(cfg)}</script>
        <script type="module" src="/e2e/startup.harness.ts"></script></body></html>`,
    }),
  );
  await page.goto(HARNESS);
}

type Call = { cmd: string; args: Record<string, unknown> };
const calls = (p: Page) => p.evaluate(() => (window as unknown as { __calls: Call[] }).__calls);
const cmds = async (p: Page) => (await calls(p)).map((c) => c.cmd);
const starts = async (p: Page) => (await calls(p)).filter((c) => c.cmd === "start_session").map((c) => c.args);

/** Projet puis sélecteur des sessions, au clavier. */
async function toSessions(page: Page) {
  await expect(page.getByTestId("project-picker")).toContainText("proj");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("session-picker")).toContainText("Ma session");
}

test("sans argument : projets, sessions, retour, reprise avec historique", async ({ page }) => {
  await open(page, { fromArgs: false, launchError: "Dossier introuvable : D:\\absent" });
  await expect(page.getByTestId("launch-error")).toHaveText("Dossier introuvable : D:\\absent");
  await toSessions(page);
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("project-picker")).toBeVisible();
  await toSessions(page);
  // Entrée 0 = nouvelle session, 1 = « Ma session ».
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  await expect(page.locator('[data-row-kind="user"]')).toContainText("bonjour 1");
  expect(await cmds(page)).toEqual([
    "get_launch_info",
    "list_projects",
    "list_sessions",
    "list_projects",
    "list_sessions",
    "start_session",
    "load_history",
  ]);
  expect(await starts(page)).toEqual([{ cwd: "C:\\proj", resume: "00000000-0000-4000-8000-000000000001" }]);
  await expect(page.locator(PROMPT)).toBeEnabled();
});

test("nouvelle session depuis le sélecteur", async ({ page }) => {
  await open(page, { fromArgs: false, launchError: null });
  await expect(page.getByTestId("launch-error")).toHaveCount(0);
  await toSessions(page);
  await page.keyboard.press("Enter");
  await expect(page.locator(PROMPT)).toBeEnabled();
  expect(await starts(page)).toEqual([{ cwd: "C:\\proj", resume: null }]);
});

test("avec argument : session directe, sans sélecteur", async ({ page }) => {
  await open(page, { fromArgs: true, launchError: null });
  await expect(page.locator(PROMPT)).toBeEnabled();
  expect(await cmds(page)).toEqual(["get_launch_info", "start_session"]);
  await expect(page.getByTestId("project-picker")).toHaveCount(0);
});

test("historique illisible : bandeau, la session continue", async ({ page }) => {
  await open(page, { fromArgs: false, launchError: null, historyError: true });
  await toSessions(page);
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  await expect(page.locator(".banner")).toContainText("Session reprise sans historique affiché");
  await expect(page.locator(PROMPT)).toBeEnabled();
});

test("historique tronqué : ligne grise en tête du fil, qui défile avec lui", async ({ page }) => {
  await open(page, { fromArgs: false, launchError: null, omitted: 1200 });
  await toSessions(page);
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  const cut = page.getByTestId("plain-row");
  await expect(cut).toHaveText("  ⎿  Début de l'historique omis (1200 lignes)");
  expect(await cut.evaluate((e) => getComputedStyle(e).color)).toBe("rgb(153, 153, 153)");
  // Dans la liste virtualisée (et non au-dessus d'elle), avant l'historique.
  const kinds = await page.locator(".vlist [data-row-kind]").evaluateAll((els) => els.map((e) => e.getAttribute("data-row-kind")));
  expect(kinds).toEqual(["plain", "user"]);
});

test("échec du démarrage : « Réessayer » relance la même demande", async ({ page }) => {
  await open(page, { fromArgs: true, launchError: null, failStarts: 1 });
  await expect(page.getByTestId("start-failed")).toBeVisible();
  await expect(page.locator(".banner.error")).toContainText("claude.exe absent (simulé)");
  await expect(page.locator(PROMPT)).toBeDisabled();
  await page.getByTestId("start-retry").click();
  await expect(page.locator(PROMPT)).toBeEnabled();
  await expect(page.getByTestId("start-failed")).toHaveCount(0);
  await expect(page.locator(".banner.error")).toHaveCount(0);
  expect(await starts(page)).toEqual([
    { cwd: "C:\\cwd", resume: null },
    { cwd: "C:\\cwd", resume: null },
  ]);
});

test("échec d'une reprise : retour aux sélecteurs, autre session reprise sur un magasin neuf", async ({ page }) => {
  await open(page, { fromArgs: false, launchError: null, failStarts: 1 });
  await toSessions(page);
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("start-failed")).toBeVisible();
  await page.getByTestId("start-back").click();
  await expect(page.getByTestId("session-picker")).toContainText("Autre session");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  await expect(page.locator('[data-row-kind="user"]')).toHaveText(/bonjour 2/);
  await expect(page.locator('[data-row-kind="user"]')).toHaveCount(1);
  expect(await starts(page)).toEqual([
    { cwd: "C:\\proj", resume: "00000000-0000-4000-8000-000000000001" },
    { cwd: "C:\\proj", resume: "00000000-0000-4000-8000-000000000002" },
  ]);
  // Historique ↑ : seulement les messages de la session reprise.
  await expect(page.locator(PROMPT)).toBeEnabled();
  await page.locator(PROMPT).focus();
  await page.keyboard.press("ArrowUp");
  await expect(page.locator(PROMPT)).toHaveValue("bonjour 2");
  await page.keyboard.press("ArrowUp");
  await expect(page.locator(PROMPT)).toHaveValue("bonjour 2");
});

test("Ctrl+O avec un sélecteur ouvert : sans effet", async ({ page }) => {
  await open(page, { fromArgs: false, launchError: null });
  await expect(page.getByTestId("project-picker")).toContainText("proj");
  await page.keyboard.press("Control+o");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("session-picker")).toContainText("Ma session");
  await page.keyboard.press("Control+o");
  await page.keyboard.press("Enter");
  await expect(page.locator(PROMPT)).toBeEnabled();
  // Vue normale : ni barre de la vue détaillée, ni saisie masquée.
  await expect(page.getByTestId("detail-bar")).toHaveCount(0);
});
