// Recette J14 (auto-amélioration) sur l'exe compilé, pilotée par CDP (aucune
// touche envoyée au bureau). L'exe est copié dans %LOCALAPPDATA%\orchestrator
// (comme l'installeur NSIS par utilisateur : le clone va dans
// %LOCALAPPDATA%\orchestrator\repo). La session principale tourne dans
// `.claude-work/j14/sandbox` ; claude.exe en haiku (ANTHROPIC_MODEL).
//
//   node tools/recette/j14.mjs <phase>
//
// Phases :
//   github   premier lancement avec l'URL compilée (GitHub, SSH) : issue du
//            clone, /ameliorer grisé avec la raison si le clone échoue.
//   broken   interface disque « aaaaaaa » active, précédente « bbbbbbb »
//            cassée (exception au chargement) : retour arrière vers la
//            cassée, watchdog de bascule, retour à l'embarquée ; même pid,
//            mêmes lignes, 0 trou.
//   cycle    clone local (config.json) puis un vrai /ameliorer en haiku :
//            bascule avec session active, fusion ff, hash = main du clone,
//            fournisseur disque face aux chemins malveillants.
//   rollback /revenir-version par la palette : réactivation, puis revert
//            par le cycle (commit Revert "…").
//   startup  version disque cassée au démarrage : règle des deux échecs.
//
// Résultats : `.claude-work/j14/<phase>.json` et captures.
import { copyFileSync, cpSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { join, relative } from "node:path";
import { ROOT, closeApp, launchApp, log, sleep, tree, waitGone } from "./lib.mjs";

const WORK = join(ROOT, ".claude-work", "j14");
const SANDBOX = join(WORK, "sandbox");
const DATA = join(process.env.LOCALAPPDATA, "orchestrator");
const UI = join(DATA, "ui");
const REPO = join(DATA, "repo");
const EXE_SRC = join(ROOT, "src-tauri", "target", "release", "orchestrator.exe");
const EXE = join(DATA, "orchestrator.exe");
const PORT = 9341;

mkdirSync(SANDBOX, { recursive: true });
const phase = process.argv[2];
const out = { phase, started: new Date().toISOString(), steps: [] };
const consoleMsgs = [];

function save() {
  writeFileSync(join(WORK, `${phase}.json`), JSON.stringify(out, null, 2));
}
function note(k, v) {
  out[k] = v;
  log(k, typeof v === "string" ? v : JSON.stringify(v));
  save();
}
async function shot(page, name) {
  try {
    await page.screenshot({ path: join(WORK, `${phase}-${name}.png`), timeout: 15_000 });
  } catch (e) {
    log("capture", name, e.message.split("\n")[0]);
  }
}
const git = (dir, ...args) => execFileSync("git", ["-C", dir, ...args], { encoding: "utf8" }).trim();
const invoke = (page, cmd, args = {}) =>
  page.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args]);
const uiInfo = (page) => invoke(page, "get_ui_info");

/** Pied de page : statut, lignes, trous, dernier seq. */
const footer = (page) =>
  page.evaluate(() => {
    const s = document.querySelector('[data-testid="footer-status"]');
    if (!s) return null;
    return {
      text: s.textContent,
      rows: Number(s.dataset.rows),
      gaps: Number(s.dataset.gaps),
      lastSeq: Number(s.dataset.lastSeq),
    };
  });
const statusText = (page) => page.evaluate(() => document.querySelector('[data-testid="status-line"]')?.textContent ?? "");

function claudePids(rootPid) {
  return tree(rootPid)
    .filter((p) => /^claude\.exe$/i.test(p.name))
    .map((p) => p.pid)
    .sort();
}

async function waitFor(what, fn, ms, every = 100) {
  const end = Date.now() + ms;
  let last;
  while (Date.now() < end) {
    try {
      last = await fn();
      if (last) return last;
    } catch (e) {
      last = e.message;
    }
    await sleep(every);
  }
  throw new Error(`${what} : délai dépassé (dernier : ${JSON.stringify(last)})`);
}

async function ready(page, ms = 90_000) {
  await waitFor(
    "saisie prête",
    () =>
      page.evaluate(() => {
        const ta = document.querySelector('[aria-label="Message"]');
        return !!ta && !ta.disabled && ta.getAttribute("placeholder") !== "session non démarrée";
      }),
    ms,
  );
}

async function type(page, text) {
  const ta = page.locator('[aria-label="Message"]');
  await ta.click();
  await ta.fill(text);
  await sleep(200);
  await page.keyboard.press("Escape").catch(() => {});
  await ta.fill(text);
  await page.keyboard.press("Enter");
}

/** Message à la session principale et attente du repos. */
async function exchange(page, text) {
  const before = (await footer(page))?.rows ?? 0;
  await type(page, text);
  await waitFor("réponse", async () => {
    const f = await footer(page);
    return f && f.rows > before && /^Idle/.test(f.text) ? f : null;
  }, 120_000, 250);
  await sleep(1000);
}

function hookConsole(page) {
  page.on("console", (m) => {
    const t = m.text();
    if (m.type() === "error" || m.type() === "warning" || /Content Security Policy|CSP/i.test(t))
      consoleMsgs.push({ t: Date.now(), type: m.type(), text: t.slice(0, 300) });
  });
  page.on("pageerror", (e) => consoleMsgs.push({ t: Date.now(), type: "pageerror", text: String(e).slice(0, 300) }));
}

async function launch() {
  copyFileSync(EXE_SRC, EXE);
  const app = await launchApp({ exe: EXE, args: [SANDBOX], port: PORT, env: {
      ORCH_LOG_DIR: join(WORK, "logs"),
      // Dossier WebView2 à part : une autre instance de l'app (celle de
      // l'utilisateur) tient le dossier par défaut avec d'autres arguments.
      WEBVIEW2_USER_DATA_FOLDER: join(WORK, "webview"),
    },
  });
  hookConsole(app.page);
  return app;
}

async function stop(app) {
  const pids = tree(app.pid).map((p) => p.pid);
  await app.browser.close().catch(() => {});
  closeApp(app.pid);
  if (!(await waitGone(pids, 10_000))) closeApp(app.pid, { force: true });
  await waitGone(pids, 10_000);
}

/** Version d'interface écrite à la main (comme `Store::install`). */
function installVersion(hash, buildDir) {
  const dir = join(UI, hash);
  rmSync(dir, { recursive: true, force: true });
  cpSync(buildDir, dir, { recursive: true });
  const files = {};
  const walk = (d) => {
    for (const e of readdirSync(d, { withFileTypes: true })) {
      const p = join(d, e.name);
      if (e.isDirectory()) walk(p);
      else {
        const rel = relative(dir, p).split("\\").join("/");
        if (rel !== "manifest.json") files[rel] = createHash("sha256").update(readFileSync(p)).digest("hex");
      }
    }
  };
  walk(dir);
  writeFileSync(join(dir, "manifest.json"), JSON.stringify({ version: 1, hash, files }, null, 2));
}

function viteBuild(hash, outDir) {
  rmSync(outDir, { recursive: true, force: true });
  execFileSync("node", ["node_modules/vite/bin/vite.js", "build", "--outDir", outDir, "--emptyOutDir"], {
    cwd: ROOT,
    env: { ...process.env, ORCH_UI_HASH: hash },
    stdio: "pipe",
  });
}

function uiDirs() {
  return existsSync(UI) ? readdirSync(UI, { withFileTypes: true }).filter((e) => e.isDirectory()).map((e) => e.name) : [];
}

async function snapshot(app, label) {
  const f = await footer(app.page);
  const s = { label, at: Date.now(), claude: claudePids(app.pid), footer: f, status: await statusText(app.page) };
  out.steps.push(s);
  save();
  return s;
}

// ---------------------------------------------------------------------------

async function phaseGithub() {
  rmSync(join(DATA, "config.json"), { force: true });
  const app = await launch();
  const t0 = Date.now();
  await ready(app.page);
  const info = await waitFor(
    "issue du clone",
    async () => {
      const i = await uiInfo(app.page);
      return i.repo.state !== "cloning" && i.repo.state !== "unknown" ? i : null;
    },
    15 * 60_000,
    500,
  );
  note("repo", info.repo);
  note("clone_ms", Date.now() - t0);
  // Palette : /ameliorer grisé avec la raison (ou disponible).
  const ta = app.page.locator('[aria-label="Message"]');
  await ta.click();
  await ta.fill("/amel");
  await sleep(500);
  note("palette", await app.page.evaluate(() => document.querySelector(".palette, [data-testid=palette]")?.textContent ?? ""));
  await shot(app.page, "palette");
  await ta.fill("");
  note("console", consoleMsgs);
  await stop(app);
}

async function phaseBroken() {
  const tmp = join(WORK, "builds");
  const good = join(tmp, "good");
  viteBuild("aaaaaaa", good);
  // Interface cassée : son propre hash (un signal « prêt » porterait le bon
  // hash) et une exception à la fin du script d'entrée, avant le montage.
  const bad = join(tmp, "bad");
  viteBuild("bbbbbbb", bad);
  const entry = /src="\/(assets\/index-[^"]+\.js)"/.exec(readFileSync(join(bad, "index.html"), "utf8"))[1];
  const p = join(bad, entry);
  const src = readFileSync(p, "utf8");
  // Le montage est le dernier appel du script d'entrée (`mount(App, …)`) :
  // l'exception est levée en tête du module, avant lui.
  writeFileSync(p, 'throw new Error("recette J14 : interface cassée au montage");\n' + src);
  note("broken_entry", entry);
  rmSync(UI, { recursive: true, force: true });
  mkdirSync(UI, { recursive: true });
  installVersion("aaaaaaa", good);
  installVersion("bbbbbbb", bad);
  writeFileSync(
    join(UI, "state.json"),
    JSON.stringify({ bad: [], installs: ["bbbbbbb", "aaaaaaa"], activations: ["bbbbbbb", "aaaaaaa"], startup_failures: {} }),
  );
  writeFileSync(join(UI, "active.json"), JSON.stringify({ active: "aaaaaaa", previous: "bbbbbbb" }));

  const app = await launch();
  await ready(app.page);
  note("info_start", await uiInfo(app.page));
  await exchange(app.page, "Réponds juste OK.");
  const before = await snapshot(app, "avant retour arrière");
  await shot(app.page, "avant");
  const t0 = Date.now();
  const r = await invoke(app.page, "ui_rollback");
  note("rollback_reply", { source: r.source, active: r.active_hash, previous: r.previous_hash });
  // La cassée se charge (exception), puis le watchdog revient en arrière.
  const info = await waitFor(
    "retour automatique",
    async () => {
      const i = await uiInfo(app.page).catch(() => null);
      const f = await footer(app.page).catch(() => null);
      return i && i.active_hash !== "bbbbbbb" && f ? { i, f } : null;
    },
    20_000,
    50,
  );
  const t1 = Date.now();
  await ready(app.page);
  await sleep(1500);
  const after = await snapshot(app, "après retour automatique");
  await shot(app.page, "apres");
  const state = JSON.parse(readFileSync(join(UI, "state.json"), "utf8"));
  note("result", {
    fallback_ms: t1 - t0,
    info_after: { source: info.i.source, active: info.i.active_hash, previous: info.i.previous_hash },
    bad: state.bad,
    same_claude: JSON.stringify(before.claude) === JSON.stringify(after.claude),
    rows: [before.footer.rows, after.footer.rows],
    gaps_after: after.footer.gaps,
    lastSeq: [before.footer.lastSeq, after.footer.lastSeq],
  });
  note("console", consoleMsgs);
  await stop(app);
}

async function phaseStartup() {
  // Version cassée active au démarrage : 1er lancement → embarquée pour
  // cette exécution ; 2e → marquée fautive.
  const bad = join(WORK, "builds", "bad");
  rmSync(UI, { recursive: true, force: true });
  mkdirSync(UI, { recursive: true });
  installVersion("bbbbbbb", bad);
  writeFileSync(join(UI, "state.json"), JSON.stringify({ bad: [], installs: ["bbbbbbb"], activations: ["bbbbbbb"] }));
  writeFileSync(join(UI, "active.json"), JSON.stringify({ active: "bbbbbbb", previous: null }));
  const runs = [];
  for (let i = 1; i <= 2; i++) {
    const app = await launch();
    const t0 = app.tSpawn;
    const info = await waitFor(
      "repli de démarrage",
      async () => {
        const i = await uiInfo(app.page).catch(() => null);
        return i && i.source === "embedded" ? i : null;
      },
      40_000,
      100,
    );
    await ready(app.page, 60_000);
    const t1 = Date.now();
    const state = JSON.parse(readFileSync(join(UI, "state.json"), "utf8"));
    runs.push({ run: i, recovered_ms: t1 - t0, source: info.source, active: info.active_hash, state });
    await stop(app);
  }
  note("runs", runs);
  note("console", consoleMsgs);
}

async function improveTimeline(page, id, ms) {
  const seen = [];
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const i = await uiInfo(page).catch(() => null);
    const st = i?.improve;
    if (st && (!id || st.id === id)) {
      if (seen.at(-1)?.step !== st.step) {
        seen.push({ step: st.step, at: Date.now(), detail: st.detail, commit: st.commit, session: st.session });
        log("étape", st.step, st.detail ?? "");
        note("timeline", seen);
      }
      if (["merged", "restart_required", "failed", "cancelled", "rolled_back"].includes(st.step)) return { st, seen, info: i };
    }
    await sleep(500);
  }
  throw new Error("cycle non terminé");
}

async function phaseCycle() {
  writeFileSync(
    join(DATA, "config.json"),
    JSON.stringify({ repo_url: "file:///" + ROOT.split("\\").join("/"), improve_model: "haiku" }, null, 2),
  );
  rmSync(UI, { recursive: true, force: true });
  const devHead = git(ROOT, "rev-parse", "HEAD");
  const app = await launch();
  await ready(app.page);
  const info0 = await waitFor(
    "clone prêt",
    async () => {
      const i = await uiInfo(app.page);
      return i.repo.state === "ready" || i.repo.state === "disabled" ? i : null;
    },
    10 * 60_000,
    500,
  );
  note("repo", info0.repo);
  if (info0.repo.state !== "ready") throw new Error("clone indisponible");
  const mainBefore = git(REPO, "rev-parse", "main");
  await exchange(app.page, "Réponds juste OK.");
  const before = await snapshot(app, "avant /ameliorer");
  await type(app.page, "/ameliorer ajoute un commentaire d'une ligne en tête de src/lib/components/StatusLine.svelte");
  const t0 = Date.now();
  const { st, seen, info } = await improveTimeline(app.page, null, 45 * 60_000);
  note("cycle_ms", Date.now() - t0);
  await ready(app.page);
  await sleep(2000);
  const after = await snapshot(app, "après bascule");
  await shot(app.page, "apres-cycle");
  const mainAfter = git(REPO, "rev-parse", "main");
  const switching = seen.find((s) => s.step === "switching");
  const merged = seen.find((s) => s.step === "merged");
  note("result", {
    final: st.step,
    detail: st.detail,
    active: info.active_hash,
    source: info.source,
    clone_main_short: git(REPO, "rev-parse", "--short", "main"),
    main_moved: [mainBefore, mainAfter],
    ff: git(REPO, "merge-base", "--is-ancestor", mainBefore, mainAfter) === "" ? "oui" : "?",
    commits_added: git(REPO, "log", "--format=%h %s", `${mainBefore}..${mainAfter}`),
    files: git(REPO, "diff", "--stat", mainBefore, mainAfter),
    origin_main_to_main: git(REPO, "log", "--oneline", "origin/main..main"),
    pushurl: git(REPO, "config", "--get", "remote.origin.pushurl"),
    dev_head_unchanged: git(ROOT, "rev-parse", "HEAD") === devHead,
    switch_to_merged_ms: switching && merged ? merged.at - switching.at : null,
    same_claude: JSON.stringify(before.claude) === JSON.stringify(after.claude),
    claude: [before.claude, after.claude],
    rows: [before.footer.rows, after.footer.rows],
    gaps_after: after.footer.gaps,
    lastSeq: [before.footer.lastSeq, after.footer.lastSeq],
    status_line: after.status,
    ui_dirs: uiDirs(),
  });
  // Fournisseur disque : chemins malveillants. `fetch` est interdit par la
  // CSP (connect-src) : on navigue la page vers chaque chemin (la
  // navigation n'est pas soumise à connect-src) et on lit ce qui est servi.
  // Chromium normalise `..` et `%2e%2e` avant la requête ; restent les
  // formes encodées (`%5c`) et les noms hors du manifeste.
  const probes = [];
  const csp0 = consoleMsgs.length;
  for (const p of [
    "/..%5cactive.json",
    "/%2e%2e%5cstate.json",
    "/assets/..%5c..%5cstate.json",
    "/C:%5cWindows%5cwin.ini",
    "/%5c%5c.%5cC:%5cWindows%5cwin.ini",
    "/manifest.json",
    "/..%2factive.json",
  ]) {
    try {
      const res = await app.page.goto("http://tauri.localhost" + p, { timeout: 15_000 });
      const body = await app.page.evaluate(() => document.documentElement.outerHTML.slice(0, 2000));
      probes.push({
        p,
        status: res?.status(),
        servedIndex: body.includes('<div id="app"') || body.includes("/assets/index-"),
        leaked: /\[fonts\]|"activations"|"active"|"files"|"bad"/.test(body),
      });
    } catch (e) {
      probes.push({ p, error: e.message.split("\n")[0] });
    }
  }
  await app.page.goto("http://tauri.localhost/", { timeout: 15_000 });
  await ready(app.page);
  note("probe_console", consoleMsgs.slice(csp0));
  note("probes", probes);
  note("console", consoleMsgs);
  await stop(app);
}

async function phaseRollback() {
  const app = await launch();
  await ready(app.page);
  const info0 = await waitFor("clone prêt", async () => {
    const i = await uiInfo(app.page);
    return i.repo.state === "ready" ? i : null;
  }, 5 * 60_000, 500);
  note("info_before", { source: info0.source, active: info0.active_hash, previous: info0.previous_hash });
  const mainBefore = git(REPO, "rev-parse", "main");
  await exchange(app.page, "Réponds juste OK.");
  const before = await snapshot(app, "avant /revenir-version");
  const t0 = Date.now();
  await type(app.page, "/revenir-version");
  const back = await waitFor(
    "réactivation",
    async () => {
      const i = await uiInfo(app.page).catch(() => null);
      const f = await footer(app.page).catch(() => null);
      return i && i.active_hash !== info0.active_hash && f ? i : null;
    },
    20_000,
    50,
  );
  await ready(app.page);
  const t1 = Date.now();
  note("reactivated", { ms: t1 - t0, source: back.source, active: back.active_hash });
  const { st, seen, info } = await improveTimeline(app.page, null, 45 * 60_000);
  await ready(app.page);
  await sleep(2000);
  const after = await snapshot(app, "après revert");
  await shot(app.page, "apres-revert");
  note("result", {
    final: st.step,
    detail: st.detail,
    active: info.active_hash,
    clone_main_short: git(REPO, "rev-parse", "--short", "main"),
    revert_commit: git(REPO, "log", "-1", "--format=%h %s", "main"),
    commits_added: git(REPO, "log", "--format=%h %s", `${mainBefore}..main`),
    same_claude: JSON.stringify(before.claude) === JSON.stringify(after.claude),
    rows: [before.footer.rows, after.footer.rows],
    gaps_after: after.footer.gaps,
    ui_dirs: uiDirs(),
    origin_main_to_main: git(REPO, "log", "--oneline", "origin/main..main"),
    timeline: seen,
  });
  note("console", consoleMsgs);
  await stop(app);
}

const phases = { github: phaseGithub, broken: phaseBroken, startup: phaseStartup, cycle: phaseCycle, rollback: phaseRollback };
if (!phases[phase]) {
  console.error(`phase inconnue : ${phase} (${Object.keys(phases).join(", ")})`);
  process.exit(2);
}
try {
  await phases[phase]();
  out.ok = true;
} catch (e) {
  out.ok = false;
  out.error = e.stack ?? String(e);
  log("ÉCHEC", e.message);
}
out.ended = new Date().toISOString();
save();
process.exit(out.ok ? 0 : 1);
