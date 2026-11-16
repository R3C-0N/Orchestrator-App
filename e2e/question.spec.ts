/// <reference types="node" />
// Carte AskUserQuestion (jalon J8), en mode simulé : rendu de la capture
// `07`, réponse au clavier (simple, chiffre, réponse libre, multi-sélection,
// plusieurs questions), « ignorer » par Échap (`t10`), « Chat about this »,
// et égalité des `answers` envoyés avec `t4`. Écrit une comparaison côte à
// côte dans `.claude-work/j8-j10/comparaison/` (ignoré par git).
import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { expect, test, type Page } from "@playwright/test";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
const REF = `${ROOT}.claude-work/j1/captures/07-question.png`;
const OUT_DIR = `${ROOT}.claude-work/j8-j10/comparaison`;

type QItem = {
  question: string;
  header: string;
  options: { label: string; description: string }[];
  multi_select: boolean;
};
type Call = { cmd: string; args: Record<string, unknown> };
type W = Window & {
  __orch: {
    store: {
      status: string;
      question: { requestId: string; canSkip: boolean } | null;
      rows: { kind: string; text?: string; result?: { text: string } | null }[];
      lastSeq: number;
      apply(e: unknown[]): void;
    };
    mock: { ask(q: QItem[], o?: { canSkip?: boolean }): string };
    calls: Call[];
  };
};

const CARD = '[data-testid="question-card"]';
const PROMPT = '[data-testid="prompt"] textarea[aria-label="Message"]';

/** Question de la capture `07` (texte lu dans `07-question.uia.txt`). */
const LANG: QItem = {
  question: "Quel langage préfères-tu ?",
  header: "Langage",
  multi_select: false,
  options: [
    { label: "Rust", description: "Performances élevées, typage strict et gestion mémoire sans ramasse-miettes." },
    {
      label: "TypeScript",
      description: "JavaScript typé, écosystème web très large, bon pour les interfaces et le Node.js.",
    },
    {
      label: "Python",
      description: "Syntaxe simple et lisible, très riche en bibliothèques pour les données, scripts et prototypes.",
    },
  ],
};

async function open(page: Page) {
  await page.setViewportSize({ width: 1136, height: 813 });
  await page.goto("/?mock=demo&rows=0&rate=400");
  await page.waitForFunction(() => (window as unknown as W).__orch?.store.status === "Idle");
  await page.evaluate(() => document.fonts.ready);
}

async function ask(page: Page, qs: QItem[], canSkip?: boolean): Promise<string> {
  const id = await page.evaluate(
    ({ qs, canSkip }) => (window as unknown as W).__orch.mock.ask(qs, canSkip === undefined ? {} : { canSkip }),
    { qs, canSkip },
  );
  await expect(page.locator(CARD)).toBeVisible();
  return id;
}

const calls = (page: Page, cmd: string) =>
  page.evaluate((cmd) => (window as unknown as W).__orch.calls.filter((c) => c.cmd === cmd), cmd);

/** Texte du résultat du dernier appel d'outil du fil. */
const lastToolResult = (page: Page) =>
  page.evaluate(() => {
    type R = { kind: string; result?: { text: string } | null };
    const rows = (window as unknown as { __orch: { store: { rows: R[] } } }).__orch.store.rows;
    const tools = rows.filter((r: R) => r.kind === "tool");
    return tools[tools.length - 1]?.result?.text ?? "";
  });

const css = (page: Page, sel: string, prop: string) =>
  page.locator(sel).first().evaluate((el, p) => getComputedStyle(el).getPropertyValue(p), prop);

/** La carte s'est fermée (`question_closed`) et la saisie est revenue. */
async function closed(page: Page) {
  await expect(page.locator(CARD)).toHaveCount(0);
  await expect(page.locator(PROMPT)).toBeVisible();
}

test("rendu de la capture 07 et comparaison côte à côte", async ({ page, browser }) => {
  await open(page);
  // Le message de la capture, puis la question.
  await page.evaluate(() => {
    const s = (window as unknown as W).__orch.store;
    s.apply([
      {
        seq: s.lastSeq + 1,
        t: Date.now(),
        session: "mock-1",
        kind: "user_message",
        thread: "main",
        uuid: null,
        text: "Utilise l'outil AskUserQuestion pour me demander quel langage je prefere, avec 3 options : Rust, TypeScript, Python.",
        origin: "Typed",
      },
    ]);
  });
  await ask(page, [LANG]);
  const card = page.locator(CARD);
  // Une ligne de terminal par élément ; le retrait des descriptions (CSS,
  // en `ch`) est rendu en espaces ; le champ libre vide montre son invite.
  const text = await card.evaluate((el) => {
    const ch = (() => {
      const s = document.createElement("span");
      s.textContent = "0000000000";
      el.appendChild(s);
      const w = s.getBoundingClientRect().width / 10;
      s.remove();
      return w;
    })();
    return [...el.children]
      .map((c) => {
        const input = c.querySelector("input");
        const pad = " ".repeat(Math.round(parseFloat(getComputedStyle(c).paddingLeft) / ch));
        const body = (c.textContent ?? "") + (input ? input.value || input.placeholder : "");
        return (c.classList.contains("desc") ? pad + body : body).replace(/\s+$/, "");
      })
      .filter((l) => l.trim() !== "" && !/^─+$/.test(l.trim()));
  });
  expect(text).toEqual([
    " ☐ Langage",
    "Quel langage préfères-tu ?",
    "❯ 1. Rust",
    "     Performances élevées, typage strict et gestion mémoire sans ramasse-miettes.",
    "  2. TypeScript",
    "     JavaScript typé, écosystème web très large, bon pour les interfaces et le Node.js.",
    "  3. Python",
    "     Syntaxe simple et lisible, très riche en bibliothèques pour les données, scripts et prototypes.",
    "  4. Type something.",
    "  5. Chat about this",
    "Enter to select · ↑/↓ to navigate · Esc to cancel",
  ]);
  // Une règle avant la pastille, une avant « Chat about this », pas d'encadré.
  await expect(card.locator(".rule")).toHaveCount(2);
  expect(await css(page, `${CARD}`, "border-top-style")).toBe("none");
  // Pastille : noir sur bleu lavande ; sélection en bleu lavande ; question en gras.
  expect(await css(page, `${CARD} .pill.on`, "background-color")).toBe("rgb(177, 185, 249)");
  expect(await css(page, `${CARD} .pill.on`, "color")).toBe("rgb(0, 0, 0)");
  expect(await css(page, `${CARD} .opt.sel`, "color")).toBe("rgb(177, 185, 249)");
  expect(Number(await css(page, `${CARD} .qtext`, "font-weight"))).toBeGreaterThanOrEqual(700);
  expect(await css(page, `${CARD} .desc`, "color")).toBe("rgb(153, 153, 153)");
  // Valeurs relevées sur les pixels de 07 : question #FFFFFF, numéros et
  // règles #999999, options et « Chat about this » #CCCCCC.
  expect(await css(page, `${CARD} .qtext`, "color")).toBe("rgb(255, 255, 255)");
  expect(await css(page, `${CARD} .opt.sel .num`, "color")).toBe("rgb(153, 153, 153)");
  expect(await css(page, `${CARD} .rule`, "color")).toBe("rgb(153, 153, 153)");
  expect(await css(page, `${CARD} .opt:not(.sel)`, "color")).toBe("rgb(204, 204, 204)");
  expect(await css(page, `${CARD} .opt[data-kind="chat"] .num`, "color")).toBe("rgb(204, 204, 204)");
  // La carte prend la place de la saisie et de la ligne d'état.
  await expect(page.locator(PROMPT)).toHaveCount(0);
  await expect(page.getByTestId("status-line")).toHaveCount(0);

  mkdirSync(OUT_DIR, { recursive: true });
  const app = `${OUT_DIR}/07-app.png`;
  await page.screenshot({ path: app });
  if (!existsSync(REF)) return;
  const url = (p: string) => `data:image/png;base64,${readFileSync(p).toString("base64")}`;
  const side = await browser.newPage({ viewport: { width: 2300, height: 900 } });
  await side.setContent(`<!doctype html><html><body style="margin:0;background:#202020;color:#ddd;font:14px sans-serif">
    <div style="display:flex;gap:16px;padding:8px">
      <figure style="margin:0"><figcaption>Référence TUI 2.1.325 : 07-question.png</figcaption><img src="${url(REF)}"></figure>
      <figure style="margin:0"><figcaption>App (mode simulé) : carte J8</figcaption><img src="${url(app)}"></figure>
    </div></body></html>`);
  await side.screenshot({ path: `${OUT_DIR}/07-comparaison.png`, fullPage: true });
  await side.close();
});

test("↓ puis Entrée : answer_question avec le label, la carte se ferme, la saisie revient", async ({ page }) => {
  await open(page);
  const id = await ask(page, [LANG]);
  await page.keyboard.press("ArrowDown");
  await expect(page.locator(`${CARD} .opt.sel`)).toContainText("2. TypeScript");
  await page.keyboard.press("Enter");
  await closed(page);
  expect(await calls(page, "answer_question")).toEqual([
    { cmd: "answer_question", args: { request_id: id, answers: { [LANG.question]: "TypeScript" }, response: null } },
  ]);
  await expect(page.locator(PROMPT)).toBeFocused();
  // Résultat de l'outil comme dans `t4`.
  await expect
    .poll(() => lastToolResult(page))
    .toContain('"Quel langage préfères-tu ?"="TypeScript"');
});

test("chiffre : choisit directement l'option ; ↑ fait le tour par le bas", async ({ page }) => {
  await open(page);
  await ask(page, [LANG]);
  await page.keyboard.press("ArrowUp");
  await expect(page.locator(`${CARD} .opt.sel`)).toContainText("5. Chat about this");
  await page.keyboard.press("3");
  await closed(page);
  const [c] = await calls(page, "answer_question");
  expect(c.args.answers).toEqual({ [LANG.question]: "Python" });
});

test("« Type something. » : réponse libre", async ({ page }) => {
  await open(page);
  await ask(page, [LANG]);
  await page.keyboard.press("4");
  await expect(page.locator(`${CARD} input[aria-label="Autre réponse"]`)).toBeFocused();
  // Les chiffres tapés dans le champ libre sont du texte.
  await page.keyboard.type("Zig 0.14");
  await page.keyboard.press("Enter");
  await closed(page);
  const [c] = await calls(page, "answer_question");
  expect(c.args.answers).toEqual({ [LANG.question]: "Zig 0.14" });
});

test("multi-sélection : Espace et Entrée cochent, Submit envoie la liste", async ({ page }) => {
  await open(page);
  const q = { ...LANG, question: "Lesquels utilises-tu ?", header: "Langages", multi_select: true };
  await ask(page, [q]);
  await page.keyboard.press("Space"); // Rust
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter"); // Python
  await expect(page.locator(`${CARD} .opt`).nth(0)).toContainText("1. [✔] Rust");
  await expect(page.locator(`${CARD} .opt`).nth(1)).toContainText("2. [ ] TypeScript");
  await expect(page.locator(`${CARD} .opt`).nth(2)).toContainText("3. [✔] Python");
  expect(await calls(page, "answer_question")).toHaveLength(0);
  await page.keyboard.press("5"); // Submit
  await closed(page);
  const [c] = await calls(page, "answer_question");
  expect(c.args.answers).toEqual({ [q.question]: ["Rust", "Python"] });
});

test("plusieurs questions : onglets, relecture, une seule réponse avec tout", async ({ page }) => {
  await open(page);
  const q2: QItem = {
    question: "Quelle couleur ?",
    header: "Couleur",
    multi_select: false,
    options: [
      { label: "Rouge", description: "" },
      { label: "Bleu", description: "" },
    ],
  };
  await ask(page, [LANG, q2]);
  await expect(page.getByTestId("question-tabs")).toHaveText(/←\s+☐ Langage\s+☐ Couleur\s+✔ Submit\s+→/);
  await expect(page.getByTestId("question-footer")).toHaveText(
    "Enter to select · Tab/Arrow keys to navigate · Esc to cancel",
  );
  await page.keyboard.press("Enter"); // Rust → onglet suivant
  await expect(page.getByTestId("question-text")).toHaveText("Quelle couleur ?");
  await expect(page.getByTestId("question-tabs")).toContainText("☒ Langage");
  await page.keyboard.press("2"); // Bleu → relecture
  await expect(page.locator(CARD)).toContainText("Review your answers");
  await expect(page.locator(CARD)).toContainText("→ Bleu");
  expect(await calls(page, "answer_question")).toHaveLength(0);
  await page.keyboard.press("Enter"); // Submit answers
  await closed(page);
  const [c] = await calls(page, "answer_question");
  expect(c.args.answers).toEqual({ [LANG.question]: "Rust", [q2.question]: "Bleu" });
});

test("Échap : ignore (skip_question skipped, t10) si ignorable, sinon ne fait rien", async ({ page }) => {
  await open(page);
  // Non ignorable : rien n'est envoyé, la carte reste, pas de « Esc to cancel ».
  const blocked = await ask(page, [LANG], false);
  await expect(page.getByTestId("question-footer")).toHaveText("Enter to select · ↑/↓ to navigate");
  await page.keyboard.press("Escape");
  await page.waitForTimeout(150);
  await expect(page.locator(CARD)).toBeVisible();
  expect(await calls(page, "skip_question")).toHaveLength(0);
  expect(await calls(page, "interrupt")).toHaveLength(0);
  // « Chat about this » est grisé et inactif sans la capacité.
  await page.keyboard.press("5");
  await page.waitForTimeout(150);
  await expect(page.locator(CARD)).toBeVisible();
  expect(await calls(page, "skip_question")).toHaveLength(0);
  await page.keyboard.press("1");
  await closed(page);
  expect((await calls(page, "answer_question"))[0].args.request_id).toBe(blocked);
  await page.waitForFunction(() => (window as unknown as W).__orch.store.status === "Idle");

  // Ignorable (capacité dans `init.capabilities`) : « ignorer » au premier Échap.
  const id = await ask(page, [LANG]);
  await page.keyboard.press("Escape");
  await closed(page);
  expect(await calls(page, "skip_question")).toEqual([
    { cmd: "skip_question", args: { request_id: id, reason: "skipped" } },
  ]);
  expect(await calls(page, "interrupt")).toHaveLength(0);
  await expect
    .poll(() => lastToolResult(page))
    .toBe("The user did not answer the questions.");
});

test("« Chat about this » : skip_question dismissed, puis focus dans la saisie", async ({ page }) => {
  await open(page);
  const id = await ask(page, [LANG]);
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("Enter");
  await closed(page);
  expect(await calls(page, "skip_question")).toEqual([
    { cmd: "skip_question", args: { request_id: id, reason: "dismissed" } },
  ]);
  await expect(page.locator(PROMPT)).toBeFocused();
});

test("t4 : la question de la fixture, Bleu choisi, donne les answers envoyés au CLI", async ({ page }) => {
  await open(page);
  const fx = JSON.parse(readFileSync(`${ROOT}fixtures/ui-events/t4.json`, "utf8")) as { kind: string; questions?: QItem[] }[];
  const qs = fx.find((e) => e.kind === "question")!.questions!;
  const sentLine = readFileSync(`${ROOT}fixtures/stream-json/t4.jsonl`, "utf8")
    .split("\n")
    .find((l) => l.startsWith(">>> ") && l.includes('"answers"'))!;
  const expected = JSON.parse(sentLine.slice(4)).response.response.updatedInput.answers;
  await ask(page, qs);
  await page.keyboard.press("2");
  await closed(page);
  const [c] = await calls(page, "answer_question");
  expect(c.args.answers).toEqual(expected);
});
