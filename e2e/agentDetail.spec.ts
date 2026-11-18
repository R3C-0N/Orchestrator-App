// Maquette « Orchestrator » J15, seconde version (2026-11-18), dans l'app en
// mode simulé :
// - clic sur un agent de la colonne : sa conversation remplace la liste
//   (en-tête, état, fil, encadré de la question en attente), le bouton
//   « Agents » ramène la liste ;
// - le bouton Clair/Sombre est posé sur le segment p95 de la barre d'état,
//   sur un fond sombre translucide.
import { expect, test, type Page } from "@playwright/test";

type W = { __orch: { store: { status: string; lastSeq: number; apply(ev: unknown[]): void }; mock: { ask(q: unknown[], o?: { thread?: string }): string } } };

const AGENT = "toolu_detail_vert";

async function open(page: Page) {
  await page.setViewportSize({ width: 1136, height: 813 });
  await page.goto("/?mock=demo&rows=0&rate=400");
  await page.waitForFunction(() => (window as unknown as W).__orch?.store.status === "Idle");
  await page.evaluate(() => document.fonts.ready);
}

async function addAgent(page: Page) {
  await page.evaluate((id) => {
    const s = (window as unknown as W).__orch.store;
    const now = Date.now();
    s.apply([
      {
        seq: s.lastSeq + 1,
        t: now,
        session: "mock-1",
        kind: "agent",
        tool_use_id: id,
        task_id: "task_detail_vert",
        agent_type: "agent-vert",
        color: "green",
        description: "Agent dont on ouvre la conversation",
        status: "Running",
        background: true,
        started_at: now,
        ended_at: null,
        last_tool: "Read",
        tokens: 1234,
        tool_uses: null,
        summary: null,
      },
    ]);
  }, AGENT);
}

test("clic sur un agent : sa conversation remplace la liste, « Agents » y revient", async ({ page }) => {
  await open(page);
  await addAgent(page);
  const col = page.getByTestId("agent-column");
  const head = page.locator(`[data-agent="${AGENT}"] button.head`);
  await expect(head).toBeVisible();
  await page.evaluate(
    (id) =>
      (window as unknown as W).__orch.mock.ask(
        [{ question: "Quelle couleur ?", header: "Couleur", multi_select: false, options: [{ label: "Vert", description: "" }] }],
        { thread: id },
      ),
    AGENT,
  );
  await expect(page.locator(`[data-agent="${AGENT}"]`)).toHaveAttribute("data-waiting", "true");

  await head.click();
  const view = col.getByTestId("agent-detail-view");
  await expect(view).toHaveAttribute("data-agent-detail", AGENT);
  await expect(col.getByTestId("agents-waiting-toggle")).toHaveCount(0);
  await expect(view).toContainText("Agent dont on ouvre la conversation");
  await expect(col.getByTestId("agent-detail-state")).toHaveText("En attente d'action");
  await expect(col.getByTestId("agent-detail-question")).toContainText("Quelle couleur ?");
  await expect(col.getByTestId("agent-count")).toBeVisible();
  const back = col.getByTestId("agent-detail-back");
  await expect(back).toBeFocused();

  await back.click();
  await expect(view).toHaveCount(0);
  await expect(col.getByTestId("agents-waiting-toggle")).toBeVisible();
  await expect(head).toBeFocused();

  // Au clavier : Entrée ouvre, Entrée sur « Agents » referme.
  await page.keyboard.press("Enter");
  await expect(col.getByTestId("agent-detail-view")).toBeVisible();
  await expect(back).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(col.getByTestId("agent-detail-view")).toHaveCount(0);
});

test("bouton de thème sur le segment p95, fond sombre translucide", async ({ page }) => {
  await open(page);
  const toggle = page.getByTestId("theme-toggle");
  const seg = page.locator("footer .seg.last");
  await expect(seg).toContainText("p95");
  await expect(seg.getByTestId("theme-toggle")).toBeVisible();
  expect(await toggle.evaluate((el) => getComputedStyle(el).backgroundColor)).toBe("rgba(0, 0, 0, 0.14)");
  // Le bouton bascule toujours le thème.
  await toggle.click();
  expect(await page.evaluate(() => document.documentElement.dataset.theme)).toBe("light");
});
