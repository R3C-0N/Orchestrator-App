// Recette J12 : mesures de rendu et de mémoire dans l'app compilée
// (WebView2), pilotée par CDP. Reprend les mesures de `e2e/scroll.spec.ts`.
//
//   node tools/recette/perf.mjs mock     interface seule, `?mock=long` (3 000 lignes)
//   node tools/recette/perf.mjs t15      t15 rejouée par le vrai cœur Rust
//                                        (faux claude.exe : replay-claude.cmd)
//   node tools/recette/perf.mjs latency  tours en streaming à 100 text_delta/s
//                                        par le vrai cœur (pied « latence token »)
//
// Résultats : `.claude-work/j12/perf-<mode>.json`, captures `perf-<mode>-*.png`.
import { join } from "node:path";
import { ROOT, SANDBOX, WORK, appMemory, closeApp, launchApp, log, save, sleep, waitGone, tree } from "./lib.mjs";

const MODE = process.argv[2] ?? "mock";
const REPLAY = join(ROOT, "tools", "recette", "replay-claude.cmd");
const out = { mode: MODE, at: new Date().toISOString() };

function installFrameRecorder() {
  // `gen` : une boucle par enregistrement (un stop() suivi d'un start()
  // dans la même image ne doit pas laisser tourner l'ancienne boucle).
  let gen = 0;
  let rec;
  window.__frames = {
    start() {
      rec = { d: [], traveledPx: 0, viewportPx: 0, minIndex: Infinity, maxIndex: -Infinity };
      const my = ++gen;
      let last = 0;
      let lastTop = null;
      const loop = (t) => {
        if (my !== gen) return;
        if (last) rec.d.push(t - last);
        last = t;
        const el = document.querySelector(".vlist");
        if (el) {
          rec.viewportPx = el.clientHeight;
          if (lastTop !== null) rec.traveledPx += Math.abs(el.scrollTop - lastTop);
          lastTop = el.scrollTop;
          const vp = el.getBoundingClientRect();
          for (const r of el.querySelectorAll("[data-index]")) {
            const b = r.getBoundingClientRect();
            if (b.bottom <= vp.top || b.top >= vp.bottom) continue;
            const i = Number(r.dataset.index);
            if (i < rec.minIndex) rec.minIndex = i;
            if (i > rec.maxIndex) rec.maxIndex = i;
          }
        }
        requestAnimationFrame(loop);
      };
      requestAnimationFrame(loop);
    },
    stop() {
      gen++;
      return rec;
    },
  };
}

function summarize(rec) {
  const f = rec.d;
  const total = f.reduce((a, b) => a + b, 0);
  const s = [...f].sort((a, b) => a - b);
  return {
    viewports: +(rec.traveledPx / rec.viewportPx).toFixed(1),
    rowsSpan: rec.maxIndex - rec.minIndex,
    frames: f.length,
    seconds: +(total / 1000).toFixed(2),
    fps: +((f.length * 1000) / total).toFixed(1),
    over33pct: +((100 * f.filter((x) => x > 33).length) / f.length).toFixed(2),
    over50: f.filter((x) => x > 50).length,
    p99: +s[Math.floor(0.99 * (s.length - 1))].toFixed(1),
    max: +s[s.length - 1].toFixed(1),
  };
}

async function dragScroll(page, ms, pxPerSec) {
  await page.evaluate(
    ({ ms, pxPerSec }) =>
      new Promise((resolve) => {
        const el = document.querySelector(".vlist");
        el.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
        let dir = -1;
        let last = performance.now();
        const end = last + ms;
        const step = (t) => {
          const dt = t - last;
          last = t;
          const before = el.scrollTop;
          el.scrollTop = before + (dir * pxPerSec * dt) / 1000;
          if (el.scrollTop === before) dir = -dir;
          if (t < end) requestAnimationFrame(step);
          else {
            window.dispatchEvent(new PointerEvent("pointerup", { bubbles: true }));
            resolve();
          }
        };
        requestAnimationFrame(step);
      }),
    { ms, pxPerSec },
  );
}

async function wheelScroll(page, ms) {
  const end = Date.now() + ms;
  let dir = -1;
  let flip = Date.now() + 2500;
  while (Date.now() < end) {
    await page.mouse.wheel(0, dir * 100);
    await page.waitForTimeout(8);
    if (Date.now() > flip) ((dir = -dir), (flip = Date.now() + 2500));
  }
}

async function scrollBoth(page, ms) {
  await page.mouse.move(400, 300);
  await page.evaluate(() => window.__frames.start());
  await dragScroll(page, ms, 3000);
  const drag = summarize(await page.evaluate(() => window.__frames.stop()));
  await page.evaluate(() => window.__frames.start());
  await wheelScroll(page, ms);
  const wheel = summarize(await page.evaluate(() => window.__frames.stop()));
  return { drag, wheel };
}

/** Saut en haut / en bas par scrollTop (Début / Fin), affichage complet après peinture. */
async function jumps(page) {
  const jump = (to) =>
    page.evaluate(
      (to) =>
        new Promise((resolve) => {
          const el = document.querySelector(".vlist");
          const t0 = performance.now();
          const c = window.__orch?.conversation?.();
          if (c) to === "top" ? c.scrollToTop() : c.scrollToBottom();
          else el.scrollTop = to === "top" ? 0 : el.scrollHeight;
          const check = () => {
            const vp = el.getBoundingClientRect();
            const first = el.querySelector('[data-index="0"]');
            const ok =
              to === "top"
                ? first && Math.abs(first.getBoundingClientRect().top - vp.top) <= 1
                : Math.abs(el.scrollHeight - el.clientHeight - el.scrollTop) <= 1 &&
                  el.querySelectorAll("[data-index]").length > 0;
            if (ok) {
              const ch = new MessageChannel();
              ch.port1.onmessage = () => resolve(performance.now() - t0);
              ch.port2.postMessage(0);
            } else requestAnimationFrame(check);
          };
          requestAnimationFrame(check);
        }),
      to,
    );
  const res = { top: [], bottom: [] };
  for (let i = 0; i < 10; i++) {
    res.top.push(+(await jump("top")).toFixed(1));
    res.bottom.push(+(await jump("bottom")).toFixed(1));
  }
  res.max = Math.max(...res.top, ...res.bottom);
  return res;
}

/** Ajout d'un message : Entrée (CDP) → ligne utilisateur peinte. */
async function addMessage(page, n = 5) {
  const res = [];
  for (let i = 0; i < n; i++) {
    const text = `message de mesure ${i} ${Date.now()}`;
    await page.locator('[aria-label="Message"]').fill(text);
    await page.evaluate((text) => {
      window.__t0 = 0;
      document.querySelector('[aria-label="Message"]').addEventListener(
        "keydown",
        (e) => {
          if (e.key === "Enter") window.__t0 = performance.now();
        },
        { once: true, capture: true },
      );
      window.__vis = new Promise((resolve) => {
        const found = () =>
          [...document.querySelectorAll('[data-row-kind="user"]')].some((r) => r.textContent?.includes(text));
        const mo = new MutationObserver(() => {
          if (!found()) return;
          mo.disconnect();
          const ch = new MessageChannel();
          ch.port1.onmessage = () => resolve(performance.now() - window.__t0);
          ch.port2.postMessage(0);
        });
        mo.observe(document.querySelector(".vlist"), { childList: true, subtree: true, characterData: true });
      });
    }, text);
    await page.keyboard.press("Enter");
    res.push(+(await page.evaluate(() => window.__vis)).toFixed(1));
    await page.waitForTimeout(300);
  }
  return res;
}

async function heap(page) {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("HeapProfiler.collectGarbage");
  const h = await cdp.send("Runtime.getHeapUsage");
  return { heapUsedMB: +(h.usedSize / 2 ** 20).toFixed(1) };
}

async function shot(page, name) {
  try {
    await page.screenshot({ path: join(WORK, `perf-${MODE}-${name}.png`), timeout: 15_000 });
    return "ok";
  } catch (e) {
    return `échec : ${e.message.split("\n")[0]}`;
  }
}

async function waitIdleStable(page, ms = 120_000) {
  // Plus de nouvelles lignes pendant 2 s, et plus de tour en cours.
  const end = Date.now() + ms;
  let last = -1;
  let since = Date.now();
  while (Date.now() < end) {
    const n = await page.evaluate(() => {
      const el = document.querySelector(".vlist");
      return el ? el.scrollHeight : 0;
    });
    if (n !== last) ((last = n), (since = Date.now()));
    if (Date.now() - since > 2000) return last;
    await sleep(250);
  }
  return last;
}

async function main() {
  const env = MODE === "mock" ? {} : { ORCH_CLAUDE_PATH: REPLAY, REPLAY_RATE: "100" };
  if (MODE === "latency") Object.assign(env, { REPLAY_SKIP_T15: "1", REPLAY_MS: "10000" });
  if (MODE === "t15") env.REPLAY_MS = "11000";
  const app = await launchApp({ args: MODE === "mock" ? [] : [SANDBOX], env });
  const { page, pid } = app;
  try {
    await page.addInitScript(installFrameRecorder);
    // Page déjà chargée (pas de navigation hors mode simulé) : aussi tout de suite.
    await page.evaluate(installFrameRecorder);
    out.visibility0 = await page.evaluate(() => document.visibilityState);
    await sleep(3000);
    out.memoryIdle = appMemory(pid);

    if (MODE === "mock") {
      const origin = new URL(page.url()).origin;
      out.url = `${origin}/?mock=long&rows=3000&rate=100`;
      await page.goto(out.url);
      await page.waitForFunction(() => window.__orch?.store.status === "Idle" && window.__orch.store.rows.length >= 3000, null, {
        timeout: 60_000,
      });
      out.session = await page.evaluate(() => ({ ...window.__orch.mock.stats, items: window.__orch.conversation().itemCount() }));
    } else {
      await page.waitForFunction(
        () => document.querySelector('[aria-label="Message"]')?.getAttribute("placeholder") !== "session non démarrée",
        null,
        { timeout: 30_000 },
      );
      await page.locator('[aria-label="Message"]').fill("rejoue");
      const t0 = Date.now();
      await page.keyboard.press("Enter");
      if (MODE === "t15") {
        out.scrollHeight = await waitIdleStable(page);
        out.t15LoadMs = Date.now() - t0 - 2000;
        out.session = await page.evaluate(() => ({
          domRows: document.querySelectorAll("[data-row-kind]").length,
          maxIndex: Math.max(...[...document.querySelectorAll("[data-index]")].map((r) => +r.dataset.index)),
        }));
      } else {
        await sleep(10_500);
        await sleep(1500);
        out.footer = await page.evaluate(() => [...document.querySelectorAll("span")].map((s) => s.textContent).find((t) => t?.includes("latence token")));
        // Deuxième tour : plus d'échantillons.
        await page.locator('[aria-label="Message"]').fill("encore");
        await page.keyboard.press("Enter");
        await sleep(12_000);
        out.footer2 = await page.evaluate(() => [...document.querySelectorAll("span")].map((s) => s.textContent).find((t) => t?.includes("latence token")));
        out.shot = await shot(page, "latence");
        return;
      }
    }
    out.visibility = await page.evaluate(() => document.visibilityState);
    await sleep(2000);
    out.memoryAfterLoad = appMemory(pid);
    out.heapAfterLoad = await heap(page);
    out.shotLoaded = await shot(page, "charge");

    log("scroll");
    out.scroll = await scrollBoth(page, 10_000);
    log(JSON.stringify(out.scroll));
    if (MODE === "mock") {
      await page.locator(".vlist").focus();
      await page.keyboard.press("Control+o");
      await page.locator(".detail-bar").waitFor({ timeout: 10_000 });
      await sleep(500);
      out.scrollDetail = await scrollBoth(page, 10_000);
      await page.locator(".vlist").focus();
      await page.keyboard.press("Control+o");
      await sleep(500);
    }
    log("sauts");
    out.jumps = await jumps(page);

    log("streaming et scroll");
    if (MODE === "mock") await page.evaluate(() => window.__orch.mock.streamReply(Infinity));
    else {
      await page.locator('[aria-label="Message"]').fill("flux pendant le défilement");
      await page.keyboard.press("Enter");
      await sleep(500);
    }
    await page.mouse.move(400, 300);
    await page.evaluate(() => window.__frames.start());
    await dragScroll(page, 5000, 3000);
    await wheelScroll(page, 5000);
    out.streamingScroll = summarize(await page.evaluate(() => window.__frames.stop()));
    if (MODE === "mock") await page.evaluate(() => window.__orch.mock.stopStream());
    else await sleep(2000);

    if (MODE === "mock") {
      log("latence token (simulé)");
      await page.evaluate(() => {
        window.__orch.conversation().scrollToBottom();
        window.__orch.mock.streamReply(Infinity);
        window.__orch.latencies.length = 0;
      });
      await sleep(5000);
      const lat = await page.evaluate(() => (window.__orch.mock.stopStream(), [...window.__orch.latencies]));
      const s = [...lat].sort((a, b) => a - b);
      const p = (q) => +s[Math.floor(q * (s.length - 1))].toFixed(1);
      out.tokenLatency = { samples: s.length, p50: p(0.5), p95: p(0.95), max: p(1) };
    }

    log("ajout de message");
    await page.evaluate(() => document.querySelector(".vlist").scrollTop = 1e9);
    if (MODE === "t15") await sleep(12_000); // fin du tour en streaming précédent
    out.addMessage = await addMessage(page, MODE === "t15" ? 3 : 5);
    if (MODE === "mock") await page.evaluate(() => window.__orch.mock.stopStream());
    await sleep(MODE === "t15" ? 14_000 : 1000);
    out.memoryEnd = appMemory(pid);
    out.heapEnd = await heap(page);
    out.shotEnd = await shot(page, "fin");
  } finally {
    const desc = tree(pid).map((p) => p.pid).filter((p) => p !== pid);
    await app.browser.close().catch(() => {});
    out.close = closeApp(pid);
    out.closedMs = await waitGone([pid, ...desc], 12_000).then((t) => (t ? "ok" : "restants"));
    if (out.closedMs !== "ok") for (const p of desc) closeApp(p, { force: true });
    log("écrit", save(`perf-${MODE}.json`, out));
  }
}

main().catch((e) => {
  console.error(e);
  out.error = String(e.stack ?? e);
  save(`perf-${MODE}.json`, out);
  process.exitCode = 1;
});
