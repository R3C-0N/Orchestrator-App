/// <reference types="node" />
// J14, étape 4 : interface de l'auto-amélioration en mode simulé
// (`MockSelfUpdate`) : `/ameliorer` par la palette, bandeau à chaque étape,
// annulation, relance requise, échec, dépôt désactivé, retour arrière, hash de
// l'interface dans la ligne d'état, rattachement aux sessions vivantes et
// saisie conservée après un rechargement.
import { expect, test, type Page } from "@playwright/test";

const PROMPT = '[data-testid="prompt"] textarea[aria-label="Message"]';
const PALETTE = '[data-testid="palette"]';
const BANNER = '[data-testid="improve-banner"]';
const STEPS = ["preparing", "working", "checking", "testing", "building", "waiting_idle", "switching", "merged"];

type Call = { cmd: string; args: Record<string, unknown> };
type Row = { kind: string; text?: string };
type Store = { status: string; lastSeq: number; gaps: number; rows: Row[] };
type Orch = {
  store: Store;
  calls: Call[];
  selfUpdate: () => { calls: Call[]; readyCalls: (string | null)[] } | null;
  router: () => {
    roles: Map<string, string>;
    get(k: string): Store | undefined;
    tail(k: string): { kind: string; text?: string }[];
  } | null;
};

async function open(page: Page, query = "", stepMs = 120) {
  await page.goto(`/?mock=demo&rows=0&rate=400&step=${stepMs}${query}`);
  await page.waitForFunction(() => (window as unknown as { __orch?: Orch }).__orch?.store.status === "Idle");
  await expect(page.locator(PROMPT)).toBeFocused();
  // Relevé de chaque étape affichée par le bandeau.
  await page.evaluate(() => {
    const w = window as unknown as { __steps: string[] };
    w.__steps = [];
    const note = () => {
      const s = document.querySelector('[data-testid="improve-banner"]')?.getAttribute("data-step");
      if (s && w.__steps.at(-1) !== s) w.__steps.push(s);
    };
    new MutationObserver(note).observe(document.body, { subtree: true, childList: true, attributes: true });
  });
}

const suCalls = (page: Page) =>
  page.evaluate(() => (window as unknown as { __orch: Orch }).__orch.selfUpdate()!.calls.map((c) => c.cmd));
const steps = (page: Page) => page.evaluate(() => (window as unknown as { __steps: string[] }).__steps);

async function improve(page: Page, request: string) {
  await page.keyboard.type("/amel");
  await expect(page.locator(`${PALETTE} [data-name="ameliorer"]`)).toBeVisible();
  await page.keyboard.press("Tab");
  await expect(page.locator(PROMPT)).toHaveValue("/ameliorer ");
  await page.keyboard.type(request);
  await page.keyboard.press("Enter");
}

test("hash de l'interface dans la ligne d'état, ui_ready après le rattachement", async ({ page }) => {
  await open(page);
  await expect(page.getByTestId("ui-hash")).toHaveText(/ui [0-9a-z]{3,7} \(embarquée\)/);
  expect(await suCalls(page)).toEqual(["attach_sessions", "ui_ready"]);
  // Interface embarquée : `ui_ready(null)`.
  expect(await page.evaluate(() => (window as unknown as { __orch: Orch }).__orch.selfUpdate()!.readyCalls)).toEqual([
    null,
  ]);
  // `/revenir-version` grisé, avec la raison.
  await page.keyboard.type("/revenir");
  const entry = page.locator(`${PALETTE} [data-name="revenir-version"]`);
  await expect(entry).toHaveAttribute("aria-disabled", "true");
  await expect(entry).toContainText("aucune version précédente");
});

test("/ameliorer : bandeau à chaque étape jusqu'à la fusion, session dédiée à part, puis retour arrière", async ({
  page,
}) => {
  await open(page);
  const rowsBefore = await page.evaluate(() => (window as unknown as { __orch: Orch }).__orch.store.rows.length);
  await improve(page, "Bandeau plus discret");
  await expect(page.locator(BANNER)).toBeVisible();
  await expect(page.locator(BANNER)).toContainText("/ameliorer Bandeau plus discret");
  await expect(page.locator(BANNER)).toHaveAttribute("data-step", "merged", { timeout: 10_000 });
  expect(await steps(page)).toEqual(STEPS);
  await expect(page.getByTestId("improve-text")).toContainText("fusionnée");
  expect(await suCalls(page)).toContain("improve_start");
  // Rien n'est parti vers claude.exe, et la session dédiée n'est pas dans le fil.
  const calls = await page.evaluate(() => (window as unknown as { __orch: Orch }).__orch.calls.map((c) => c.cmd));
  expect(calls).not.toContain("send_message");
  const sep = await page.evaluate(() => {
    const o = (window as unknown as { __orch: Orch }).__orch;
    return {
      rows: o.store.rows.length,
      // Session dédiée : aucun magasin (jamais affichée), derniers événements seulement.
      store: o.router()!.get("improve-1") ?? null,
      improve: o.router()!.tail("improve-1").filter((e) => e.kind === "user_message").map((e) => e.text),
      role: o.router()!.roles.get("improve-1"),
    };
  });
  expect(sep.rows).toBe(rowsBefore);
  expect(sep.store).toBeNull();
  expect(sep.improve).toEqual(["Prompt d'amélioration (simulé)"]);
  expect(sep.role).toBe("improve");
  // Interface sur disque : le hash actif diffère de celui de cette page (rechargement attendu).
  await expect(page.getByTestId("ui-hash")).toContainText("(disque)");
  await expect(page.getByTestId("ui-mismatch")).toContainText("actif mock100");
  // `/ameliorer` est de nouveau possible ; retour arrière par le bandeau.
  await page.getByTestId("improve-rollback").click();
  await expect(page.locator(BANNER)).toHaveAttribute("data-step", "rolled_back");
  await expect(page.getByTestId("ui-hash")).toContainText("(embarquée)");
  await expect(page.getByTestId("ui-mismatch")).toHaveCount(0);
  expect(await suCalls(page)).toContain("ui_rollback");
  // Fermer le bandeau.
  await page.getByTestId("improve-close").click();
  await expect(page.locator(BANNER)).toHaveCount(0);
});

test("retour arrière par la palette (/revenir-version)", async ({ page }) => {
  await open(page);
  await improve(page, "x");
  await expect(page.locator(BANNER)).toHaveAttribute("data-step", "merged", { timeout: 10_000 });
  await page.locator(PROMPT).focus();
  await page.keyboard.type("/revenir-version");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("prompt-hint")).toContainText("Interface embarquée réactivée");
  await expect(page.locator(BANNER)).toHaveAttribute("data-step", "rolled_back");
  expect((await suCalls(page)).filter((c) => c === "ui_rollback")).toHaveLength(1);
});

test("annulation pendant le cycle", async ({ page }) => {
  await open(page, "", 600);
  await improve(page, "Annuler moi");
  await expect(page.locator(BANNER)).toHaveAttribute("data-step", "working");
  await page.getByTestId("improve-cancel").click();
  await expect(page.locator(BANNER)).toHaveAttribute("data-step", "cancelled");
  await expect(page.getByTestId("improve-text")).toContainText("annulée");
  await page.waitForTimeout(1200);
  await expect(page.locator(BANNER)).toHaveAttribute("data-step", "cancelled");
  expect(await suCalls(page)).toContain("improve_cancel");
});

test("relance requise : message clair et ligne d'état", async ({ page }) => {
  await open(page, "&improve=restart");
  await improve(page, "Changer le cœur");
  await expect(page.locator(BANNER)).toHaveAttribute("data-step", "restart_required", { timeout: 10_000 });
  expect(await steps(page)).toEqual(["preparing", "working", "checking", "testing", "restart_required"]);
  const text = page.getByTestId("improve-text");
  await expect(text).toContainText("Relance requise");
  await expect(text).toContainText("npm run tauri build");
  await expect(text).toContainText("C:\\mock\\repo");
  await expect(page.getByTestId("ui-restart")).toContainText("relance requise (mock100)");
  // Pas de bascule : l'interface reste l'embarquée.
  await expect(page.getByTestId("ui-hash")).toContainText("(embarquée)");
});

test("envoi : rien de nouveau sans le réglage du propriétaire", async ({ page }) => {
  await open(page);
  await improve(page, "Sans envoi");
  await expect(page.locator(BANNER)).toHaveAttribute("data-step", "merged", { timeout: 10_000 });
  await page.waitForTimeout(400);
  await expect(page.getByTestId("improve-push")).toHaveCount(0);
  await expect(page.locator(BANNER)).not.toContainText("GitHub");
});

test("envoi du propriétaire : « poussé sur GitHub : auto/… » dans le bandeau final", async ({ page }) => {
  await open(page, "&push=pushed");
  await improve(page, "Avec envoi");
  await expect(page.locator(BANNER)).toHaveAttribute("data-step", "merged", { timeout: 10_000 });
  const push = page.getByTestId("improve-push");
  await expect(push).toHaveAttribute("data-push", "pushed", { timeout: 5_000 });
  await expect(push).toHaveText(/^⎿\s+poussé sur GitHub : auto\/\d{8}-\d{4}-avec-envoi$/);
  await expect(page.getByTestId("improve-text")).toContainText("fusionnée");
});

test("envoi du propriétaire en échec : fusion locale gardée, raison affichée", async ({ page }) => {
  await open(page, "&push=failed&improve=restart");
  await improve(page, "Envoi en panne");
  await expect(page.locator(BANNER)).toHaveAttribute("data-step", "restart_required", { timeout: 10_000 });
  const push = page.getByTestId("improve-push");
  await expect(push).toHaveAttribute("data-push", "failed", { timeout: 5_000 });
  await expect(push).toContainText("fusionné localement, envoi échoué : ssh: connect to host github.com");
  await expect(page.getByTestId("improve-text")).toContainText("Relance requise");
});

test("échec : détail affiché", async ({ page }) => {
  await open(page, "&improve=failed");
  await improve(page, "Casser");
  await expect(page.locator(BANNER)).toHaveAttribute("data-step", "failed", { timeout: 10_000 });
  await expect(page.getByTestId("improve-text")).toContainText("2 tests en échec");
  await expect(page.getByTestId("improve-text")).toContainText("non fusionnée");
});

test("dépôt désactivé : /ameliorer grisé avec la raison, rien n'est demandé", async ({ page }) => {
  await open(page, "&repo=disabled");
  await page.keyboard.type("/amel");
  const entry = page.locator(`${PALETTE} [data-name="ameliorer"]`);
  await expect(entry).toHaveAttribute("aria-disabled", "true");
  await expect(entry).toContainText("git introuvable");
  // Entrée sur l'entrée grisée : la raison, rien d'inséré.
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("prompt-hint")).toContainText("indisponible : git introuvable");
  // Tapée en entier : même refus.
  await page.keyboard.press("Control+a");
  await page.keyboard.type("/ameliorer quelque chose");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("prompt-hint")).toContainText("indisponible");
  expect(await suCalls(page)).not.toContain("improve_start");
  await expect(page.locator(BANNER)).toHaveCount(0);
  // Le reste de l'app fonctionne.
  await page.keyboard.press("Control+a");
  await page.keyboard.type("bonjour");
  await page.keyboard.press("Enter");
  await expect(page.locator('[data-row-kind="user"]').last()).toContainText("bonjour");
});

test("rattachement : sessions vivantes rejouées, aucune session relancée, la suite continue", async ({ page }) => {
  await page.goto("/?mock=demo&rows=0&rate=400&attach=40");
  await page.waitForFunction(() => (window as unknown as { __orch?: Orch }).__orch?.store.status === "Idle");
  const st = await page.evaluate(() => {
    const o = (window as unknown as { __orch: Orch }).__orch;
    return { rows: o.store.rows.length, lastSeq: o.store.lastSeq, gaps: o.store.gaps, improve: o.router()!.roles.get("improve-0") };
  });
  expect(st.rows).toBeGreaterThanOrEqual(40);
  expect(st.gaps).toBe(0);
  expect(st.improve).toBe("improve");
  expect(await suCalls(page)).toEqual(["attach_sessions", "ui_ready"]);
  // La conversation continue sur la même session, sans trou de `seq`.
  await page.locator(PROMPT).focus();
  await page.keyboard.type("suite");
  await page.keyboard.press("Enter");
  await page.waitForFunction(
    (n) => {
      const s = (window as unknown as { __orch: Orch }).__orch.store;
      return s.lastSeq > n && s.status === "Idle";
    },
    st.lastSeq,
    { timeout: 20_000 },
  );
  expect(await page.evaluate(() => (window as unknown as { __orch: Orch }).__orch.store.gaps)).toBe(0);
});

test("rechargement : saisie, historique ↑↓ et Ctrl+O conservés", async ({ page }) => {
  await open(page);
  await page.keyboard.type("premier envoi");
  await page.keyboard.press("Enter");
  await page.waitForFunction(() => (window as unknown as { __orch: Orch }).__orch.store.status === "Idle");
  await page.locator(PROMPT).focus();
  await page.keyboard.type("brouillon en cours");
  await page.keyboard.press("Control+o");
  await expect(page.locator(".detail-bar")).toBeVisible();
  await page.reload();
  await page.waitForFunction(() => (window as unknown as { __orch?: Orch }).__orch?.store.status === "Idle");
  // Vue détaillée rétablie, puis retour à la saisie : le brouillon est là.
  await expect(page.locator(".detail-bar")).toBeVisible();
  await page.keyboard.press("Control+o");
  await expect(page.locator(PROMPT)).toHaveValue("brouillon en cours");
  // Historique ↑ : saisie vidée puis rappel de l'envoi d'avant le rechargement.
  await page.locator(PROMPT).focus();
  await page.keyboard.press("Control+a");
  await page.keyboard.press("Delete");
  await page.keyboard.press("ArrowUp");
  await expect(page.locator(PROMPT)).toHaveValue("premier envoi");
});

test("stockage de session illisible : démarrage normal", async ({ page }) => {
  await page.goto("/?mock=demo&rows=0");
  await page.evaluate(() => sessionStorage.setItem("orchestrator.ui.v1", "{pas du json"));
  await page.reload();
  await page.waitForFunction(() => (window as unknown as { __orch?: Orch }).__orch?.store.status === "Idle");
  await expect(page.locator(PROMPT)).toHaveValue("");
});

test("rattachement refusé : 3 essais, aucune session lancée, pas de ui_ready, puis « Réessayer »", async ({ page }) => {
  await page.goto("/?mock=demo&rows=0&rate=400&attachFail=3");
  await expect(page.getByTestId("mount-error")).toContainText("Rattachement aux sessions impossible");
  await expect(page.getByTestId("mount-error")).toContainText("Aucune session n'a été lancée");
  type Su = { calls: Call[] };
  const suCmds = () =>
    page.evaluate(() => (window as unknown as { __orchSelfUpdate: Su }).__orchSelfUpdate.calls.map((c) => c.cmd));
  expect(await suCmds()).toEqual(["attach_sessions", "attach_sessions", "attach_sessions"]);
  // Aucune session simulée démarrée (`__orch` n'existe qu'une fois la session lancée).
  await page.waitForTimeout(500);
  expect(await page.evaluate(() => "__orch" in window)).toBe(false);
  await page.getByTestId("mount-retry").click();
  await page.waitForFunction(() => (window as unknown as { __orch?: Orch }).__orch?.store.status === "Idle");
  await expect(page.getByTestId("mount-error")).toHaveCount(0);
  expect(await suCmds()).toEqual(["attach_sessions", "attach_sessions", "attach_sessions", "attach_sessions", "ui_ready"]);
});

test("demande sur plusieurs lignes : en-tête sur une ligne ; /revenir-version grisé pendant le cycle", async ({ page }) => {
  await open(page, "", 600);
  await page.keyboard.type("/amel");
  await page.keyboard.press("Tab");
  await page.keyboard.type("Première ligne");
  await page.keyboard.press("Shift+Enter");
  await page.keyboard.type("seconde ligne");
  await expect(page.locator(PROMPT)).toHaveValue("/ameliorer Première ligne\nseconde ligne");
  await page.keyboard.press("Enter");
  await expect(page.locator(BANNER)).toContainText("/ameliorer Première ligne seconde ligne");
  await expect(page.locator(BANNER)).toHaveAttribute("data-step", "working");
  const head = page.locator(`${BANNER} .head`);
  const lineHeight = await page.locator(`${BANNER} .msg`).first().evaluate((e) => e.getBoundingClientRect().height);
  expect((await head.boundingBox())!.height).toBeLessThan(lineHeight * 1.5);
  await page.locator(PROMPT).focus();
  await page.keyboard.type("/revenir");
  await expect(page.locator(`${PALETTE} [data-name="revenir-version"]`)).toContainText("une amélioration est en cours");
});
