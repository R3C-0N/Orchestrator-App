/// <reference types="node" />
// J16 : onglet d'amélioration en mode simulé (`MockSelfUpdate`). `/ameliorer`
// ouvre un onglet actif qui montre la conversation de la session dédiée
// (lignes d'état `improve.*` dans le fil, badge d'étape dans le titre),
// « Valider » seulement en attente, message à la session (en file pendant un
// tour, abandon des tests, compteur remis à 0), Échap, fermeture avec
// confirmation dans la page (jamais `confirm()`), fermeture refusée pendant la
// bascule, rechargement en attente, second `/ameliorer`. Restent de J14 : hash
// de l'interface, retour arrière, relance requise, envoi, dépôt désactivé,
// rattachement et rechargement.
import { expect, test, type Page } from "@playwright/test";

const PROMPT = '[data-testid="prompt"] textarea[aria-label="Message"]';
const PALETTE = '[data-testid="palette"]';
const BANNER = '[data-testid="improve-banner"]';
const TAB = '[data-testid="tab"]';
const ITAB = '[data-testid="tab"][data-kind="improve"]';
const STEPS = ["preparing", "working", "checking", "testing", "building", "waiting_idle", "switching", "merged"];

type Call = { cmd: string; args: Record<string, unknown> };
type Row = { kind: string; text?: string };
type Store = { status: string; lastSeq: number; gaps: number; rows: Row[] };
type Status = { id: string; step: string; attempt: number; turn: number; waiting: string | null; session: string | null };
type Orch = {
  store: Store;
  calls: Call[];
  selfUpdate: () => {
    calls: Call[];
    readyCalls: (string | null)[];
    guardRuns: number;
    info: { improve: Status | null };
  } | null;
  router: () => {
    roles: Map<string, string>;
    get(k: string): Store | undefined;
    tail(k: string): { kind: string; text?: string }[];
  } | null;
};

/**
 * Ouvre l'app simulée et relève, à chaque mutation, l'étape de l'onglet
 * d'amélioration (`data-step`), son badge, l'heure où il devient actif et
 * l'heure de la dernière frappe d'Entrée ; compte les `dialog` du navigateur.
 */
async function open(page: Page, query = "", stepMs = 120) {
  const dialogs: string[] = [];
  page.on("dialog", (d) => {
    dialogs.push(d.type());
    void d.dismiss();
  });
  await page.goto(`/?mock=demo&rows=0&rate=400&step=${stepMs}${query}`);
  await page.waitForFunction(() => (window as unknown as { __orch?: Orch }).__orch?.store.status === "Idle");
  await expect(page.locator(PROMPT)).toBeFocused();
  await watch(page);
  return dialogs;
}

async function watch(page: Page) {
  await page.evaluate(() => {
    type W = { __steps: string[]; __badges: string[]; __activeAt: number | null; __enterAt: number | null };
    const w = window as unknown as W;
    w.__steps = [];
    w.__badges = [];
    w.__activeAt = null;
    w.__enterAt = null;
    window.addEventListener(
      "keydown",
      (e) => {
        if (e.key === "Enter") w.__enterAt = performance.now();
      },
      true,
    );
    const note = () => {
      const t = document.querySelector('[data-testid="tab"][data-kind="improve"]');
      const s = t?.getAttribute("data-step");
      if (s && w.__steps.at(-1) !== s) w.__steps.push(s);
      const b = t?.querySelector('[data-testid="tab-badge"]')?.textContent?.trim();
      if (b && w.__badges.at(-1) !== b) w.__badges.push(b);
      if (w.__activeAt === null && t?.querySelector('[role="tab"]')?.getAttribute("aria-selected") === "true")
        w.__activeAt = performance.now();
    };
    new MutationObserver(note).observe(document.body, {
      subtree: true,
      childList: true,
      attributes: true,
      characterData: true,
    });
  });
}

const suCalls = (page: Page) =>
  page.evaluate(() => (window as unknown as { __orch: Orch }).__orch.selfUpdate()!.calls.map((c) => c.cmd));
const steps = (page: Page) => page.evaluate(() => (window as unknown as { __steps: string[] }).__steps);
const badges = (page: Page) => page.evaluate(() => (window as unknown as { __badges: string[] }).__badges);
const status = (page: Page) =>
  page.evaluate(() => (window as unknown as { __orch: Orch }).__orch.selfUpdate()!.info.improve);
/** Lignes du fil de l'onglet d'amélioration (magasin de la session du cycle). */
const improveRows = (page: Page) =>
  page.evaluate(() => {
    const o = (window as unknown as { __orch: Orch }).__orch;
    const s = o.selfUpdate()!.info.improve?.session;
    return s ? (o.router()!.get(s)?.rows ?? []).map((r) => ({ kind: r.kind, text: r.text ?? "" })) : [];
  });

async function improve(page: Page, request: string) {
  await page.locator(PROMPT).focus();
  await page.keyboard.type("/amel");
  await expect(page.locator(`${PALETTE} [data-name="ameliorer"]`)).toBeVisible();
  await page.keyboard.press("Tab");
  await expect(page.locator(PROMPT)).toHaveValue("/ameliorer ");
  await page.keyboard.type(request);
  await page.keyboard.press("Enter");
}

/** Message envoyé par la saisie de l'onglet actif (rempli d'un coup : un tour simulé est court). */
async function say(page: Page, text: string) {
  await page.locator(PROMPT).fill(text);
  await page.locator(PROMPT).press("Enter");
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

test("/ameliorer : un seul onglet, actif en moins de 300 ms, sans bandeau ; étapes jusqu'à « fusionné » dans le fil", async ({
  page,
}) => {
  await open(page);
  const rowsBefore = await page.evaluate(() => (window as unknown as { __orch: Orch }).__orch.store.rows.length);
  await improve(page, "Bandeau plus discret");
  await expect(page.locator(ITAB)).toHaveCount(1);
  await expect(page.locator(`${ITAB} [role="tab"]`)).toHaveAttribute("aria-selected", "true");
  const delay = await page.evaluate(() => {
    const w = window as unknown as { __activeAt: number | null; __enterAt: number | null };
    return w.__activeAt! - w.__enterAt!;
  });
  expect(delay).toBeLessThan(300);
  await expect(page.locator(BANNER)).toHaveCount(0);
  await expect(page.locator(ITAB)).toContainText("Bandeau plus discret");
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "merged", { timeout: 10_000 });
  await expect(page.locator(`${ITAB} [data-testid="tab-badge"]`)).toHaveText("fusionné");
  expect(await steps(page)).toEqual(STEPS);
  expect(await badges(page)).toContain("tour 1");
  await expect(page.locator(BANNER)).toHaveCount(0);
  await expect(page.locator(TAB)).toHaveCount(2);
  // Le fil de l'onglet : la demande, le travail de la session, les lignes d'état.
  const rows = await improveRows(page);
  expect(rows.find((r) => r.kind === "user")?.text).toBe("Bandeau plus discret");
  const texts = rows.map((r) => r.text).join("\n");
  for (const t of ["✻ Préparation de la branche auto/", "✓ Garde de sécurité (tour 1)", "✻ Contrôles…", "✻ Tests…", "✓ Fusionné mock100"])
    expect(texts).toContain(t);
  await expect(page.locator('[data-row-kind="plain"]').filter({ hasText: "✓ Fusionné mock100" })).toBeVisible();
  // Rien n'est parti vers claude.exe ; la conversation principale est intacte.
  const calls = await page.evaluate(() => (window as unknown as { __orch: Orch }).__orch.calls.map((c) => c.cmd));
  expect(calls).not.toContain("send_message");
  expect(await suCalls(page)).toContain("improve_start");
  expect(await page.evaluate(() => (window as unknown as { __orch: Orch }).__orch.store.rows.length)).toBe(rowsBefore);
  expect(await page.evaluate(() => (window as unknown as { __orch: Orch }).__orch.router()!.roles.get("improve-1"))).toBe(
    "improve",
  );
  // Interface sur disque : le hash actif diffère de celui de cette page.
  await expect(page.getByTestId("ui-hash")).toContainText("(disque)");
  await expect(page.getByTestId("ui-mismatch")).toContainText("actif mock100");
});

test("tests en échec : erreur renvoyée à la session, tour de correction 1/3, puis fusion", async ({ page }) => {
  await open(page, "&improve=fix");
  await improve(page, "Corriger seul");
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "merged", { timeout: 10_000 });
  expect(await steps(page)).toEqual([
    "preparing",
    "working",
    "checking",
    "testing",
    "fixing",
    "checking",
    "testing",
    "building",
    "waiting_idle",
    "switching",
    "merged",
  ]);
  expect(await badges(page)).toContain("correction · 1/3");
  const rows = await improveRows(page);
  const feedback = rows.filter((r) => r.kind === "user" && r.text.startsWith("[Contrôle automatique — tentative 1/3]"));
  expect(feedback).toHaveLength(1);
  expect(feedback[0].text).toContain("npm test : 2 tests en échec");
  const texts = rows.map((r) => r.text).join("\n");
  expect(texts).toContain("✗ Tests en échec (tentative 1/3) : erreur renvoyée à la session");
  expect(texts).toContain("✻ Correction automatique (tentative 1/3)");
  expect(texts).toContain("✓ Garde de sécurité (tour 2)");
});

test("4 échecs : 3 corrections puis attente ; « Valider » actif seulement en attente ; un message remet le compteur", async ({
  page,
}) => {
  await open(page, "&improve=exhausted");
  await improve(page, "Toujours rouge");
  const validate = page.getByTestId("improve-validate");
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "working");
  await expect(validate).toBeDisabled();
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "awaiting_user", { timeout: 15_000 });
  const seen = await steps(page);
  expect(seen.filter((s) => s === "fixing")).toHaveLength(3);
  expect(seen.filter((s) => s === "testing")).toHaveLength(4);
  expect(await status(page)).toMatchObject({ step: "awaiting_user", waiting: "attempts_exhausted", attempt: 3 });
  await expect(page.locator(`${ITAB} [data-testid="tab-badge"]`)).toContainText("en attente");
  await expect(validate).toBeEnabled();
  // Un message de l'utilisateur : nouveau tour, compteur à 0, puis les tests passent.
  await say(page, "Essaie sans toucher au rattachement");
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "working");
  expect(await status(page)).toMatchObject({ step: "working", attempt: 0 });
  await expect(validate).toBeDisabled();
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "merged", { timeout: 10_000 });
  const cmds = await suCalls(page);
  expect(cmds).toContain("improve_send");
  expect(cmds).not.toContain("improve_validate");
  expect((await improveRows(page)).some((r) => r.kind === "user" && r.text === "Essaie sans toucher au rattachement")).toBe(
    true,
  );
});

test("tour sans commit : attente, « Valider » relance contrôles et tests", async ({ page }) => {
  await open(page, "&improve=nocommit");
  await improve(page, "Question d'abord");
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "awaiting_user", { timeout: 10_000 });
  expect(await steps(page)).toEqual(["preparing", "working", "awaiting_user"]);
  expect(await status(page)).toMatchObject({ waiting: "no_new_commit", turn: 1 });
  expect((await improveRows(page)).map((r) => r.text).join("\n")).toContain("⏸ Tour terminé sans nouveau commit");
  await page.getByTestId("improve-validate").click();
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "merged", { timeout: 10_000 });
  // Pas de commit sur la branche : les contrôles le renvoient à la session (tentative 1/3).
  expect((await steps(page)).slice(2, 5)).toEqual(["awaiting_user", "checking", "fixing"]);
  expect(await suCalls(page)).toContain("improve_validate");
});

test("message pendant un tour : livré au tour suivant, sans tests entre les deux", async ({ page }) => {
  await open(page, "", 1500);
  await improve(page, "Long tour");
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "working");
  await say(page, "Et garde le thème clair");
  await expect(page.locator(`${ITAB} [data-testid="tab-badge"]`)).toHaveText("tour 2", { timeout: 5_000 });
  expect(await steps(page)).toEqual(["preparing", "working"]);
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "checking", { timeout: 5_000 });
  expect(await steps(page)).toEqual(["preparing", "working", "checking"]);
  expect(await status(page)).toMatchObject({ turn: 2, attempt: 0 });
  const users = (await improveRows(page)).filter((r) => r.kind === "user").map((r) => r.text);
  expect(users).toEqual(["Long tour", "Et garde le thème clair"]);
});

test("message pendant les tests : tests abandonnés en moins de 1 s, nouveau tour", async ({ page }) => {
  await open(page, "", 1500);
  await improve(page, "Tests longs");
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "testing", { timeout: 10_000 });
  const t0 = Date.now();
  await say(page, "Change plutôt la couleur");
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "working", { timeout: 1_000 });
  expect(Date.now() - t0).toBeLessThan(1_000);
  expect((await improveRows(page)).map((r) => r.text).join("\n")).toContain("✻ Contrôles abandonnés : nouveau message");
  expect(await status(page)).toMatchObject({ step: "working", turn: 1, attempt: 0 });
});

test("Échap pendant un tour : interrompu, en attente", async ({ page }) => {
  await open(page, "", 1500);
  await improve(page, "Interrompre");
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "working", { timeout: 5_000 });
  await page.locator(PROMPT).focus();
  await page.keyboard.press("Escape");
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "awaiting_user", { timeout: 2_000 });
  expect(await status(page)).toMatchObject({ waiting: "interrupted" });
  expect(await suCalls(page)).toContain("improve_interrupt");
  await expect(page.getByTestId("improve-validate")).toBeEnabled();
});

test("fermeture pendant le cycle : confirmation dans la page, « Continuer » puis « Abandonner » → annulé", async ({
  page,
}) => {
  const dialogs = await open(page, "", 600);
  await improve(page, "Abandon");
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "working");
  await page.locator(`${ITAB} [data-testid="tab-close"]`).click();
  const confirm = page.getByTestId("improve-close-confirm");
  await expect(confirm).toBeVisible();
  await expect(confirm).toContainText("Fermer abandonne l'amélioration");
  await expect(confirm).toContainText("auto/20261117-1200-abandon");
  await page.getByTestId("improve-continue").click();
  await expect(confirm).toHaveCount(0);
  await expect(page.locator(ITAB)).toHaveCount(1);
  expect(await suCalls(page)).not.toContain("improve_cancel");
  await page.locator(`${ITAB} [data-testid="tab-close"]`).click();
  await page.getByTestId("improve-abandon").click();
  await expect(page.locator(ITAB)).toHaveCount(0);
  expect(await status(page)).toMatchObject({ step: "cancelled" });
  const cmds = await suCalls(page);
  expect(cmds).toContain("improve_cancel");
  await expect.poll(() => suCalls(page)).toContain("improve_dismiss");
  // Aucune boîte de dialogue du navigateur (`confirm()`), à aucun moment.
  expect(dialogs).toEqual([]);
  // Le cycle ne repart pas.
  await page.waitForTimeout(1500);
  expect(await status(page)).toMatchObject({ step: "cancelled" });
  await expect(page.locator(TAB)).toHaveCount(1);
});

test("fermeture refusée pendant la bascule", async ({ page }) => {
  const dialogs = await open(page, "", 1500);
  await improve(page, "Bascule");
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "switching", { timeout: 15_000 });
  await page.locator(`${ITAB} [data-testid="tab-close"]`).click();
  await expect(page.getByTestId("improve-close-confirm")).toHaveCount(0);
  await expect(page.getByTestId("prompt-hint")).toContainText("Fermeture impossible pendant la bascule");
  await expect(page.locator(ITAB)).toHaveCount(1);
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "merged", { timeout: 5_000 });
  expect(await suCalls(page)).not.toContain("improve_cancel");
  expect(dialogs).toEqual([]);
  // État final : la croix ferme directement et libère la conversation.
  await page.locator(`${ITAB} [data-testid="tab-close"]`).click();
  await expect(page.locator(ITAB)).toHaveCount(0);
  await expect.poll(() => suCalls(page)).toContain("improve_dismiss");
});

test("rechargement en attente : même onglet, actif, même nombre de lignes", async ({ page }) => {
  await open(page, "&improve=nocommit");
  await improve(page, "Recharger");
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "awaiting_user", { timeout: 10_000 });
  const before = await improveRows(page);
  expect(before.length).toBeGreaterThan(3);
  await page.reload();
  await page.waitForFunction(() => (window as unknown as { __orch?: Orch }).__orch?.store.status === "Idle");
  await expect(page.locator(ITAB)).toHaveCount(1);
  await expect(page.locator(`${ITAB} [role="tab"]`)).toHaveAttribute("aria-selected", "true");
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "awaiting_user");
  await expect.poll(async () => (await improveRows(page)).length).toBe(before.length);
  expect(await improveRows(page)).toEqual(before);
  await expect(page.getByTestId("improve-validate")).toBeEnabled();
  await expect(page.locator(BANNER)).toHaveCount(0);
});

test("rechargement par la bascule : l'onglet d'amélioration reste actif et montre « fusionné »", async ({ page }) => {
  await open(page, "", 1500);
  await improve(page, "Basculer");
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "switching", { timeout: 15_000 });
  // La bascule recharge l'interface (le cœur simulé reprend son état).
  await page.reload();
  await page.waitForFunction(() => (window as unknown as { __orch?: Orch }).__orch?.store.status === "Idle");
  await expect(page.locator(ITAB)).toHaveCount(1);
  await expect(page.locator(`${ITAB} [role="tab"]`)).toHaveAttribute("aria-selected", "true");
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "merged", { timeout: 5_000 });
  await expect(page.locator(`${ITAB} [data-testid="tab-badge"]`)).toHaveText("fusionné");
  await expect(page.locator('[data-row-kind="plain"]').filter({ hasText: "✓ Fusionné mock100" })).toBeVisible();
});

test("second /ameliorer : refusé, l'onglet existant est activé", async ({ page }) => {
  await open(page, "", 600);
  await improve(page, "Premier");
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "working");
  await page.locator(`${TAB}:not([data-kind="improve"]) [role="tab"]`).click();
  await expect(page.locator(`${ITAB} [role="tab"]`)).toHaveAttribute("aria-selected", "false");
  await page.locator(PROMPT).focus();
  await page.keyboard.type("/ameliorer Second");
  await page.keyboard.press("Enter");
  await expect(page.locator(`${ITAB} [role="tab"]`)).toHaveAttribute("aria-selected", "true");
  await expect(page.locator(ITAB)).toHaveCount(1);
  expect((await suCalls(page)).filter((c) => c === "improve_start")).toHaveLength(1);
});

test("violation de la garde : échec, rien n'est renvoyé à la session", async ({ page }) => {
  await open(page, "&improve=violation");
  await improve(page, "Toucher la config");
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "failed", { timeout: 10_000 });
  expect(await steps(page)).toEqual(["preparing", "working", "failed"]);
  await expect(page.locator(`${ITAB} [data-testid="tab-badge"]`)).toHaveText("échec");
  const rows = await improveRows(page);
  expect(rows.filter((r) => r.kind === "user")).toHaveLength(1);
  expect(rows.some((r) => r.text.includes("[Contrôle automatique"))).toBe(false);
  expect(rows.map((r) => r.text).join("\n")).toContain("✗ Échec");
  expect(await status(page)).toMatchObject({ step: "failed", turn: 1, waiting: null });
});

test("retour arrière par la palette (/revenir-version)", async ({ page }) => {
  await open(page);
  await improve(page, "x");
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "merged", { timeout: 10_000 });
  await page.locator(`${TAB}:not([data-kind="improve"]) [role="tab"]`).click();
  await page.locator(PROMPT).focus();
  await page.keyboard.type("/revenir-version");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("prompt-hint")).toContainText("Interface embarquée réactivée");
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "rolled_back");
  await expect(page.getByTestId("ui-hash")).toContainText("(embarquée)");
  expect((await suCalls(page)).filter((c) => c === "ui_rollback")).toHaveLength(1);
});

test("relance requise : badge et ligne d'état", async ({ page }) => {
  await open(page, "&improve=restart");
  await improve(page, "Changer le cœur");
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "restart_required", { timeout: 10_000 });
  expect(await steps(page)).toEqual(["preparing", "working", "checking", "testing", "restart_required"]);
  await expect(page.locator(`${ITAB} [data-testid="tab-badge"]`)).toHaveText("relance requise");
  await expect(page.getByTestId("ui-restart")).toContainText("relance requise (mock100)");
  expect((await improveRows(page)).map((r) => r.text).join("\n")).toContain("relance requise");
  // Pas de bascule : l'interface reste l'embarquée.
  await expect(page.getByTestId("ui-hash")).toContainText("(embarquée)");
});

test("envoi : rien de nouveau sans le réglage du propriétaire", async ({ page }) => {
  await open(page);
  await improve(page, "Sans envoi");
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "merged", { timeout: 10_000 });
  await page.waitForTimeout(400);
  expect((await improveRows(page)).some((r) => r.text.includes("GitHub"))).toBe(false);
});

test("envoi du propriétaire : « poussé sur GitHub : auto/… » dans le fil", async ({ page }) => {
  await open(page, "&push=pushed");
  await improve(page, "Avec envoi");
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "merged", { timeout: 10_000 });
  await expect
    .poll(async () => (await improveRows(page)).map((r) => r.text).join("\n"), { timeout: 5_000 })
    .toMatch(/poussé sur GitHub : auto\/\d{8}-\d{4}-avec-envoi/);
});

test("envoi du propriétaire en échec : fusion locale gardée, raison dans le fil", async ({ page }) => {
  await open(page, "&push=failed&improve=restart");
  await improve(page, "Envoi en panne");
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "restart_required", { timeout: 10_000 });
  await expect
    .poll(async () => (await improveRows(page)).map((r) => r.text).join("\n"), { timeout: 5_000 })
    .toContain("fusionné localement, envoi échoué : ssh: connect to host github.com");
});

test("échec définitif : badge « échec », détail dans le fil", async ({ page }) => {
  await open(page, "&improve=failed");
  await improve(page, "Casser");
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "failed", { timeout: 10_000 });
  await expect(page.locator(`${ITAB} [data-testid="tab-badge"]`)).toHaveText("échec");
  expect((await improveRows(page)).map((r) => r.text).join("\n")).toContain("registre npm injoignable");
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
  await expect(page.locator(ITAB)).toHaveCount(0);
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

test("demande sur plusieurs lignes : titre d'onglet sur une ligne ; /revenir-version grisé pendant le cycle", async ({
  page,
}) => {
  await open(page, "", 600);
  await page.keyboard.type("/amel");
  await page.keyboard.press("Tab");
  await page.keyboard.type("Première ligne");
  await page.keyboard.press("Shift+Enter");
  await page.keyboard.type("seconde ligne");
  await expect(page.locator(PROMPT)).toHaveValue("/ameliorer Première ligne\nseconde ligne");
  await page.keyboard.press("Enter");
  await expect(page.locator(ITAB)).toHaveAttribute("data-step", "working");
  await expect(page.locator(`${ITAB} .label`)).toContainText("Première ligne seconde");
  const tabH = (await page.locator(ITAB).boundingBox())!.height;
  expect(tabH).toBeLessThan(30);
  await page.locator(`${TAB}:not([data-kind="improve"]) [role="tab"]`).click();
  await page.locator(PROMPT).focus();
  await page.keyboard.type("/revenir");
  await expect(page.locator(`${PALETTE} [data-name="revenir-version"]`)).toContainText("une amélioration est en cours");
});
