/// <reference types="node" />
// Collage en bas (J3) : cas limites relevés à la relecture.
// - B1 : contenu plus court que la vue, geste vers le haut → le collage reste.
// - Un message plus haut que la vue reste collé pendant son streaming.
// - Redimensionnement de la fenêtre pendant le streaming.
import { expect, test, type Page } from "@playwright/test";

type Orch = {
  store: { rows: unknown[] };
  mock: { opts: { rate: number }; streamReply(chars?: number): void; stopStream(): void };
  conversation: () => { isSticky(): boolean; scrollToBottom(): void };
};
const orch = (page: Page) => page.evaluate(() => (window as unknown as { __orch: Orch }).__orch !== undefined);

async function open(page: Page, rows: number) {
  await page.goto(`/?mock=long&rows=${rows}&rate=400`);
  await expect.poll(() => orch(page)).toBe(true);
  await page.waitForFunction(
    // J5 : les agents d'arrière-plan ajoutent leur ligne de fin, d'où « ≥ ».
    (n) => (window as unknown as { __orch: Orch }).__orch.store.rows.length >= n,
    rows,
  );
}

const sticky = (page: Page) =>
  page.evaluate(() => (window as unknown as { __orch: Orch }).__orch.conversation().isSticky());

/** Distance au bas relevée à chaque image pendant `ms`. */
const watchBottom = (page: Page, ms: number) =>
  page.evaluate(
    (ms) =>
      new Promise<{ frames: number; maxPx: number; over1Line: number; finalPx: number; liveHeight: number; viewport: number }>(
        (resolve) => {
          const el = document.querySelector<HTMLElement>(".vlist")!;
          const d: number[] = [];
          const end = performance.now() + ms;
          // Relevé après la peinture de chaque image (après le ResizeObserver
          // et les corrections de défilement de cette image) : ce qui est vu.
          const ch = new MessageChannel();
          let done = false;
          ch.port1.onmessage = () => {
            d.push(el.scrollHeight - el.clientHeight - el.scrollTop);
            if (!done) requestAnimationFrame(loop);
          };
          const loop = () => {
            if (performance.now() < end) return ch.port2.postMessage(0);
            done = true;
            const rows = el.querySelectorAll<HTMLElement>('[data-row-kind="assistant"]');
            const last = rows[rows.length - 1];
            resolve({
              frames: d.length,
              maxPx: Math.max(...d),
              over1Line: d.filter((x) => x > 18).length,
              finalPx: d[d.length - 1],
              liveHeight: last ? last.getBoundingClientRect().height : 0,
              viewport: el.clientHeight,
            });
          };
          requestAnimationFrame(loop);
        },
      ),
    ms,
  );

function report(name: string, data: unknown) {
  console.log(`MESURE ${name} ${JSON.stringify(data)}`);
}

test("B1 : contenu plus court que la vue, molette / ↑ / PageUp vers le haut → reste collé", async ({ page }) => {
  await open(page, 1);
  const fits = await page.evaluate(() => {
    const el = document.querySelector<HTMLElement>(".vlist")!;
    return el.scrollHeight <= el.clientHeight;
  });
  expect(fits).toBe(true);
  await page.mouse.move(400, 200);
  await page.mouse.wheel(0, -300);
  await page.waitForTimeout(300);
  expect(await sticky(page)).toBe(true);
  await page.locator(".vlist").focus();
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("PageUp");
  await page.waitForTimeout(300);
  expect(await sticky(page)).toBe(true);
  // Le contenu grandit ensuite au-delà de la vue : il suit le bas.
  await page.evaluate(() => (window as unknown as { __orch: Orch }).__orch.mock.streamReply(Infinity));
  const m = await watchBottom(page, 3000);
  await page.evaluate(() => (window as unknown as { __orch: Orch }).__orch.mock.stopStream());
  report("B1-collage-apres-geste", m);
  expect(m.liveHeight).toBeGreaterThan(m.viewport);
  expect(m.finalPx).toBeLessThanOrEqual(1);
});

test("un message plus haut que la vue reste collé en bas pendant son streaming", async ({ page }) => {
  await open(page, 1);
  await page.evaluate(() => (window as unknown as { __orch: Orch }).__orch.mock.streamReply(Infinity));
  const m = await watchBottom(page, 6000);
  await page.evaluate(() => (window as unknown as { __orch: Orch }).__orch.mock.stopStream());
  report("message-plus-haut-que-la-vue", m);
  expect(m.liveHeight).toBeGreaterThan(2 * m.viewport);
  expect(m.over1Line).toBe(0);
  expect(m.finalPx).toBeLessThanOrEqual(1);
  expect(await sticky(page)).toBe(true);
});

test("redimensionnement de la fenêtre pendant le streaming : reste collé", async ({ page }) => {
  await open(page, 3000);
  await page.evaluate(() => (window as unknown as { __orch: Orch }).__orch.mock.streamReply(Infinity));
  const sizes = [
    { width: 800, height: 500 },
    { width: 1300, height: 900 },
    { width: 600, height: 400 },
    { width: 1100, height: 760 },
  ];
  const watching = watchBottom(page, 5000);
  for (let i = 0; i < 8; i++) {
    await page.setViewportSize(sizes[i % sizes.length]);
    await page.waitForTimeout(500);
  }
  const m = await watching;
  await page.evaluate(() => (window as unknown as { __orch: Orch }).__orch.mock.stopStream());
  report("redimensionnement-streaming", { ...m, resizes: 8 });
  expect(await sticky(page)).toBe(true);
  expect(m.finalPx).toBeLessThanOrEqual(1);
  // virtua ne mesure les lignes montées pendant un rappel de ResizeObserver
  // qu'à l'image suivante : au plus une image peinte hors du bas par
  // redimensionnement, jamais un décrochage durable.
  expect(m.over1Line).toBeLessThanOrEqual(8);
});

test("Ctrl+O pendant un streaming, puis molette vers le haut : la vue ne revient pas sur la ligne j", async ({ page }) => {
  await open(page, 600);
  await page.mouse.move(500, 300);
  await page.evaluate(() => (window as unknown as { __orch: Orch }).__orch.conversation().scrollToBottom());
  await page.evaluate(() => (window as unknown as { __orch: Orch }).__orch.mock.streamReply(Infinity));
  await page.waitForTimeout(1000);
  // Lecture un peu au-dessus du bas : la ligne en cours, qui grandit, reste
  // rendue (donc mesurée à chaque ligne ajoutée) sous la ligne lue.
  await page.mouse.wheel(0, -150);
  await page.waitForTimeout(300);
  expect(await sticky(page)).toBe(false);
  // Ligne source lue en haut de la vue, avant la bascule.
  const rowAtTop = () =>
    page.evaluate(() => {
      const el = document.querySelector<HTMLElement>(".vlist")!;
      const top = el.getBoundingClientRect().top + 4;
      const r = [...el.querySelectorAll<HTMLElement>("[data-index]")].find((r) => {
        const b = r.getBoundingClientRect();
        return b.top <= top && b.bottom > top;
      })!;
      return r.dataset.firstRow!;
    });
  const read = await rowAtTop();
  await page.keyboard.press("Control+o");
  await expect(page.getByTestId("detail-bar")).toBeVisible();
  await page.waitForTimeout(500);
  // Bascule sans perdre la lecture : la même ligne source est en haut.
  expect(await rowAtTop()).toBe(read);
  // Ligne j : celle qui contient le haut de la vue après la bascule (virtua
  // ne range pas les lignes dans l'ordre du DOM).
  const j = await page.evaluate(() => {
    const el = document.querySelector<HTMLElement>(".vlist")!;
    const top = el.getBoundingClientRect().top + 4;
    const rows = [...el.querySelectorAll<HTMLElement>("[data-index]")];
    return rows.find((r) => {
      const b = r.getBoundingClientRect();
      return b.top <= top && b.bottom > top;
    })!.dataset.index!;
  });
  const topOf = (j: string) =>
    page.evaluate((j) => {
      const el = document.querySelector<HTMLElement>(".vlist")!;
      const it = el.querySelector<HTMLElement>(`[data-index="${j}"]`);
      return it ? it.getBoundingClientRect().top - el.getBoundingClientRect().top : Infinity;
    }, j);
  const before = await topOf(j);
  for (let i = 0; i < 3; i++) await page.mouse.wheel(0, -200);
  await page.waitForTimeout(1000);
  // La ligne j est descendue d'environ 600 px et y reste (le streaming
  // continue) ; l'ancien `scrollToIndex(j)` la ramenait en haut.
  const after = await topOf(j);
  const topOfJ = after - before;
  console.log(`MESURE ctrl-o-puis-molette ${JSON.stringify({ j, before, after })}`);
  expect(topOfJ).toBeGreaterThan(300);
  expect(await sticky(page)).toBe(false);
  await page.evaluate(() => (window as unknown as { __orch: Orch }).__orch.mock.stopStream());
});
