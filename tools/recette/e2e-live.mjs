// Recette J12 : essais réels de bout en bout dans l'app compilée, pilotée
// par CDP (aucune touche envoyée au bureau), avec le vrai claude.exe en
// haiku (ANTHROPIC_MODEL) dans `.claude-work/j12/sandbox`.
//
//   node tools/recette/e2e-live.mjs
//
// Essais : palette `/`, message en streaming, interruption par Échap,
// `/model`, AskUserQuestion répondue, 2 sous-agents (colonne des agents),
// reprise de la session avec son historique. Captures et résultats dans
// `.claude-work/j12/e2e-*.png` et `e2e-live.json`.
import { basename, join } from "node:path";
import { SANDBOX, WORK, closeApp, launchApp, log, save, sleep, tree, waitGone } from "./lib.mjs";

const results = [];
let step = 0;

async function shot(page, name) {
  const f = `e2e-${String(++step).padStart(2, "0")}-${name}.png`;
  try {
    await page.screenshot({ path: join(WORK, f), timeout: 15_000 });
    return f;
  } catch (e) {
    return `échec capture : ${e.message.split("\n")[0]}`;
  }
}

const status = (page) =>
  page.evaluate(() => document.querySelector('[data-testid="footer-status"]')?.textContent?.split(" · ")[0] ?? "");
const footer = (page) => page.evaluate(() => [...document.querySelectorAll("footer span")].map((s) => s.textContent));
const rowsText = (page) => page.evaluate(() => document.querySelector(".vlist")?.innerText ?? "");

async function waitStatus(page, re, ms = 90_000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const s = await status(page);
    if (re.test(s)) return { s, at: Date.now() };
    await sleep(25);
  }
  throw new Error(`statut ${re} non atteint (dernier : ${await status(page)})`);
}

async function send(page, text) {
  const ta = page.locator('[aria-label="Message"]');
  await ta.click();
  await ta.fill(text);
  await page.keyboard.press("Enter");
}

async function trial(name, fn) {
  log("essai", name);
  const r = { name };
  try {
    Object.assign(r, await fn());
    r.ok ??= true;
  } catch (e) {
    r.ok = false;
    r.error = e.message.split("\n")[0];
  }
  log(JSON.stringify(r));
  results.push(r);
  return r;
}

async function ready(page) {
  await page.waitForFunction(
    () => {
      const ta = document.querySelector('[aria-label="Message"]');
      return ta && !ta.disabled && ta.getAttribute("placeholder") !== "session non démarrée";
    },
    null,
    { timeout: 60_000 },
  );
}

async function firstSession() {
  const app = await launchApp({ args: [SANDBOX] });
  const { page } = app;
  await ready(page);
  await sleep(500);

  await trial("palette /", async () => {
    await page.locator('[aria-label="Message"]').click();
    await page.keyboard.type("/");
    await page.getByTestId("palette").waitFor({ timeout: 5000 });
    const entries = await page.evaluate(() => document.querySelector('[data-testid="palette"]').querySelectorAll('[role="option"]').length);
    await page.keyboard.type("mod");
    await sleep(300);
    const filtered = await page.evaluate(() => document.querySelector('[data-testid="palette"]')?.innerText.split("\n").slice(0, 4));
    const capture = await shot(page, "palette");
    await page.keyboard.press("Escape");
    await page.keyboard.press("Escape");
    await page.keyboard.press("Escape");
    await page.locator('[aria-label="Message"]').fill("");
    return { entries, filtered, capture, ok: entries > 10 };
  });

  await trial("message en streaming", async () => {
    const statuses = new Set();
    await send(page, "Écris deux phrases courtes sur la mer. Aucun outil.");
    const t0 = Date.now();
    let firstStreaming = null;
    const end = Date.now() + 90_000;
    for (;;) {
      const s = await status(page);
      statuses.add(s);
      if (s === "Streaming" && !firstStreaming) firstStreaming = Date.now() - t0;
      if (s === "Idle" && statuses.size > 1) break;
      if (Date.now() > end) throw new Error("pas de fin de tour");
      await sleep(20);
    }
    const text = await rowsText(page);
    return {
      statuses: [...statuses],
      firstStreamingMs: firstStreaming,
      turnMs: Date.now() - t0,
      footer: await footer(page),
      tail: text.slice(-300),
      capture: await shot(page, "streaming"),
      ok: statuses.has("Streaming"),
    };
  });

  await trial("interruption par Échap", async () => {
    await send(page, "Compte de 1 à 300 en toutes lettres, un nombre par ligne. Aucun outil.");
    await waitStatus(page, /^Streaming$/, 60_000);
    await sleep(1500);
    await page.locator('[aria-label="Message"]').focus();
    const t0 = Date.now();
    await page.keyboard.press("Escape");
    const idle = await waitStatus(page, /^Idle$/, 15_000);
    const text = await rowsText(page);
    return {
      escToIdleMs: idle.at - t0,
      interruptedShown: /Interrupted/.test(text),
      capture: await shot(page, "interruption"),
      ok: idle.at - t0 < 2000 && /Interrupted/.test(text),
    };
  });

  await trial("/model", async () => {
    await send(page, "/model");
    await page.getByTestId("model-picker").waitFor({ timeout: 5000 });
    const options = await page.evaluate(() =>
      [...document.querySelectorAll('[data-testid="model-picker"] [role="option"]')].map((o) => o.dataset.value),
    );
    const capture = await shot(page, "model-picker");
    // Choisir haiku : ↓ jusqu'à l'option, puis Entrée.
    const target = options.findIndex((v) => /haiku/i.test(v));
    const cur = await page.evaluate(() =>
      [...document.querySelectorAll('[data-testid="model-picker"] [role="option"]')].findIndex((o) => o.getAttribute("aria-selected") === "true"),
    );
    for (let i = cur; i < target; i++) await page.keyboard.press("ArrowDown");
    for (let i = cur; i > target; i--) await page.keyboard.press("ArrowUp");
    await page.keyboard.press("Enter");
    await sleep(1500);
    const hint = await page.evaluate(() => document.querySelector('[data-testid="prompt-hint"]')?.textContent);
    const closed = (await page.getByTestId("model-picker").count()) === 0;
    return { options, chosen: options[target], hint, closed, capture2: await shot(page, "model-choisi"), capture, ok: target >= 0 && closed };
  });

  await trial("AskUserQuestion répondue", async () => {
    await send(
      page,
      "Utilise l'outil AskUserQuestion pour me demander ma couleur préférée, en-tête « Couleur », options « Rouge » et « Bleu ». Ensuite répète ma réponse en un mot.",
    );
    await page.getByTestId("question-card").waitFor({ timeout: 90_000 });
    await sleep(300);
    const q = await page.evaluate(() => document.querySelector('[data-testid="question-card"]')?.innerText);
    const capture = await shot(page, "question");
    await page.keyboard.press("Enter");
    await page.getByTestId("question-card").waitFor({ state: "detached", timeout: 10_000 });
    await waitStatus(page, /^Idle$/, 90_000);
    const text = await rowsText(page);
    return { question: q.slice(0, 300), tail: text.slice(-300), capture, capture2: await shot(page, "question-repondue"), ok: true };
  });

  await trial("2 sous-agents", async () => {
    await send(
      page,
      "Utilise l'outil Agent pour lancer en parallèle, dans un seul message, deux sous-agents avec model haiku : subagent_type « agent-bleu » et subagent_type « agent-rouge », chacun avec le prompt « Réponds seulement OK, sans aucun outil. ». Quand les deux ont fini, dis « terminé ».",
    );
    const col = page.getByTestId("agent-column");
    const end = Date.now() + 120_000;
    let maxCount = 0;
    let during = null;
    let cardsSeen = [];
    while (Date.now() < end) {
      const info = await page.evaluate(() => {
        const c = document.querySelector('[data-testid="agent-column"]');
        return { text: c?.innerText ?? "", count: c?.querySelector('[data-testid="agent-count"]')?.textContent ?? "" };
      });
      const names = (info.text.match(/agent-(bleu|rouge)/g) ?? []).filter((v, i, a) => a.indexOf(v) === i);
      if (names.length > maxCount) {
        maxCount = names.length;
        cardsSeen = names;
        if (names.length >= 2 && !during) during = await shot(page, "agents-en-cours");
      }
      const s = await status(page);
      if (s === "Idle" && maxCount >= 2) break;
      await sleep(200);
    }
    await sleep(2000);
    const colText = await col.innerText();
    return {
      agentsSeen: cardsSeen,
      column: colText.slice(0, 600),
      capture: during,
      capture2: await shot(page, "agents-fin"),
      ok: maxCount >= 2,
    };
  });

  const desc = tree(app.pid).map((p) => p.pid).filter((p) => p !== app.pid);
  await app.browser.close().catch(() => {});
  closeApp(app.pid);
  const gone = await waitGone([app.pid, ...desc], 12_000);
  results.push({ name: "fermeture après la 1re session", ok: !!gone });
}

async function resumeSession() {
  const app = await launchApp({ args: [] });
  const { page } = app;
  await trial("reprise avec historique", async () => {
    await page.getByTestId("project-picker").waitFor({ timeout: 20_000 });
    await page.getByLabel("Filtrer les projets").fill(basename(WORK) + "\\" + basename(SANDBOX));
    await sleep(500);
    const rows = await page.getByTestId("project-row").count();
    if (rows < 1) throw new Error("projet du bac à sable absent du sélecteur");
    await page.keyboard.press("Enter");
    await page.getByTestId("session-picker").waitFor({ timeout: 20_000 });
    await page.getByTestId("session-row").first().waitFor({ timeout: 20_000 });
    const sessions = await page.getByTestId("session-row").count();
    const c1 = await shot(page, "sessions");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    await ready(page);
    await sleep(1500);
    const hist = await rowsText(page);
    const rowsN = await page.evaluate(() => document.querySelector('[data-testid="footer-status"]')?.textContent);
    const c2 = await shot(page, "reprise-historique");
    await send(page, "Quelle couleur ai-je choisie tout à l'heure ? Un mot, aucun outil.");
    await waitStatus(page, /^(Requesting|Streaming)$/, 30_000);
    await waitStatus(page, /^Idle$/, 90_000);
    const after = await rowsText(page);
    return {
      sessions,
      footer: rowsN,
      historyHasEarlierTurn: /mer/i.test(hist) && /AskUserQuestion|Couleur|Rouge/i.test(hist),
      historyHead: hist.slice(0, 400),
      answer: after.slice(-200),
      captures: [c1, c2, await shot(page, "reprise-reponse")],
      ok: /mer/i.test(hist),
    };
  });
  const desc = tree(app.pid).map((p) => p.pid).filter((p) => p !== app.pid);
  await app.browser.close().catch(() => {});
  closeApp(app.pid);
  const gone = await waitGone([app.pid, ...desc], 12_000);
  results.push({ name: "fermeture après la reprise", ok: !!gone });
}

try {
  await firstSession();
  await resumeSession();
} finally {
  log("écrit", save("e2e-live.json", results));
}
