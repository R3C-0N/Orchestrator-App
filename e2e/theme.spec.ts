/// <reference types="node" />
// Thème clair/sombre (J15), dans l'app en mode simulé : le bouton de la barre
// d'état bascule au clic, à l'Entrée et à l'Espace (l'Espace ne part pas dans
// la saisie, malgré le gestionnaire global de la fenêtre), ignore les touches
// avec modificateur, et le choix survit au rechargement.
import { expect, test, type Page } from "@playwright/test";

type W = Window & { __orch: { store: { status: string } } };

const PROMPT = '[data-testid="prompt"] textarea[aria-label="Message"]';

async function open(page: Page) {
  await page.goto(`/?mock=demo&rows=6&rate=200`);
  await page.waitForFunction(() => (window as unknown as W).__orch?.store.status === "Idle");
}

const theme = (page: Page) => page.evaluate(() => document.documentElement.dataset.theme);
const bg = (page: Page) => page.evaluate(() => getComputedStyle(document.body).backgroundColor);

test("bouton de thème : clic, Entrée, Espace, modificateurs, persistance", async ({ page }) => {
  await open(page);
  const toggle = page.getByTestId("theme-toggle");
  // Défaut : sombre, palette de la maquette.
  expect(await theme(page)).toBe("dark");
  expect(await bg(page)).toBe("rgb(22, 22, 22)");
  await expect(toggle).toHaveAttribute("aria-label", "Passer au thème clair");
  await expect(toggle).toContainText("Clair");

  // Clic : clair.
  await toggle.click();
  expect(await theme(page)).toBe("light");
  expect(await bg(page)).toBe("rgb(246, 246, 244)");
  await expect(toggle).toHaveAttribute("aria-label", "Passer au thème sombre");
  await expect(toggle).toContainText("Sombre");

  // Espace : sombre, et rien n'est tapé dans la saisie.
  await toggle.focus();
  await page.keyboard.press("Space");
  expect(await theme(page)).toBe("dark");
  expect(await page.locator(PROMPT).inputValue()).toBe("");

  // Entrée : clair.
  await toggle.focus();
  await page.keyboard.press("Enter");
  expect(await theme(page)).toBe("light");

  // Avec un modificateur : ignoré.
  await toggle.focus();
  await page.keyboard.press("Shift+Space");
  await page.keyboard.press("Shift+Enter");
  expect(await theme(page)).toBe("light");

  // Rechargement : le choix est gardé.
  await open(page);
  expect(await theme(page)).toBe("light");
  expect(await bg(page)).toBe("rgb(246, 246, 244)");
});
