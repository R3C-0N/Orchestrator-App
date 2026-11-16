/// <reference types="node" />
// Mesures du jalon J3 (plan 1.11) sur 3 000 lignes simulées :
// cadence du scroll, collage en bas, absence de saut en lecture, image max
// pendant le streaming, latence d'un token, saut en haut / en bas, ajout
// d'un message, mémoire. Chaque test imprime sa mesure (`MESURE …`).
import { execFileSync } from "node:child_process";
import { expect, test, type Page } from "@playwright/test";

const ROWS = 3000;
const RATE = 100;

type Orch = {
  store: { rows: unknown[]; live: unknown; status: string };
  mock: {
    streamReply(chars?: number): void;
    stopStream(): void;
    send(text: string, replyChars?: number): void;
    stats: { rows: number; textBytes: number; tools: number; agents: number } | null;
  };
  conversation: () => { scrollToTop(): void; scrollToBottom(): void; isSticky(): boolean; itemCount(): number };
  latencies: number[];
};
declare global {
  interface Window {
    __orch: Orch;
    __frames: { start(): void; stop(): Recording };
  }
}

/**
 * Enregistrement : durée entre deux `requestAnimationFrame`, plus le chemin
 * parcouru (somme des |Δ scrollTop|) et les lignes réellement affichées
 * (plus petit et plus grand `data-index` vus dans la vue).
 */
type Recording = { d: number[]; traveledPx: number; viewportPx: number; minIndex: number; maxIndex: number };

function installFrameRecorder() {
  let run = false;
  let rec: Recording = { d: [], traveledPx: 0, viewportPx: 0, minIndex: Infinity, maxIndex: -Infinity };
  window.__frames = {
    start() {
      rec = { d: [], traveledPx: 0, viewportPx: 0, minIndex: Infinity, maxIndex: -Infinity };
      run = true;
      let last = 0;
      let lastTop: number | null = null;
      const loop = (t: number) => {
        if (!run) return;
        if (last) rec.d.push(t - last);
        last = t;
        const el = document.querySelector<HTMLElement>(".vlist");
        if (el) {
          rec.viewportPx = el.clientHeight;
          if (lastTop !== null) rec.traveledPx += Math.abs(el.scrollTop - lastTop);
          lastTop = el.scrollTop;
          const vp = el.getBoundingClientRect();
          for (const r of el.querySelectorAll<HTMLElement>("[data-index]")) {
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
      run = false;
      return rec;
    },
  };
}

function summarize(rec: Recording) {
  const frames = rec.d;
  const total = frames.reduce((a, b) => a + b, 0);
  const sorted = [...frames].sort((a, b) => a - b);
  return {
    traveledPx: Math.round(rec.traveledPx),
    viewports: +(rec.traveledPx / rec.viewportPx).toFixed(1),
    rowsSpan: rec.maxIndex - rec.minIndex,
    frames: frames.length,
    seconds: +(total / 1000).toFixed(2),
    fps: +((frames.length * 1000) / total).toFixed(1),
    over33pct: +((100 * frames.filter((x) => x > 33).length) / frames.length).toFixed(2),
    over50: frames.filter((x) => x > 50).length,
    p99: +sorted[Math.floor(0.99 * (sorted.length - 1))].toFixed(1),
    max: +sorted[sorted.length - 1].toFixed(1),
  };
}

function report(name: string, data: unknown) {
  console.log(`MESURE ${name} ${JSON.stringify(data)}`);
  test.info().annotations.push({ type: name, description: JSON.stringify(data) });
}

const distance = (page: Page) =>
  page.evaluate(() => {
    const el = document.querySelector<HTMLElement>(".vlist")!;
    return el.scrollHeight - el.clientHeight - el.scrollTop;
  });

/** Défilement « glisser » : scrollTop modifié à chaque image, aller-retour. */
async function dragScroll(page: Page, ms: number, pxPerSec: number) {
  await page.evaluate(
    ({ ms, pxPerSec }) =>
      new Promise<void>((resolve) => {
        const el = document.querySelector<HTMLElement>(".vlist")!;
        // Comme un glisser de la barre : bouton tenu pendant tout le défilement.
        el.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
        let dir = -1;
        let last = performance.now();
        const end = last + ms;
        const step = (t: number) => {
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

/** Défilement à la molette : vrais événements `wheel` envoyés par le navigateur. */
async function wheelScroll(page: Page, ms: number) {
  const end = Date.now() + ms;
  let dir = -1;
  let flip = Date.now() + 2500;
  while (Date.now() < end) {
    await page.mouse.wheel(0, dir * 100);
    await page.waitForTimeout(8);
    if (Date.now() > flip) {
      dir = -dir;
      flip = Date.now() + 2500;
    }
  }
}

test.describe.configure({ mode: "serial" });

let page: Page;
/** Index de la dernière ligne affichée (après regroupement des appels, J5). */
let LAST = ROWS - 1;

test.beforeAll(async ({ browser }) => {
  page = await browser.newPage();
  await page.addInitScript(installFrameRecorder);
  await page.goto(`/?mock=long&rows=${ROWS}&rate=${RATE}`);
  // J5 : les agents d'arrière-plan ajoutent leur ligne de fin, d'où « ≥ ».
  await page.waitForFunction(
    (n) => window.__orch?.store.status === "Idle" && window.__orch.store.rows.length >= n,
    ROWS,
  );
  LAST = (await page.evaluate(() => window.__orch.conversation().itemCount())) - 1;
  await page.waitForSelector(`[data-index="${LAST}"]`);
  await page.mouse.move(500, 300);
  const stats = await page.evaluate(() => window.__orch.mock.stats);
  report("session", { ...stats, items: LAST + 1, domRows: await page.locator("[data-row-kind]").count() });
});

test.afterAll(async () => {
  await page.close();
});

test("démarre collé en bas, liste virtualisée", async () => {
  expect(await page.evaluate(() => window.__orch.conversation().isSticky())).toBe(true);
  expect(await distance(page)).toBeLessThanOrEqual(1);
  // Virtualisation : quelques dizaines de lignes dans le DOM, pas 3 000.
  expect(await page.locator("[data-row-kind]").count()).toBeLessThan(150);
});

test("scroll : ≥ 58 i/s et < 1 % d'images > 33 ms, 10 s au glisser puis 10 s à la molette", async () => {
  await page.evaluate(() => window.__frames.start());
  await dragScroll(page, 10_000, 3000);
  const drag = summarize(await page.evaluate(() => window.__frames.stop()));
  await page.evaluate(() => window.__frames.start());
  await wheelScroll(page, 10_000);
  const wheel = summarize(await page.evaluate(() => window.__frames.stop()));
  report("scroll-glisser", drag);
  report("scroll-molette", wheel);
  for (const m of [drag, wheel]) {
    // 10 s de défilement réel sur le contenu, pas une vue immobile.
    expect(m.seconds).toBeGreaterThanOrEqual(9.9);
    expect(m.viewports).toBeGreaterThanOrEqual(20);
    expect(m.rowsSpan).toBeGreaterThanOrEqual(30);
    expect(m.fps).toBeGreaterThanOrEqual(58);
    expect(m.over33pct).toBeLessThan(1);
  }
});

test("scroll en vue détaillée (Ctrl+O) : mêmes seuils", async () => {
  await page.locator(".vlist").focus();
  await page.keyboard.press("Control+o");
  await page.locator(".detail-bar").waitFor();
  await page.waitForTimeout(500);
  await page.mouse.move(500, 300);
  await page.evaluate(() => window.__frames.start());
  await dragScroll(page, 10_000, 3000);
  const drag = summarize(await page.evaluate(() => window.__frames.stop()));
  await page.evaluate(() => window.__frames.start());
  await wheelScroll(page, 10_000);
  const wheel = summarize(await page.evaluate(() => window.__frames.stop()));
  const items = await page.evaluate(() => window.__orch.conversation().itemCount());
  await page.locator(".vlist").focus();
  await page.keyboard.press("Control+o");
  await page.locator(".detail-bar").waitFor({ state: "detached" });
  await page.waitForTimeout(500);
  report("scroll-detail-glisser", { ...drag, items });
  report("scroll-detail-molette", wheel);
  for (const m of [drag, wheel]) {
    expect(m.seconds).toBeGreaterThanOrEqual(9.9);
    expect(m.viewports).toBeGreaterThanOrEqual(20);
    expect(m.fps).toBeGreaterThanOrEqual(58);
    expect(m.over33pct).toBeLessThan(1);
  }
});

test("saut en haut / en bas : affichage complet < 100 ms", async () => {
  const jump = (to: "top" | "bottom") =>
    page.evaluate(
      ({ to, last }) =>
        new Promise<number>((resolve) => {
          const el = document.querySelector<HTMLElement>(".vlist")!;
          const t0 = performance.now();
          const c = window.__orch.conversation();
          if (to === "top") c.scrollToTop();
          else c.scrollToBottom();
          const sel = `[data-index="${to === "top" ? 0 : last}"]`;
          const check = () => {
            const row = el.querySelector<HTMLElement>(sel);
            const vp = el.getBoundingClientRect();
            const r = row?.getBoundingClientRect();
            const ok =
              r &&
              (to === "top"
                ? Math.abs(r.top - vp.top) <= 1
                : Math.abs(el.scrollHeight - el.clientHeight - el.scrollTop) <= 1);
            if (ok) {
              // Mesure après la peinture de l'image.
              const ch = new MessageChannel();
              ch.port1.onmessage = () => resolve(performance.now() - t0);
              ch.port2.postMessage(0);
            } else requestAnimationFrame(check);
          };
          requestAnimationFrame(check);
        }),
      { to, last: LAST },
    );
  const res = { top: [] as number[], bottom: [] as number[] };
  for (let i = 0; i < 10; i++) {
    res.top.push(+(await jump("top")).toFixed(1));
    res.bottom.push(+(await jump("bottom")).toFixed(1));
  }
  report("saut", res);
  expect(Math.max(...res.top, ...res.bottom)).toBeLessThan(100);
});

test("streaming : reste collé en bas, puis ne saute pas pendant la lecture, puis reprend le collage", async () => {
  await page.evaluate(() => window.__orch.conversation().scrollToBottom());
  await page.evaluate(() => window.__orch.mock.streamReply(Infinity));
  // 1. Collé : distance au bas mesurée à chaque image pendant 3 s.
  const stuck = await page.evaluate(
    () =>
      new Promise<{ frames: number; maxPx: number; over1Line: number; finalPx: number }>((resolve) => {
        const el = document.querySelector<HTMLElement>(".vlist")!;
        const d: number[] = [];
        const end = performance.now() + 3000;
        const loop = () => {
          d.push(el.scrollHeight - el.clientHeight - el.scrollTop);
          if (performance.now() < end) requestAnimationFrame(loop);
          else
            resolve({
              frames: d.length,
              maxPx: Math.max(...d),
              over1Line: d.filter((x) => x > 18).length,
              finalPx: d[d.length - 1],
            });
        };
        requestAnimationFrame(loop);
      }),
  );
  report("collage-streaming", stuck);
  expect(stuck.over1Line).toBe(0);

  // 2. Lecture plus haut : la ligne repère ne bouge pas pendant que le flux
  //    grandit et que de nouvelles lignes s'ajoutent en bas.
  for (let i = 0; i < 6; i++) await page.mouse.wheel(0, -200);
  await page.waitForTimeout(400);
  expect(await page.evaluate(() => window.__orch.conversation().isSticky())).toBe(false);
  const anchor = () =>
    page.evaluate(() => {
      const el = document.querySelector<HTMLElement>(".vlist")!;
      const vp = el.getBoundingClientRect();
      const rows = [...el.querySelectorAll<HTMLElement>("[data-index]")];
      const mid = rows.find((r) => r.getBoundingClientRect().bottom > vp.top + vp.height / 2)!;
      return { index: mid.dataset.index, top: mid.getBoundingClientRect().top, scrollTop: el.scrollTop };
    });
  const a0 = await anchor();
  const rows0 = await page.evaluate(() => window.__orch.store.rows.length);
  await page.waitForTimeout(1500);
  await page.evaluate(() => window.__orch.mock.send("nouvelle question pendant la lecture", Infinity));
  await page.waitForTimeout(1500);
  const a1 = await page.evaluate(
    (index) => {
      const el = document.querySelector<HTMLElement>(".vlist")!;
      return el.querySelector<HTMLElement>(`[data-index="${index}"]`)!.getBoundingClientRect().top;
    },
    a0.index,
  );
  const rows1 = await page.evaluate(() => window.__orch.store.rows.length);
  const drift = { index: a0.index, driftPx: +(a1 - a0.top).toFixed(2), rowsAdded: rows1 - rows0 };
  report("lecture-sans-saut", drift);
  expect(rows1).toBeGreaterThan(rows0);
  expect(Math.abs(drift.driftPx)).toBeLessThanOrEqual(1);
  expect(await page.evaluate(() => window.__orch.conversation().isSticky())).toBe(false);

  // 3. Retour en bas à la molette : le collage reprend.
  for (let i = 0; i < 40 && (await distance(page)) > 2; i++) {
    await page.mouse.wheel(0, 2000);
    await page.waitForTimeout(50);
  }
  await page.waitForTimeout(300);
  const sticky = await page.evaluate(() => window.__orch.conversation().isSticky());
  await page.waitForTimeout(1000);
  const after = await distance(page);
  report("reprise-collage", { sticky, distancePx: after });
  expect(sticky).toBe(true);
  expect(after).toBeLessThanOrEqual(18);
});

test("streaming pendant le scroll : aucune image > 50 ms à 100 text_delta/s", async () => {
  await page.evaluate(() => window.__orch.mock.streamReply(Infinity));
  await page.evaluate(() => window.__frames.start());
  await dragScroll(page, 5000, 3000);
  await wheelScroll(page, 5000);
  const m = summarize(await page.evaluate(() => window.__frames.stop()));
  await page.evaluate(() => window.__orch.mock.stopStream());
  report("streaming-et-scroll", m);
  expect(m.viewports).toBeGreaterThanOrEqual(10);
  expect(m.over50).toBe(0);
});

test("latence d'un token : p50 < 25 ms, p95 < 50 ms (t de chaque delta → peinture)", async () => {
  await page.evaluate(() => {
    window.__orch.conversation().scrollToBottom();
    window.__orch.mock.streamReply(Infinity);
    window.__orch.latencies.length = 0;
  });
  await page.waitForTimeout(5000);
  const lat = await page.evaluate(() => {
    window.__orch.mock.stopStream();
    return [...window.__orch.latencies];
  });
  const s = [...lat].sort((a, b) => a - b);
  const p = (q: number) => +s[Math.floor(q * (s.length - 1))].toFixed(1);
  const m = { samples: s.length, p50: p(0.5), p95: p(0.95), max: p(1) };
  report("latence-token", m);
  expect(m.p50).toBeLessThan(25);
  expect(m.p95).toBeLessThan(50);
});

test("ajout d'un message utilisateur : visible < 50 ms après Entrée", async () => {
  const res: number[] = [];
  for (let i = 0; i < 5; i++) {
    const text = `message de mesure ${i}`;
    await page.locator('input[aria-label="Message"]').fill(text);
    // Écouteurs posés avant la frappe : `keydown` donne t0, un observateur de
    // mutations voit la ligne dans l'image même où elle est insérée, puis la
    // mesure est prise après la peinture de cette image.
    await page.evaluate((text) => {
      const w = window as unknown as { __t0: number; __vis: Promise<number> };
      w.__t0 = 0;
      document.querySelector('input[aria-label="Message"]')!.addEventListener(
        "keydown",
        (e) => {
          if ((e as KeyboardEvent).key === "Enter") w.__t0 = performance.now();
        },
        { once: true, capture: true },
      );
      w.__vis = new Promise<number>((resolve) => {
        const found = () =>
          [...document.querySelectorAll<HTMLElement>('[data-row-kind="user"]')].some((r) =>
            r.textContent?.includes(text),
          );
        const mo = new MutationObserver(() => {
          if (!found()) return;
          mo.disconnect();
          const ch = new MessageChannel();
          ch.port1.onmessage = () => resolve(performance.now() - w.__t0);
          ch.port2.postMessage(0);
        });
        mo.observe(document.querySelector(".vlist")!, { childList: true, subtree: true, characterData: true });
      });
    }, text);
    await page.keyboard.press("Enter");
    const done = page.evaluate(() => (window as unknown as { __vis: Promise<number> }).__vis);
    res.push(+(await done).toFixed(1));
    await page.waitForTimeout(300);
  }
  await page.evaluate(() => window.__orch.mock.stopStream());
  report("ajout-message", res);
  expect(Math.max(...res)).toBeLessThan(50);
});

test("mémoire : tas JS après GC, et ensemble de travail du navigateur", async () => {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("HeapProfiler.collectGarbage");
  const heap = (await cdp.send("Runtime.getHeapUsage")) as { usedSize: number; totalSize: number };
  const rows = await page.evaluate(() => window.__orch.store.rows.length);
  let processes: unknown = "non mesuré";
  if (process.platform === "win32") {
    // Processus Chromium de Playwright (révision du navigateur lancé), hors autres navigateurs.
    const ps = `$p = Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -like '*ms-playwright*chrom*' -and $_.Name -like '*chrom*' }; ` +
      `$ws = ($p | Measure-Object WorkingSetSize -Sum).Sum; $pr = ($p | Measure-Object PrivatePageCount -Sum).Sum; ` +
      `"{0};{1};{2}" -f $p.Count, $ws, $pr`;
    try {
      const out = execFileSync("powershell.exe", ["-NoProfile", "-Command", ps], { encoding: "utf8" }).trim();
      const [count, ws, priv] = out.split(";").map(Number);
      processes = { count, workingSetMB: +(ws / 2 ** 20).toFixed(0), privateBytesMB: +(priv / 2 ** 20).toFixed(0) };
    } catch (e) {
      processes = `échec : ${String(e).slice(0, 200)}`;
    }
  }
  const m = { rows, heapUsedMB: +(heap.usedSize / 2 ** 20).toFixed(1), heapTotalMB: +(heap.totalSize / 2 ** 20).toFixed(1), processes };
  report("memoire", m);
  // Le seuil du plan (< 450 Mo app + WebView2 après t15) se mesure sur l'app
  // compilée ; ici on borne le tas JS, part qui dépend de l'interface.
  expect(m.heapUsedMB).toBeLessThan(150);
});
