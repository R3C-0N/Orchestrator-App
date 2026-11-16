/// <reference types="node" />
// J11 : sélecteurs de projets et de sessions, et enchaînement de la reprise,
// en mode simulé (IPC Tauri simulé par `mockIPC`, sans cœur Rust). Les
// composants sont montés seuls par `sessions.harness.ts` : leur branchement
// dans `App.svelte` est hors de ce jalon.
import { expect, test, type Page } from "@playwright/test";
import type { Harness } from "./sessions.harness";
import type { HistoryLoaded } from "../src/lib/core/bindings/HistoryLoaded";

const HARNESS = "/__sessions-harness.html";

async function open(page: Page) {
  await page.route(`**${HARNESS}`, (route) =>
    route.fulfill({
      contentType: "text/html",
      body: `<!doctype html><html><head><meta charset="utf-8"></head>
        <body><div id="app" style="height:100vh;display:flex;flex-direction:column"></div>
        <script type="module" src="/e2e/sessions.harness.ts"></script></body></html>`,
    }),
  );
  await page.goto(HARNESS);
  await page.waitForFunction(() => "__sessions" in window);
}


const PROJECTS = [
  { encoded: "C--Users--user--Code-demo", path: "C:\\Users\\<user>\\Code\\demo", exists: true, last_used: Date.now() - 60_000, session_count: 4 },
  { encoded: "D--disparu-a-b", path: "D:\\disparu\\a-b", exists: false, last_used: Date.now() - 3_600_000, session_count: 1 },
  { encoded: "C--Users--user--Code-autre", path: "C:\\Users\\<user>\\Code\\autre", exists: true, last_used: Date.now() - 86_400_000 * 3, session_count: 1 },
];

function sessions(n: number) {
  return Array.from({ length: n }, (_, i) => ({
    id: `00000000-0000-4000-8000-${String(i).padStart(12, "0")}`,
    title: i === 1 ? null : `Titre ${i}`,
    last_prompt: `dernier prompt ${i}`,
    modified: Date.now() - i * 60_000,
    size_bytes: 1024 * (i + 1),
    entrypoint: i === 1 ? "sdk-cli" : "cli",
  }));
}

test.describe("sélecteur de projets", () => {
  test("liste les vrais chemins, grise les dossiers disparus, Entrée ouvre", async ({ page }) => {
    await open(page);
    await page.evaluate((p) => (window as unknown as { __sessions: Harness }).__sessions.mountProjects(p), PROJECTS);
    const rows = page.getByTestId("project-row");
    await expect(rows).toHaveCount(3);
    await expect(rows.nth(0)).toContainText("C:\\Users\\<user>\\Code\\demo");
    await expect(rows.nth(0)).toContainText("4 sessions");
    await expect(rows.nth(1)).toHaveAttribute("aria-disabled", "true");
    await expect(rows.nth(1)).toContainText("introuvable");
    await expect(rows.nth(0)).toHaveAttribute("aria-selected", "true");

    // ↓ saute le dossier disparu.
    await page.keyboard.press("ArrowDown");
    await expect(rows.nth(2)).toHaveAttribute("aria-selected", "true");
    await expect(rows.nth(1)).toHaveAttribute("aria-selected", "false");
    await page.keyboard.press("Enter");
    const picked = await page.evaluate(() => (window as unknown as { __sessions: Harness }).__sessions.picked);
    expect(picked).toEqual(["C:\\Users\\<user>\\Code\\autre"]);

    // Clic sur un dossier disparu : rien.
    await rows.nth(1).click({ force: true });
    expect(await page.evaluate(() => (window as unknown as { __sessions: Harness }).__sessions.picked.length)).toBe(1);

    // Filtre, puis Échap annule.
    await page.getByLabel("Filtrer les projets").fill("autre");
    await expect(rows).toHaveCount(1);
    await page.keyboard.press("Escape");
    expect(await page.evaluate(() => (window as unknown as { __sessions: Harness }).__sessions.cancelled)).toBe(1);
  });

  test("erreur de lecture affichée", async ({ page }) => {
    await open(page);
    await page.evaluate(() =>
      (window as unknown as { __sessions: Harness }).__sessions.mountProjects({
        error: { kind: "Io", message: "dossier personnel introuvable" },
      }),
    );
    await expect(page.getByTestId("picker-error")).toContainText("Io : dossier personnel introuvable");
  });
});

test.describe("sélecteur de sessions", () => {
  test("200 sessions affichées en moins de 300 ms, nouvelle session en tête", async ({ page }) => {
    await open(page);
    const list = sessions(200);
    const t0 = await page.evaluate(
      (l) => (window as unknown as { __sessions: Harness }).__sessions.mountSessions("C:\\demo", l),
      list,
    );
    await page.waitForFunction(() => document.querySelectorAll('[data-testid="session-row"]').length === 200);
    const ms = await page.evaluate((t) => performance.now() - t, t0);
    console.log(`SessionPicker : 200 sessions affichées en ${ms.toFixed(0)} ms`);
    expect(ms).toBeLessThan(300);

    const calls = await page.evaluate(() => (window as unknown as { __sessions: Harness }).__sessions.calls);
    expect(calls).toEqual([{ cmd: "list_sessions", args: { cwd: "C:\\demo" } }]);

    await expect(page.getByTestId("session-new")).toHaveAttribute("aria-selected", "true");
    const rows = page.getByTestId("session-row");
    await expect(rows.nth(0)).toContainText("Titre 0");
    // Session `-p` sans titre : le dernier prompt sert de libellé.
    await expect(rows.nth(1)).toContainText("dernier prompt 1");
    await expect(rows.nth(1)).toContainText("sdk-cli");
  });

  test("clavier : ↓ puis Entrée reprend la session, Entrée en tête démarre une nouvelle session", async ({ page }) => {
    await open(page);
    await page.evaluate(
      (l) => (window as unknown as { __sessions: Harness }).__sessions.mountSessions("C:\\demo", l),
      sessions(5),
    );
    await expect(page.getByTestId("session-row")).toHaveCount(5);
    await page.keyboard.press("Enter");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    await expect(page.getByTestId("session-row").nth(1)).toHaveAttribute("aria-selected", "true");
    await page.keyboard.press("Enter");
    const picked = await page.evaluate(() => (window as unknown as { __sessions: Harness }).__sessions.picked);
    expect(picked).toEqual(["new", "00000000-0000-4000-8000-000000000001"]);

    // Filtre : la ligne « nouvelle session » disparaît, la première trouvée est choisie.
    await page.getByLabel("Filtrer les sessions").fill("Titre 3");
    await expect(page.getByTestId("session-new")).toHaveCount(0);
    await expect(page.getByTestId("session-row")).toHaveCount(1);
    await page.keyboard.press("Enter");
    expect(await page.evaluate(() => (window as unknown as { __sessions: Harness }).__sessions.picked.at(-1))).toBe(
      "00000000-0000-4000-8000-000000000003",
    );
  });

  test("chargement lent : message d'attente, puis la liste", async ({ page }) => {
    await open(page);
    await page.evaluate(
      (l) => (window as unknown as { __sessions: Harness }).__sessions.mountSessions("C:\\demo", l, { delayMs: 300 }),
      sessions(3),
    );
    await expect(page.getByText("Lecture des transcripts…")).toBeVisible();
    await expect(page.getByTestId("session-row")).toHaveCount(3);
  });
});

test.describe("reprise", () => {
  const history: HistoryLoaded = {
    session_id: "0b8f3c2e-1111-4a6b-9c1d-000000000001",
    events: [
      { seq: 0, t: 10, session: "k-1", kind: "user_message", thread: "main", uuid: "u1", text: "Bonjour", origin: "History" },
      { seq: 1, t: 11, session: "k-1", kind: "block_done", thread: "main", msg: "msg_1", index: 0, block: { type: "Text", text: "Salut" } },
      { seq: 2, t: 12, session: "k-1", kind: "turn_result", subtype: "success", is_error: false, terminal_reason: null, duration_ms: 3000, cost_usd: 0, num_turns: 0, context_used: null, context_window: null, permission_denials: [], result: null },
    ],
    lines: 3, used: 3, ignored: 0, unknown: 0, unknown_elements: 0, bad: 0, sidechain: 0, truncated: false, subagent_threads: 0,
  };

  test("stop, start --resume, load_history : historique affiché sans trou de seq", async ({ page }) => {
    await open(page);
    // Passé en JSON : le type récursif `JsonValue` est trop profond pour `evaluate`.
    const r = await page.evaluate(
      (h) => (window as unknown as { __sessions: Harness }).__sessions.resume(JSON.parse(h)),
      JSON.stringify(history),
    );
    expect(r.calls).toEqual([
      "stop_session",
      "start_session:0b8f3c2e-1111-4a6b-9c1d-000000000001",
      "load_history:0b8f3c2e-1111-4a6b-9c1d-000000000001",
    ]);
    expect(r.rows.map((x) => x.kind)).toEqual(["user", "assistant", "turn"]);
    expect(r.rows[0]).toMatchObject({ text: "Bonjour", origin: "History" });
    expect(r.gaps).toBe(0);
    expect(r.lastSeq).toBe(1);
    expect(r.banners).toEqual([]);
  });

  test("historique illisible : reprise quand même, avec un avertissement", async ({ page }) => {
    await open(page);
    const r = await page.evaluate(
      () =>
        (window as unknown as { __sessions: Harness }).__sessions.resume({
          error: { kind: "TranscriptFormat", message: "aucune ligne JSON lisible" },
        }),
    );
    expect(r.calls).toHaveLength(3);
    expect(r.rows).toEqual([]);
    expect(r.banners).toEqual([{ code: "history_unavailable" }]);
  });
});
