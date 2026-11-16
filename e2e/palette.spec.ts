/// <reference types="node" />
// Palette (jalon J10), en mode simulé, remplie par la réponse à `initialize`
// de `t11` (108 commandes) : ouverture par `/` en début de saisie et par
// Ctrl+K, rendu de la capture `10`, filtre, ↑↓, Entrée, Tab, Échap.
// Écrit une comparaison côte à côte dans `.claude-work/j8-j10/comparaison/`
// (ignoré par git).
import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { expect, test, type Page } from "@playwright/test";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
const REF = `${ROOT}.claude-work/j1/captures/10-menu-slash.png`;
const OUT_DIR = `${ROOT}.claude-work/j8-j10/comparaison`;

type Cmd = { name: string; description: string; argument_hint: string | null };
type Call = { cmd: string; args: Record<string, unknown> };
type Orch = {
  store: { status: string; lastSeq: number; apply(e: unknown[]): void; meta: { commands: Cmd[] } };
  calls: Call[];
};

const t11 = JSON.parse(readFileSync(`${ROOT}fixtures/ui-events/t11.json`, "utf8")) as Record<string, unknown>[];
const READY = t11.find((e) => e.kind === "ready")!;
const T11_COMMANDS = READY.commands as Cmd[];

const PROMPT = '[data-testid="prompt"] textarea[aria-label="Message"]';
const PALETTE = '[data-testid="palette"]';

async function open(page: Page, commands: Cmd[] = T11_COMMANDS) {
  await page.setViewportSize({ width: 1136, height: 813 });
  await page.goto("/?mock=demo&rows=0&rate=400");
  await page.waitForFunction(() => (window as unknown as { __orch?: Orch }).__orch?.store.status === "Idle");
  await page.evaluate(() => document.fonts.ready);
  // `ready` de t11 (réponse à `initialize`), à la suite de la numérotation.
  await page.evaluate(
    ({ ready, commands }) => {
      const s = (window as unknown as { __orch: Orch }).__orch.store;
      s.apply([{ ...ready, commands, seq: s.lastSeq + 1, t: Date.now(), session: "mock-1" }]);
    },
    { ready: READY, commands },
  );
  await expect(page.locator(PROMPT)).toBeFocused();
}

const draft = (page: Page) => page.locator(PROMPT).inputValue();
const calls = (page: Page, cmd: string) => page.evaluate((cmd) => (window as unknown as { __orch: Orch }).__orch.calls.filter((c) => c.cmd === cmd), cmd);
const shownNames = (page: Page) =>
  page.locator(`${PALETTE} [role="option"]`).evaluateAll((els) => els.map((e) => (e as HTMLElement).dataset.name));
const selected = (page: Page) => page.locator(`${PALETTE} [aria-selected="true"]`).getAttribute("data-name");

test("`/` en début de saisie : t11 (108 commandes), /formulaire-dynamique et /orchestrateur visibles", async ({ page }) => {
  await open(page);
  expect(await page.evaluate(() => (window as unknown as { __orch: Orch }).__orch.store.meta.commands.length)).toBe(108);
  await page.keyboard.type("/");
  await expect(page.locator(PALETTE)).toBeVisible();
  expect(await shownNames(page)).toEqual(["formulaire-dynamique", "graphify", "orchestrateur"]);
  expect(await selected(page)).toBe("formulaire-dynamique");
  // Au-dessus de la saisie : le menu finit sur la règle du haut.
  const pal = (await page.locator(PALETTE).boundingBox())!;
  const rule = (await page.locator('[data-testid="prompt"] .rule').first().boundingBox())!;
  expect(Math.abs(pal.y + pal.height - rule.y)).toBeLessThan(2);
  // Les deux skills de l'utilisateur, par leur filtre.
  await page.keyboard.type("orch");
  expect((await shownNames(page))[0]).toBe("orchestrateur");
  await page.keyboard.press("Control+a");
  await page.keyboard.type("/formu");
  expect((await shownNames(page))[0]).toBe("formulaire-dynamique");
});

test("rendu de la capture 10 et comparaison côte à côte", async ({ page, browser }) => {
  // Les trois premières entrées de la capture (ordre « récents » du TUI), puis t11.
  const loop = T11_COMMANDS.find((c) => c.name === "loop")!;
  const form = T11_COMMANDS.find((c) => c.name === "formulaire-dynamique")!;
  const commands: Cmd[] = [
    { ...loop, description: "Repeat a prompt or command on an interval (e.g. /loop 5m /foo)" },
    { name: "artifact-capabilities", description: "Runtime capabilities for published Artifacts", argument_hint: null },
    form,
    ...T11_COMMANDS.filter((c) => c !== loop && c !== form),
  ];
  await open(page, commands);
  await page.keyboard.type("/");
  const rows = page.locator(`${PALETTE} [role="option"]`);
  await expect(rows).toHaveCount(3);
  expect(await rows.evaluateAll((els) => els.map((e) => (e as HTMLElement).innerText.replace(/\s+/g, " ").trim()))).toEqual([
    "❯ /loop Repeat a prompt or command on an interval (e.g. /loop 5m /foo)",
    "/artifact-capabilities Runtime capabilities for published Artifacts",
    expect.stringMatching(/^\/formulaire-dynamique Faire trancher à l'utilisateur une série de choix/),
  ]);
  // Entrée choisie (nom et description) en bleu lavande, les autres en gris (#999999, relevé sur 10).
  const color = (sel: string) => page.locator(sel).first().evaluate((el) => getComputedStyle(el).color);
  expect(await color(`${PALETTE} .sel .name`)).toBe("rgb(177, 185, 249)");
  expect(await color(`${PALETTE} .sel .desc`)).toBe("rgb(177, 185, 249)");
  expect(await color(`${PALETTE} .row:not(.sel) .name`)).toBe("rgb(153, 153, 153)");
  expect(await color(`${PALETTE} .row:not(.sel) .desc`)).toBe("rgb(153, 153, 153)");
  // Descriptions alignées en colonne ; la longue repliée sur 2 lignes puis tronquée.
  const measure = () =>
    page.locator(`${PALETTE} .desc`).evaluateAll((els) =>
      els.map((e) => {
        const r = e.getBoundingClientRect();
        const lh = parseFloat(getComputedStyle(e).lineHeight);
        return { x: Math.round(r.left), lines: Math.round(r.height / lh), clipped: e.scrollHeight > e.clientHeight + 1 };
      }),
    );
  // Fenêtre de la capture, colonne des agents à droite : la saisie est plus
  // étroite que les 120 colonnes du TUI, la colonne des noms cède (45 %).
  let descs = await measure();
  expect(new Set(descs.map((d) => d.x)).size).toBe(1);
  expect(descs[2]).toEqual({ x: descs[0].x, lines: 2, clipped: true });
  // Saisie d'au moins 120 colonnes : description en colonne 4 + 46 = 50
  // (capture 10), /loop sur une ligne, la longue repliée puis tronquée.
  await page.setViewportSize({ width: 1700, height: 813 });
  await page.waitForTimeout(100);
  descs = await measure();
  const ch = await page.locator(`${PALETTE} .cur`).first().evaluate((e) => e.getBoundingClientRect().width / 4);
  const left = await page.locator(PALETTE).evaluate((e) => e.getBoundingClientRect().left);
  expect(Math.round((descs[0].x - left) / ch)).toBe(50);
  expect(descs[0].lines).toBe(1);
  expect(descs[2]).toEqual({ x: descs[0].x, lines: 2, clipped: true });
  await page.setViewportSize({ width: 1136, height: 813 });
  await page.waitForTimeout(100);

  mkdirSync(OUT_DIR, { recursive: true });
  const app = `${OUT_DIR}/10-app.png`;
  await page.screenshot({ path: app });
  if (!existsSync(REF)) return;
  const url = (p: string) => `data:image/png;base64,${readFileSync(p).toString("base64")}`;
  const side = await browser.newPage({ viewport: { width: 2300, height: 900 } });
  await side.setContent(`<!doctype html><html><body style="margin:0;background:#202020;color:#ddd;font:14px sans-serif">
    <div style="display:flex;gap:16px;padding:8px">
      <figure style="margin:0"><figcaption>Référence TUI 2.1.325 : 10-menu-slash.png</figcaption><img src="${url(REF)}"></figure>
      <figure style="margin:0"><figcaption>App (mode simulé) : palette J10</figcaption><img src="${url(app)}"></figure>
    </div></body></html>`);
  await side.screenshot({ path: `${OUT_DIR}/10-comparaison.png`, fullPage: true });
  await side.close();
});

test("↑↓ déplacent la sélection, la fenêtre de 3 suit ; Entrée envoie une commande sans argument", async ({ page }) => {
  await open(page);
  await page.keyboard.type("/");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  expect(await selected(page)).toBe("orchestrateur");
  await page.keyboard.press("ArrowDown");
  expect(await shownNames(page)).toEqual(["graphify", "orchestrateur", T11_COMMANDS[3].name]);
  await page.keyboard.press("ArrowUp");
  expect(await selected(page)).toBe("orchestrateur");
  await page.keyboard.press("Enter");
  await expect(page.locator(PALETTE)).toHaveCount(0);
  expect(await calls(page, "send_message")).toEqual([{ cmd: "send_message", args: { text: "/orchestrateur" } }]);
  expect(await draft(page)).toBe("");
});

test("Entrée sur une commande à argument insère `/nom ` ; Tab insère toujours", async ({ page }) => {
  await open(page);
  await page.keyboard.type("/loo");
  expect(await selected(page)).toBe("loop");
  await page.keyboard.press("Enter");
  expect(await draft(page)).toBe("/loop ");
  await expect(page.locator(PALETTE)).toHaveCount(0);
  await page.keyboard.type("5m ping");
  expect(await draft(page)).toBe("/loop 5m ping");
  await page.keyboard.press("Control+a");
  await page.keyboard.type("/orch");
  await page.keyboard.press("Tab");
  expect(await draft(page)).toBe("/orchestrateur ");
  expect(await calls(page, "send_message")).toHaveLength(0);
});

test("Échap ferme sans rien insérer ; la frappe suivante rouvre ; `/` au milieu du texte reste un caractère", async ({ page }) => {
  await open(page);
  await page.keyboard.type("/gra");
  await expect(page.locator(PALETTE)).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.locator(PALETTE)).toHaveCount(0);
  expect(await draft(page)).toBe("/gra");
  // Échap n'a pas armé l'effacement : un second Échap arme, sans effacer.
  await page.keyboard.type("p");
  await expect(page.locator(PALETTE)).toBeVisible();
  await page.keyboard.press("Control+a");
  await page.keyboard.type("du texte / puis");
  await expect(page.locator(PALETTE)).toHaveCount(0);
  // Aucun résultat : le menu disparaît (et Entrée envoie le texte tel quel).
  await page.keyboard.press("Control+a");
  await page.keyboard.type("/zzzzqqq");
  await expect(page.locator(PALETTE)).toHaveCount(0);
  await page.keyboard.press("Enter");
  expect(await calls(page, "send_message")).toEqual([{ cmd: "send_message", args: { text: "/zzzzqqq" } }]);
});

test("Ctrl+K : ouvre n'importe où, filtre à la frappe, garde le texte déjà tapé", async ({ page }) => {
  await open(page);
  await page.keyboard.type("à relire");
  // Focus hors de la saisie : Ctrl+K l'ouvre quand même.
  await page.locator(".vlist").focus();
  await page.keyboard.press("Control+k");
  await expect(page.locator(PALETTE)).toBeVisible();
  await expect(page.locator(PROMPT)).toBeFocused();
  expect(await selected(page)).toBe("formulaire-dynamique");
  await page.keyboard.type("orch");
  expect(await selected(page)).toBe("orchestrateur");
  await page.keyboard.press("Enter");
  // Texte déjà tapé : il suit la commande, rien ne part seul.
  expect(await draft(page)).toBe("/orchestrateur à relire");
  expect(await calls(page, "send_message")).toHaveLength(0);
  // Échap ferme sans rien insérer.
  await page.keyboard.press("Control+a");
  await page.keyboard.press("Delete");
  await page.keyboard.press("Control+k");
  await page.keyboard.type("zzzzqqq");
  await expect(page.locator(PALETTE)).toContainText("No matching commands");
  await page.keyboard.press("Escape");
  await expect(page.locator(PALETTE)).toHaveCount(0);
  expect(await draft(page)).toBe("zzzzqqq");
});

test("`/model` reste traité par l'app : Entrée ouvre le sélecteur de modèle", async ({ page }) => {
  await open(page);
  await page.keyboard.type("/model");
  expect(await selected(page)).toBe("model");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("model-picker")).toBeVisible();
  expect(await calls(page, "send_message")).toHaveLength(0);
});
