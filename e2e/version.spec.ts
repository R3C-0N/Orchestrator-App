/// <reference types="node" />
// Bandeau « version non testée » et sa croix : fermé à la souris ou au
// clavier, il reste masqué pour cette version après rechargement, et revient
// quand la version simulée change. `App.svelte` monté tel quel, IPC simulé
// (`startup.harness.ts`, option `version`).
import { expect, test, type Page } from "@playwright/test";

const HARNESS = "/__startup-harness.html";
const PROMPT = 'textarea[aria-label="Message"]';
const NOTICE = "version-notice";
const CLOSE = "version-notice-close";

async function open(page: Page, version: string) {
  await page.unroute(`**${HARNESS}`);
  const cfg = { fromArgs: true, launchError: null, version };
  await page.route(`**${HARNESS}`, (route) =>
    route.fulfill({
      contentType: "text/html",
      body: `<!doctype html><html><head><meta charset="utf-8"></head><body><div id="app"></div>
        <script>window.__cfg=${JSON.stringify(cfg)}</script>
        <script type="module" src="/e2e/startup.harness.ts"></script></body></html>`,
    }),
  );
  await page.goto(HARNESS);
  await ready(page);
}

/** Le lot du faux cœur (`status` Idle, `init`, notice éventuelle) est appliqué. */
async function ready(page: Page) {
  await expect(page.locator(PROMPT)).toBeEnabled();
  await expect(page.locator("footer")).toContainText("Idle");
}

const stored = (p: Page) => p.evaluate(() => localStorage.getItem("orchestrator.versionNotice.dismissed"));

test("version testée : pas de bandeau", async ({ page }) => {
  await open(page, "2.1.325");
  await expect(page.getByTestId(NOTICE)).toHaveCount(0);
});

test("croix au clic : masqué, mémorisé après rechargement, revient si la version change", async ({ page }) => {
  await open(page, "2.1.326");
  const notice = page.getByTestId(NOTICE);
  await expect(notice).toContainText("2.1.326 n'est pas la version testée");
  const close = page.getByRole("button", { name: "Fermer l'avertissement de version" });
  await expect(close).toHaveText("✕");
  await close.click();
  await expect(notice).toHaveCount(0);
  expect(await stored(page)).toBe("2.1.326");

  // Lancement suivant, même version : toujours masqué.
  await page.reload();
  await ready(page);
  await expect(page.getByTestId(CLOSE)).toHaveCount(0);
  await expect(notice).toHaveCount(0);

  // Nouvelle version : l'avertissement revient.
  await open(page, "2.1.327");
  await expect(notice).toContainText("2.1.327 n'est pas la version testée");
});

for (const key of ["Enter", " "]) {
  test(`croix au clavier (${key === " " ? "Espace" : key})`, async ({ page }) => {
    await open(page, "2.1.326");
    const close = page.getByTestId(CLOSE);
    await close.focus();
    await expect(close).toBeFocused();
    await page.keyboard.press(key);
    await expect(page.getByTestId(NOTICE)).toHaveCount(0);
    expect(await stored(page)).toBe("2.1.326");
    // La touche n'a rien écrit dans la saisie.
    await expect(page.locator(PROMPT)).toHaveValue("");
  });
}

test("croix atteinte au clavier par Maj+Tab depuis la saisie", async ({ page }) => {
  await open(page, "2.1.326");
  await page.locator(PROMPT).focus();
  const close = page.getByTestId(CLOSE);
  const focused = () => close.evaluate((el) => el === document.activeElement);
  for (let i = 0; i < 20 && !(await focused()); i++) await page.keyboard.press("Shift+Tab");
  await expect(close).toBeFocused();
});
