/// <reference types="node" />
// Onglets de conversation (`TabBar.svelte`, spec décision 21) : « + » ouvre
// un onglet sur le sélecteur des projets, la nouvelle conversation démarre à
// côté de la première sans l'arrêter, chaque onglet garde sa saisie et sa
// session, et fermer un onglet arrête sa session. `App.svelte` est monté tel
// quel, l'IPC Tauri est simulé (`startup.harness.ts`).
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
const of = async (p: Page, cmd: string) => (await calls(p)).filter((c) => c.cmd === cmd).map((c) => c.args);

test("« + », deux conversations côte à côte, saisie gardée par onglet, fermeture", async ({ page }) => {
  await open(page, { fromArgs: true, launchError: null });
  const tabs = page.getByTestId("tab");
  await expect(page.locator(PROMPT)).toBeEnabled();
  await expect(tabs).toHaveCount(1);
  // Un seul onglet : pas de croix.
  await expect(page.getByTestId("tab-close")).toHaveCount(0);
  await page.locator(PROMPT).fill("brouillon du premier");

  // « + » : nouvel onglet sur le sélecteur des projets.
  await page.getByTestId("tab-new").click();
  await expect(tabs).toHaveCount(2);
  await expect(tabs.nth(1)).toContainText("Nouvel onglet");
  await expect(tabs.nth(1).getByRole("tab")).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("project-picker")).toContainText("proj");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("session-picker")).toContainText("Ma session");
  await expect(tabs.nth(1)).toContainText("proj");
  await page.keyboard.press("Enter");
  await expect(page.locator(PROMPT)).toBeEnabled();
  await expect(page.locator(PROMPT)).toHaveValue("");
  expect((await of(page, "start_session")).length).toBe(2);
  // La première conversation n'est pas arrêtée.
  expect(await of(page, "stop_session")).toEqual([]);

  // Retour au premier onglet : sa saisie, et ses envois vont à sa session.
  await tabs.nth(0).getByRole("tab").click();
  await expect(page.locator(PROMPT)).toHaveValue("brouillon du premier");
  await page.locator(PROMPT).press("Enter");
  await expect.poll(async () => (await of(page, "send_message")).map((a) => a.session)).toEqual(["k1"]);

  // Fermer le deuxième onglet arrête sa session ; le premier reste affiché.
  await tabs.nth(1).getByTestId("tab-close").click();
  await expect(tabs).toHaveCount(1);
  await expect.poll(async () => (await of(page, "stop_session")).map((a) => a.session)).toEqual(["k2"]);
  await expect(page.locator(PROMPT)).toBeEnabled();
});
