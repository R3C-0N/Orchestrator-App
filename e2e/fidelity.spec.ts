/// <reference types="node" />
// Fidélité du rendu (jalon J5) : chaque scénario de `scenarios.ts` est
// rejoué dans le magasin en mode simulé, puis :
// - le texte attendu de la capture de référence est vérifié dans le DOM ;
// - une capture d'écran de l'app est écrite dans `.claude-work/j5/comparaison/`
//   (dossier ignoré par git), avec une image côte à côte « référence | app »
//   quand le PNG de référence de J1 est présent sur le poste.
import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { expect, test, type Page } from "@playwright/test";
import { scenarios, type Scenario } from "./scenarios";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
const REF_DIR = `${ROOT}.claude-work/j1/captures`;
const OUT_DIR = `${ROOT}.claude-work/j5/comparaison`;

/** Textes que la capture de référence montre (lus dans les `.uia.txt`). */
const EXPECT: Record<string, string[]> = {
  "02": ["Titre", "Sous-titre", "- Un élément en gras", "1. Premier élément", "def saluer(nom):", "for 6s · done 12:20"],
  "03": ["Read 1 file, ran 1 shell command", "ok"],
  "03b": [
    "Read(C:\\projets\\demo\\03-outils\\notes.txt)",
    "Read 8 lines",
    "Bash(seq 1 5 | tee nombres.txt)",
    "01:13 PM claude-haiku-5-5",
    "Showing detailed transcript · ctrl+o to toggle",
  ],
  "04": ["Searched for 1 pattern, read 1 file", "Update(config.py)", "Added 3 lines, removed 2 lines", "TIMEOUT = 60", "BACKOFF = 1.5"],
  "05": ["2 background agents launched (↓ to manage)", "agent-bleu", "(Ping rouge agent)", "Waiting for 2 background agents to finish"],
  "06": ['Agent "Ping rouge" finished · 42s', 'Agent "Ping bleu" finished · 43s', "Message from @agent-rouge (ctrl+o to expand)"],
  "08": ["No result recorded for 1 call", "$ ping -n 90 127.0.0.1", "Interrupted · What should Claude do instead?"],
  "09": ["Update(config.py)", "Error editing file", "Ran 2 shell commands"],
  "09b": [
    "Update(C:\\projets\\demo\\09-erreurs\\config.py)",
    "Error: String to replace not found in file.",
    "String: INEXISTANT = 0",
    "Bash(mkdir dossier-absent/sous-dossier)",
    "Error: Exit code 1",
    "Bash(exit 3)",
    "Error: Exit code 3",
    "12:29 PM claude-haiku-5-5",
  ],
  k1: ["Running ping for 90 requests to localhost ·", "$ ping -n 90 127.0.0.1", "tokens · thought for 1s)"],
};

/** Textes qui ne doivent pas apparaître (règles de la vue normale). */
const ABSENT: Record<string, string[]> = {
  "02": ["● ##", "## Titre", "```"],
  "03": ["Read(", "Bash("],
  "05": ["Done ("],
  "06": ["Done ("],
  "08": ["Interrupted by user"],
  "09": ["Bash(", "Exit code"],
};

/** Couleur calculée (`color` ou `background-color`) du premier élément trouvé. */
const css = (page: Page, sel: string, prop: "color" | "background-color") =>
  page.locator(sel).first().evaluate((el, p) => getComputedStyle(el).getPropertyValue(p), prop);

/** Assertions de style ciblées (valeurs relevées sur les pixels des captures). */
const STYLE: Record<string, (page: Page) => Promise<void>> = {
  "02": async (page) => {
    expect(await css(page, ".md-code", "color")).toBe("rgb(177, 185, 249)");
    // f-string : seul `f"Bonjour, ` est rouge, la suite garde la couleur du texte.
    const strings = await page.locator(".md-pre .hljs-string").allInnerTexts();
    expect(strings[0]).toBe('f"Bonjour, ');
    // Deux lignes vides avant `if __name__` (02.uia.txt, lignes 25-26).
    expect(await page.locator(".md-pre").innerText()).toContain("print(message)\n\n\nif __name__");
    // Bande du message utilisateur.
    expect(await css(page, ".user-band", "background-color")).toBe("rgb(55, 55, 55)");
  },
  "03": async (page) => {
    await expect(page.locator('[aria-label="Message"]')).toBeVisible();
    await expect(page.getByTestId("status-line")).toHaveText(/bypass permissions on/);
  },
  "03b": async (page) => {
    await expect(page.locator('[aria-label="Message"]')).toHaveCount(0);
    await expect(page.getByTestId("status-line")).toHaveCount(0);
    await expect(page.getByTestId("detail-bar")).toBeVisible();
  },
  "04": async (page) => {
    expect(await css(page, ".g.ok", "color")).toBe("rgb(78, 186, 101)");
    expect(await css(page, ".dl.del", "background-color")).toBe("rgb(61, 1, 0)");
    expect(await css(page, ".dl.add", "background-color")).toBe("rgb(2, 40, 0)");
    // Mots changés : fonds relevés sur 04 (« 30 » #5C0200 en (199, 365), « 5 » #044700 en (199, 422)).
    expect(await css(page, ".dl.del .dw", "background-color")).toBe("rgb(92, 2, 0)");
    expect(await css(page, ".dl.add .dw", "background-color")).toBe("rgb(4, 71, 0)");
  },
  "09": async (page) => {
    expect(await css(page, ".g.err", "color")).toBe("rgb(255, 107, 128)");
  },
  "09b": async (page) => {
    expect(await css(page, ".g.err", "color")).toBe("rgb(255, 107, 128)");
    await expect(page.locator('[aria-label="Message"]')).toHaveCount(0);
  },
};

async function play(page: Page, sc: Scenario) {
  await page.goto("/?mock=demo&rows=0");
  await page.waitForFunction(() => (window as unknown as { __orch?: unknown }).__orch !== undefined);
  await page.evaluate(() => document.fonts.ready);
  if (sc.verbose) {
    await page.locator(".vlist").focus();
    await page.keyboard.press("Control+o");
    await expect(page.locator(".detail-bar")).toBeVisible();
  }
  const rel = sc.relative === true;
  await page.evaluate(({ steps, rel }) => {
    const s = (window as unknown as { __orch: { store: { lastSeq: number; apply(e: unknown[]): void } } }).__orch.store;
    let seq = s.lastSeq + 1;
    const base = rel ? Date.now() : 0;
    s.apply(steps.map((st) => ({ seq: seq++, t: base + st.t, session: "mock-1", ...st.body })));
  }, { steps: sc.steps as unknown as { t: number; body: Record<string, unknown> }[], rel });
  await page.waitForTimeout(sc.settleMs ?? 300);
}

function dataUrl(path: string): string {
  return `data:image/png;base64,${readFileSync(path).toString("base64")}`;
}


for (const sc of scenarios()) {
  test(`fidélité ${sc.id} (${sc.ref})`, async ({ page, browser }) => {
    // Fenêtre de la capture de référence (1136 × 853, barre de titre ôtée).
    await page.setViewportSize({ width: 1136, height: 813 });
    await play(page, sc);
    // Comparaison à espaces près (les cellules de la grille sont des blocs du DOM).
    const flat = (x: string) => x.replace(/\s+/g, " ").trim();
    const text = flat(await page.locator(".conversation").innerText());
    for (const s of EXPECT[sc.id] ?? []) expect(text, `« ${s} » attendu (${sc.id})`).toContain(flat(s));
    for (const s of ABSENT[sc.id] ?? []) expect(text, `« ${s} » inattendu (${sc.id})`).not.toContain(flat(s));

    await STYLE[sc.id]?.(page);

    mkdirSync(OUT_DIR, { recursive: true });
    const app = `${OUT_DIR}/${sc.id}-app.png`;
    await page.screenshot({ path: app });

    const ref = `${REF_DIR}/${sc.ref}.png`;
    if (!existsSync(ref)) return;
    const side = await browser.newPage({ viewport: { width: 2300, height: 900 } });
    await side.setContent(`<!doctype html><html><body style="margin:0;background:#202020;color:#ddd;font:14px sans-serif">
      <div style="display:flex;gap:16px;padding:8px">
        <figure style="margin:0"><figcaption>Référence TUI 2.1.325 : ${sc.ref}.png</figcaption><img src="${dataUrl(ref)}"></figure>
        <figure style="margin:0"><figcaption>App (mode simulé) : ${sc.id}${sc.verbose ? " (Ctrl+O)" : ""}</figcaption><img src="${dataUrl(app)}"></figure>
      </div></body></html>`);
    await side.screenshot({ path: `${OUT_DIR}/${sc.id}-comparaison.png`, fullPage: true });
    await side.close();
  });
}

test("Ctrl+O bascule et revient sans perdre la ligne lue", async ({ page }) => {
  await page.setViewportSize({ width: 1136, height: 813 });
  await page.goto("/?mock=long&rows=600");
  await page.waitForFunction(
    () => ((window as unknown as { __orch?: { store: { rows: unknown[] } } }).__orch?.store.rows.length ?? 0) >= 600,
  );
  await page.locator(".vlist").focus();
  // Remonter au milieu, puis lire la première ligne source visible.
  await page.evaluate(() => {
    const c = (window as unknown as { __orch: { conversation(): { handle(): { scrollToIndex(i: number, o: unknown): void } ; itemCount(): number } } }).__orch.conversation();
    c.handle().scrollToIndex(Math.floor(c.itemCount() / 2), { align: "start" });
  });
  await page.mouse.move(500, 300);
  await page.mouse.wheel(0, -40);
  await page.waitForTimeout(300);
  const topText = () =>
    page.evaluate(() => {
      const el = document.querySelector<HTMLElement>(".vlist")!;
      const vp = el.getBoundingClientRect();
      // Première ligne affichée en haut de la vue : sa ligne source (`data-first-row`).
      const rows = [...el.querySelectorAll<HTMLElement>("[data-first-row]")].sort(
        (a, b) => a.getBoundingClientRect().top - b.getBoundingClientRect().top,
      );
      const r = rows.find((x) => x.getBoundingClientRect().bottom > vp.top + 4);
      return r?.dataset.firstRow ?? "";
    });
  const before = await topText();
  await page.keyboard.press("Control+o");
  await expect(page.locator(".detail-bar")).toBeVisible();
  await page.waitForTimeout(300);
  const during = await topText();
  await page.keyboard.press("Control+o");
  await expect(page.locator(".detail-bar")).toHaveCount(0);
  await page.waitForTimeout(300);
  const after = await topText();
  console.log(`MESURE ctrl-o ${JSON.stringify({ before, during, after })}`);
  expect(before).not.toBe("");
  expect(after).toBe(before);
  expect(during).not.toBe("");
});
