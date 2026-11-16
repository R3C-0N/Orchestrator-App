/// <reference types="node" />
// Colonne des agents (jalon J9), sur sa page de démonstration en mode simulé
// (`src/lib/components/agents/demo/index.html`, servie par Vite) :
// - `t3` rejoué (fixture `fixtures/ui-events/t3.json`, temps étiré) : 2 lignes
//   bleu et rouge, état, durée qui avance puis se fige, `last_tool` ;
// - bonus : clic → fil de l'agent ;
// - 4 agents + 1 gris : lisible (pas de débordement, 2 lignes par agent) ;
// - défilement de la conversation à côté de la colonne vivante : seuils 1.11 ;
// - comparaison visuelle avec la capture `05` du TUI dans
//   `.claude-work/j9/comparaison/` (dossier non versionné).
import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { expect, test, type Page } from "@playwright/test";
import { scenarios } from "./scenarios";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
const REF = `${ROOT}.claude-work/j1/captures/05-agents-en-cours.png`;
const OUT_DIR = `${ROOT}.claude-work/j9/comparaison`;
const DEMO = "/src/lib/components/agents/demo/index.html";

const BLEU = "toolu_01Fvzn2k9s2bVaPNYQBYGJJG";
const ROUGE = "toolu_01SDmyAGx26yC6ZqWR52vuwR";

type Demo = {
  store: { agents: { status: string }[]; status: string; rows: unknown[] };
  apply(events: unknown[]): void;
  done(): boolean;
};
declare global {
  interface Window {
    __agentsDemo: Demo;
    __rec: { start(): void; stop(): number[] };
  }
}

async function open(page: Page, query: string) {
  await page.goto(`${DEMO}?${query}`);
  await page.waitForFunction(() => window.__agentsDemo !== undefined);
  await page.evaluate(() => document.fonts.ready);
}

const card = (page: Page, id: string) => page.locator(`[data-agent="${id}"]`);
const stats = (page: Page, id: string) => card(page, id).getByTestId("agent-stats").innerText();
const secs = (s: string) => Number(/^(\d+)s/.exec(s)?.[1] ?? NaN);

test("t3 : deux agents bleu et rouge, état, durée qui avance puis se fige, last_tool", async ({ page }) => {
  await page.setViewportSize({ width: 1136, height: 813 });
  // 600 ms de fixture × 20 = 12 s.
  await open(page, "mock=t3&stretch=20");
  await expect(card(page, BLEU)).toBeVisible();
  await expect(card(page, ROUGE)).toBeVisible();
  const col = page.getByTestId("agent-column");
  await expect(col).toContainText("● main");
  await expect(col.locator(".card")).toHaveCount(2);

  // Couleurs : nom noir sur le fond de l'agent (capture 05 : bleu #6A9BCC, rouge #DC2626).
  const bg = (id: string) => card(page, id).locator(".name").evaluate((el) => getComputedStyle(el).backgroundColor);
  expect(await bg(BLEU)).toBe("rgb(106, 155, 204)");
  expect(await bg(ROUGE)).toBe("rgb(220, 38, 38)");
  expect(await card(page, BLEU).locator(".name").evaluate((el) => getComputedStyle(el).color)).toBe("rgb(0, 0, 0)");
  await expect(card(page, BLEU).locator(".name")).toHaveText("agent-bleu");
  await expect(card(page, BLEU).locator(".desc")).toHaveText("Lecture a.txt en 3 mots");

  // Pendant l'exécution : ◯, outil en cours, durée qui avance.
  await expect(card(page, BLEU).getByTestId("agent-detail")).toHaveText("Read…", { timeout: 10_000 });
  await expect(card(page, BLEU).getByTestId("agent-glyph")).toHaveText("◯");
  await expect(card(page, BLEU)).toHaveAttribute("data-status", "Running");
  const s1 = await stats(page, BLEU);
  expect(s1).toMatch(/^\d+s · ↓ 25\.7k tokens$/);
  await page.waitForTimeout(2100);
  const s2 = await stats(page, BLEU);
  expect(secs(s2)).toBeGreaterThanOrEqual(secs(s1) + 1);

  // Fin : ● vert, durée figée à ended_at − started_at = (520 − 160) × 20 ms ≈ 7 s.
  await page.waitForFunction(() => window.__agentsDemo.done(), null, { timeout: 20_000 });
  await expect(card(page, BLEU)).toHaveAttribute("data-status", "Completed");
  await expect(card(page, BLEU).getByTestId("agent-glyph")).toHaveText("●");
  expect(await card(page, BLEU).getByTestId("agent-glyph").evaluate((el) => getComputedStyle(el).color)).toBe(
    "rgb(78, 186, 101)",
  );
  await expect(card(page, BLEU).getByTestId("agent-detail")).toHaveText("Done · 1 tool use");
  const f1 = await stats(page, BLEU);
  expect(f1).toBe("7s · ↓ 35.6k tokens");
  await page.waitForTimeout(1500);
  expect(await stats(page, BLEU)).toBe(f1);
  // Rouge : (440 − 220) × 20 ms = 4,4 s.
  expect(await stats(page, ROUGE)).toBe("4s · ↓ 24.0k tokens");
  await expect(page.getByTestId("agent-main-state")).toHaveText("Exited");

  mkdirSync(OUT_DIR, { recursive: true });
  await page.screenshot({ path: `${OUT_DIR}/t3-app.png` });
});

test("bonus : clic sur un agent → son fil, au clavier aussi", async ({ page }) => {
  await page.setViewportSize({ width: 1136, height: 813 });
  await open(page, "mock=t3&stretch=1");
  await page.waitForFunction(() => window.__agentsDemo.done());
  const head = card(page, BLEU).locator("button.head");
  await expect(head).toHaveAttribute("aria-expanded", "false");
  await head.click();
  await expect(head).toHaveAttribute("aria-expanded", "true");
  const thread = card(page, BLEU).getByTestId("agent-thread");
  await expect(thread).toContainText("❯lis a.txt et réponds en 3 mots");
  await expect(thread).toContainText("Read(");
  await expect(thread).toContainText(/⎿\s*Read \d+ lines/);
  await expect(thread).toContainText("Bonjour fichier");
  await page.screenshot({ path: `${OUT_DIR}/t3-fil-deroule.png` });
  // Au clavier : Entrée replie, Espace déplie le rouge.
  await head.focus();
  await page.keyboard.press("Enter");
  await expect(card(page, BLEU).getByTestId("agent-thread")).toHaveCount(0);
  await card(page, ROUGE).locator("button.head").focus();
  await page.keyboard.press("Space");
  await expect(card(page, ROUGE).getByTestId("agent-thread")).toContainText("agent rouge");
});

test("le fil déroulé suit le flux en direct", async ({ page }) => {
  await page.setViewportSize({ width: 1136, height: 813 });
  await open(page, "mock=four&scale=2");
  const vert = card(page, "toolu_four_vert");
  await vert.locator("button.head").click();
  const thread = vert.getByTestId("agent-thread");
  await expect(thread).toContainText("Mission : Tests Vitest du magasin.");
  // Le résultat de `npm test` et la réponse arrivent après l'ouverture du fil.
  await expect(thread).toContainText("Tests  92 passed (92)", { timeout: 15_000 });
  await expect(thread).toContainText("Tous les tests passent.", { timeout: 15_000 });
});

test("4 agents + 1 gris : lisible, sans débordement, 2 lignes par agent", async ({ page }) => {
  await page.setViewportSize({ width: 1136, height: 813 });
  await open(page, "mock=four");
  const col = page.getByTestId("agent-column");
  await expect(col.locator(".card")).toHaveCount(5);
  await expect(col.locator('[data-status="Running"]')).toHaveCount(5, { timeout: 5_000 });
  await page.waitForTimeout(2500);
  mkdirSync(OUT_DIR, { recursive: true });
  await page.screenshot({ path: `${OUT_DIR}/four-en-cours.png` });
  // Pas de défilement horizontal ; chaque ligne d'agent tient sur 2 lignes de grille.
  expect(await col.evaluate((el) => el.scrollWidth <= el.clientWidth)).toBe(true);
  const lh = await col.evaluate((el) => parseFloat(getComputedStyle(el).lineHeight));
  for (const h of await col.locator("button.head").evaluateAll((els) => els.map((e) => e.getBoundingClientRect().height)))
    expect(h).toBeCloseTo(2 * lh, 0);
  // Gris pour la couleur inconnue.
  expect(
    await card(page, "toolu_four_inconnu").locator(".name").evaluate((el) => getComputedStyle(el).backgroundColor),
  ).toBe("rgb(118, 118, 118)");
  await page.waitForFunction(() => window.__agentsDemo.done(), null, { timeout: 30_000 });
  await expect(card(page, "toolu_four_orange")).toHaveAttribute("data-status", "Failed");
  await expect(card(page, "toolu_four_orange").getByTestId("agent-detail")).toHaveText("Failed · 2 tool uses");
  await page.screenshot({ path: `${OUT_DIR}/four-termine.png` });
});

function dataUrl(path: string): string {
  return `data:image/png;base64,${readFileSync(path).toString("base64")}`;
}

test("comparaison visuelle avec la capture 05 du TUI", async ({ page, browser }) => {
  await page.setViewportSize({ width: 1136, height: 813 });
  await open(page, "mock=none");
  const sc = scenarios().find((s) => s.id === "05")!;
  // Même situation que 05 : agents lancés il y a 6 s, ~39k tokens chacun.
  const first = sc.steps.find((s) => s.body.kind === "agent")!.body as { started_at: number };
  const t0 = first.started_at;
  await page.evaluate(
    ({ steps, t0 }) => {
      const shift = Date.now() - 6_200 - t0;
      const tok: Record<string, number> = { toolu_bleu: 38_900, toolu_rouge: 38_800 };
      window.__agentsDemo.apply(
        steps.map((st) => {
          const b = { ...st.body } as Record<string, unknown>;
          if (b.kind === "agent") {
            b.started_at = (b.started_at as number) + shift;
            b.tokens = tok[b.tool_use_id as string];
          }
          return { seq: 0, t: st.t + shift, session: "demo", ...b };
        }),
      );
    },
    { steps: sc.steps as unknown as { t: number; body: Record<string, unknown> }[], t0 },
  );
  await page.waitForTimeout(300);
  await expect(page.getByTestId("agent-column")).toContainText("6s · ↓ 38.9k tokens");
  await expect(page.getByTestId("agent-column")).toContainText("6s · ↓ 38.8k tokens");
  mkdirSync(OUT_DIR, { recursive: true });
  const app = `${OUT_DIR}/05-app.png`;
  await page.screenshot({ path: app });
  if (!existsSync(REF)) return;
  const side = await browser.newPage({ viewport: { width: 2300, height: 900 } });
  await side.setContent(`<!doctype html><html><body style="margin:0;background:#202020;color:#ddd;font:14px sans-serif">
    <div style="display:flex;gap:16px;padding:8px">
      <figure style="margin:0"><figcaption>Référence TUI 2.1.325 : 05-agents-en-cours.png</figcaption><img src="${dataUrl(REF)}"></figure>
      <figure style="margin:0"><figcaption>App (démo J9, colonne des agents à droite)</figcaption><img src="${dataUrl(app)}"></figure>
    </div></body></html>`);
  await side.screenshot({ path: `${OUT_DIR}/05-comparaison.png`, fullPage: true });
  // Gros plan : panneau du TUI (bas de la capture) au-dessus de la colonne.
  const crop = `${OUT_DIR}/05-colonne.png`;
  await page.getByTestId("agent-column").screenshot({ path: crop });
  await side.setViewportSize({ width: 1200, height: 400 });
  await side.setContent(`<!doctype html><html><body style="margin:0;background:#202020;color:#ddd;font:14px sans-serif;padding:8px">
    <div>Panneau des agents du TUI (05, bas de la capture)</div>
    <div style="width:1136px;height:90px;background:url('${dataUrl(REF)}') 0 -763px no-repeat"></div>
    <div style="margin-top:12px">Colonne de l'app (même état)</div><img src="${dataUrl(crop)}" style="max-height:220px"></body></html>`);
  await side.screenshot({ path: `${OUT_DIR}/05-panneau-gros-plan.png`, fullPage: true });
  await side.close();
});

// --- Défilement à côté de la colonne vivante (plan 1.11) ----------------------

function installRecorder() {
  let d: number[] = [];
  let run = false;
  window.__rec = {
    start() {
      d = [];
      run = true;
      let last = 0;
      const loop = (t: number) => {
        if (!run) return;
        if (last) d.push(t - last);
        last = t;
        requestAnimationFrame(loop);
      };
      requestAnimationFrame(loop);
    },
    stop() {
      run = false;
      return d;
    },
  };
}

test("défilement de 3 000 lignes avec 5 agents vivants dans la colonne : ≥ 58 i/s, < 1 % > 33 ms", async ({ page }) => {
  test.setTimeout(120_000);
  await page.addInitScript(installRecorder);
  await page.setViewportSize({ width: 1100, height: 760 });
  // scale 4 : les agents tournent ~1 min, la colonne se redessine chaque seconde.
  await open(page, "mock=four&scale=4&rows=3000");
  await expect(page.locator('[data-status="Running"]')).toHaveCount(5, { timeout: 10_000 });
  await page.locator(".vlist").focus();
  await page.evaluate(() => window.__rec.start());
  await page.evaluate(
    () =>
      new Promise<void>((resolve) => {
        const el = document.querySelector<HTMLElement>(".vlist")!;
        el.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
        let dir = -1;
        let last = performance.now();
        const end = last + 10_000;
        const step = (t: number) => {
          const dt = t - last;
          last = t;
          const before = el.scrollTop;
          el.scrollTop = before + (dir * 3000 * dt) / 1000;
          if (el.scrollTop === before) dir = -dir;
          if (t < end) requestAnimationFrame(step);
          else {
            window.dispatchEvent(new PointerEvent("pointerup", { bubbles: true }));
            resolve();
          }
        };
        requestAnimationFrame(step);
      }),
  );
  const d = await page.evaluate(() => window.__rec.stop());
  const total = d.reduce((a, b) => a + b, 0);
  const m = {
    frames: d.length,
    seconds: +(total / 1000).toFixed(2),
    fps: +((d.length * 1000) / total).toFixed(1),
    over33pct: +((100 * d.filter((x) => x > 33).length) / d.length).toFixed(2),
    max: +Math.max(...d).toFixed(1),
  };
  console.log(`MESURE scroll-avec-colonne ${JSON.stringify(m)}`);
  // Les agents tournaient pendant toute la mesure.
  await expect(page.locator('[data-status="Running"]').first()).toBeVisible();
  expect(m.seconds).toBeGreaterThanOrEqual(9.9);
  expect(m.fps).toBeGreaterThanOrEqual(58);
  expect(m.over33pct).toBeLessThan(1);
});
