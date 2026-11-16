// Recette J12 : démarrage, mémoire au repos, fenêtres console, et processus
// claude.exe restants après une fermeture normale, forcée, ou un rechargement.
//
//   node tools/recette/startup.mjs [nombre de démarrages normaux, 3 par défaut]
//
// Chaque démarrage ouvre `.claude-work/j12/sandbox` (argument de l'app) :
// claude.exe est lancé et reçoit `initialize`, aucun message n'est envoyé
// (aucun tour facturé). Résultats : `.claude-work/j12/startup.json`.
import { mkdirSync } from "node:fs";
import {
  SANDBOX,
  appMemory,
  closeApp,
  launchApp,
  log,
  processes,
  save,
  sleep,
  tree,
  visibleWindows,
  waitGone,
} from "./lib.mjs";

const RUNS = Number(process.argv[2] ?? 3);
const NOT_READY = "session non démarrée";
mkdirSync(SANDBOX, { recursive: true });

/** Instant (epoch ms) où la saisie quitte « session non démarrée » : réponse à initialize. */
async function readyAt(page) {
  const h = await page.waitForFunction(
    (nr) => {
      const ta = document.querySelector('[aria-label="Message"]');
      return ta && ta.getAttribute("placeholder") !== nr ? Date.now() : false;
    },
    NOT_READY,
    { polling: 10, timeout: 30_000 },
  );
  return h.jsonValue();
}

async function paintAt(page) {
  return page.evaluate(() => {
    const fcp = performance.getEntriesByType("paint").find((e) => e.name === "first-contentful-paint");
    const fp = performance.getEntriesByType("paint").find((e) => e.name === "first-paint");
    return {
      timeOrigin: performance.timeOrigin,
      fp: fp ? performance.timeOrigin + fp.startTime : null,
      fcp: fcp ? performance.timeOrigin + fcp.startTime : null,
      visibility: document.visibilityState,
      url: location.href,
    };
  });
}

/** Descendants hors webview : claude.exe et ce qu'il lance. */
function nonWebview(pid) {
  return tree(pid).filter((p) => p.pid !== pid && !/^msedgewebview2\.exe$/i.test(p.name));
}

async function oneRun(i, mode) {
  log(`démarrage ${i} (${mode})`);
  const app = await launchApp({ args: [SANDBOX] });
  const { page, pid, tSpawn, tCdp } = app;
  let ready = null;
  let readyNote = "mesuré";
  try {
    ready = await readyAt(page);
    if (ready <= tCdp + 30) readyNote = "déjà prêt à la connexion CDP : borne haute";
  } catch (e) {
    readyNote = `pas prêt : ${e.message.split("\n")[0]}`;
  }
  const paint = await paintAt(page);
  // Repos : 10 s après initialize.
  await sleep(10_000);
  const mem = appMemory(pid);
  const all = tree(pid);
  const wins = visibleWindows(all.map((p) => p.pid));
  const consoles = wins.filter((w) => /ConsoleWindowClass|CASCADIA_HOSTING/i.test(w.cls));
  const children = nonWebview(pid);
  const claudeBefore = children.filter((p) => /^claude\.exe$/i.test(p.name)).map((p) => p.pid);
  const res = {
    run: i,
    mode,
    pid,
    windowPaintMs: paint.fcp ? Math.round(paint.fcp - tSpawn) : null,
    firstPaintMs: paint.fp ? Math.round(paint.fp - tSpawn) : null,
    navStartMs: Math.round(paint.timeOrigin - tSpawn),
    cdpMs: tCdp - tSpawn,
    initializeMs: ready ? ready - tSpawn : null,
    initializeNote: readyNote,
    visibility: paint.visibility,
    memoryIdle: mem,
    windows: wins,
    consoleWindows: consoles,
    descendants: children,
  };

  if (mode === "reload") {
    const tReload = Date.now();
    await page.reload();
    const r2 = await readyAt(page).catch(() => null);
    await sleep(1000);
    const after = nonWebview(pid).filter((p) => /^claude\.exe$/i.test(p.name)).map((p) => p.pid);
    const oldGone = await waitGone(claudeBefore, 8000);
    res.reload = {
      claudeBefore: [...claudeBefore],
      claudeAfter: after,
      oldClaudeGoneMs: oldGone ? oldGone - tReload : null,
      oldClaudeStillAlive: claudeBefore.filter((p) => after.includes(p)),
      newSessionReady: r2 !== null,
    };
    claudeBefore.splice(0, claudeBefore.length, ...after);
  }

  await app.browser.close().catch(() => {});
  const allDesc = nonWebview(pid).map((p) => p.pid);
  const t0 = Date.now();
  res.close = closeApp(pid, { force: mode === "force" });
  // Fermeture normale : stdin fermé, 3 s d'attente, puis le Job (plan 1.9).
  const gone = await waitGone([pid, ...allDesc], 10_000);
  const left = processes().filter((p) => allDesc.includes(p.pid) || p.pid === pid);
  res.afterClose = {
    allGoneMs: gone ? gone - t0 : null,
    left,
  };
  if (left.length) {
    log("reste après fermeture, nettoyage forcé", left);
    for (const p of left) closeApp(p.pid, { force: true });
  }
  log(JSON.stringify({ run: i, mode, paint: res.windowPaintMs, init: res.initializeMs, mem: mem.totalMb, consoles: consoles.length, left: left.length }));
  return res;
}

const results = [];
for (let i = 1; i <= RUNS; i++) results.push(await oneRun(i, "normal"));
results.push(await oneRun(RUNS + 1, "force"));
results.push(await oneRun(RUNS + 2, "reload"));
const f = save("startup.json", results);
log("écrit", f);
