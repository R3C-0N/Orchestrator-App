/// <reference types="node" />
// Barre de titre de la maquette « Orchestrator » (`TitleBar.svelte`, spec
// décision 20) montée seule, hors de Tauri : rendu (pastille, nom, version,
// trois boutons), zone de déplacement, et clics ou touches sans exception
// (sans `__TAURI_INTERNALS__`, les boutons ne font rien).
//
// Banc : une page servie par `page.route` monte le composant avec le `mount`
// de Svelte. Le module de Svelte est pris dans les dépendances pré-groupées
// de Vite avec le même `?v=` que celui importé par le composant transformé,
// pour qu'ils partagent un seul moteur. Les variables de couleur viennent de
// `src/app.css`.
import { expect, test, type Page } from "@playwright/test";

const HARNESS = "/__titlebar-harness.html";
const BAR = "title-bar";

/**
 * `internals` : pose un `__TAURI_INTERNALS__` sans `metadata` avant le
 * montage, comme `mockIPC` dans `e2e/startup.harness.ts` (`getCurrentWindow`
 * y lève) ; ses `invoke` échouent.
 */
async function open(page: Page, version: string | null, internals = false): Promise<string[]> {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  page.on("console", (m) => {
    // Le client HMR de Vite (WebSocket refusé par Chromium) n'est pas en cause.
    if (m.type() === "error" && !/WebSocket|\[vite\]|Vite server/.test(m.text())) errors.push(m.text());
  });
  await page.route(`**${HARNESS}`, (route) =>
    route.fulfill({
      contentType: "text/html",
      body: `<!doctype html><html><head><meta charset="utf-8">
        <link rel="stylesheet" href="/src/app.css"></head><body><div id="app"></div>
        <script type="module">
          ${internals ? 'window.__TAURI_INTERNALS__ = { invoke: () => Promise.reject(new Error("simulé")) };' : ""}
          const COMP = "/src/lib/components/TitleBar.svelte";
          const src = await (await fetch(COMP)).text();
          const v = /\\/node_modules\\/\\.vite\\/deps\\/[^"']+\\?v=([0-9a-f]+)/.exec(src);
          const { mount } = await import("/node_modules/.vite/deps/svelte.js" + (v ? "?v=" + v[1] : ""));
          const { default: TitleBar } = await import(COMP);
          mount(TitleBar, { target: document.getElementById("app"), props: { version: ${JSON.stringify(version)} } });
          window.__mounted = true;
        </script></body></html>`,
    }),
  );
  await page.goto(HARNESS);
  await page.waitForFunction(() => (window as unknown as { __mounted?: boolean }).__mounted === true);
  await expect(page.getByTestId(BAR)).toBeVisible();
  return errors;
}

test("rendu : nom, version, trois boutons, mesures de la maquette", async ({ page }) => {
  const errors = await open(page, "2.1.325");
  const bar = page.getByTestId(BAR);
  await expect(bar).toContainText("orchestrator");
  await expect(page.getByTestId("title-bar-version")).toHaveText("— claude 2.1.325");
  for (const label of ["Réduire", "Agrandir", "Fermer"]) {
    await expect(bar.getByRole("button", { name: label, exact: true })).toBeVisible();
  }
  // Toute la barre déplace la fenêtre (les boutons restent cliquables).
  await expect(bar).toHaveAttribute("data-tauri-drag-region", "deep");
  const m = await bar.evaluate((el) => {
    const s = getComputedStyle(el);
    const b = el.querySelector("button")!.getBoundingClientRect();
    return { h: el.getBoundingClientRect().height, bg: s.backgroundColor, fs: s.fontSize, bw: b.width, bh: b.height };
  });
  expect(m).toEqual({ h: 36, bg: "rgb(16, 16, 16)", fs: "12px", bw: 46, bh: 36 });
  expect(errors).toEqual([]);
});

test("version inconnue : rien après « orchestrator »", async ({ page }) => {
  const errors = await open(page, null);
  await expect(page.getByTestId("title-bar-version")).toHaveCount(0);
  await expect(page.getByTestId(BAR)).not.toContainText("claude");
  expect(errors).toEqual([]);
});

test("hors de Tauri : clics et touches sans exception", async ({ page }) => {
  const errors = await open(page, "2.1.325");
  const bar = page.getByTestId(BAR);
  for (const label of ["Réduire", "Agrandir", "Fermer"]) {
    const b = bar.getByRole("button", { name: label, exact: true });
    await b.click();
    await b.focus();
    await page.keyboard.press("Enter");
    await page.keyboard.press(" ");
  }
  await bar.dblclick({ position: { x: 300, y: 18 } });
  // Toujours là, rien n'a levé.
  await expect(bar).toBeVisible();
  expect(errors).toEqual([]);
});

test("__TAURI_INTERNALS__ sans metadata (mockIPC) : montage, clics et touches sans exception", async ({ page }) => {
  const errors = await open(page, "2.1.325", true);
  const bar = page.getByTestId(BAR);
  await expect(bar.getByRole("button", { name: "Agrandir", exact: true })).toBeVisible();
  for (const label of ["Réduire", "Agrandir", "Fermer"]) {
    const b = bar.getByRole("button", { name: label, exact: true });
    await b.click();
    await b.focus();
    await page.keyboard.press("Enter");
  }
  await expect(bar).toBeVisible();
  expect(errors).toEqual([]);
});
