// J16 : onglet d'amélioration. Modèle pur (`src/lib/state/improve.ts`) :
// badge d'étape, droits de « Valider », d'« Annuler », de la saisie et
// d'Échap, fermeture de l'onglet (confirmation, refus, fermeture directe),
// réouverture au rechargement ; magasin de rôle `improve`
// (`session.svelte.ts`) : lignes d'état `improve.*` dans le fil, sortie
// attendue entre deux tours sans bandeau, pas de limite d'événements.
import { describe, expect, it } from "vitest";
import type { CoreEvent } from "../../src/lib/core/bindings/CoreEvent";
import type { EventBody } from "../../src/lib/core/bindings/EventBody";
import type { ImproveStatus } from "../../src/lib/core/bindings/ImproveStatus";
import type { ImproveStep } from "../../src/lib/core/bindings/ImproveStep";
import {
  FINAL_STEPS,
  STEP_LABEL,
  bannerText,
  canAbandon,
  canCancel,
  canInterrupt,
  canValidate,
  closeAction,
  closeConfirmText,
  confirmKey,
  dismissOnMount,
  improveBadge,
  improveTabLabel,
  improveTabTitle,
  improvePromptLine,
  improveToReopen,
  isFinal,
  isImproveNotice,
  isNewer,
  pushLine,
  sendBlocked,
} from "../../src/lib/state/improve";
import { SessionStore } from "../../src/lib/state/session.svelte";
import { SessionRouter } from "../../src/lib/state/sessions.svelte";

const ALL_STEPS = Object.keys(STEP_LABEL) as ImproveStep[];

const base: ImproveStatus = {
  id: "improve-1",
  branch: "auto/20261119-1200-onglet",
  request: "Onglet\nd'amélioration",
  step: "working",
  session: "improve-1",
  commit: null,
  detail: null,
  push: null,
  t: 1000,
  attempt: 0,
  max_attempts: 3,
  turn: 0,
  waiting: null,
};
const at = (step: ImproveStep, over: Partial<ImproveStatus> = {}): ImproveStatus => ({ ...base, step, ...over });

/** Magasin appliqué tout de suite (planificateur synchrone). */
const improveStore = () => new SessionStore((cb) => cb(), "improve");

function events(session: string, bodies: EventBody[], from = 0): CoreEvent[] {
  return bodies.map((b, i) => ({ ...b, seq: from + i, t: 1000 + i, session }) as CoreEvent);
}

describe("badge d'étape (titre de l'onglet)", () => {
  it("tour, contrôles, tests, en attente, fusionné, échec", () => {
    expect(improveBadge(at("working"))).toEqual({ text: "tour 1", tone: "run" });
    expect(improveBadge(at("working", { turn: 2 }))).toEqual({ text: "tour 3", tone: "run" });
    expect(improveBadge(at("checking")).text).toBe("contrôles");
    expect(improveBadge(at("testing")).text).toBe("tests");
    expect(improveBadge(at("awaiting_user", { waiting: "no_new_commit" }))).toEqual({ text: "en attente", tone: "wait" });
    expect(improveBadge(at("merged"))).toEqual({ text: "fusionné", tone: "ok" });
    expect(improveBadge(at("failed"))).toEqual({ text: "échec", tone: "err" });
    expect(improveBadge(at("restart_required")).tone).toBe("warn");
  });

  it("n/3 pendant une correction automatique, jamais sur un état final", () => {
    expect(improveBadge(at("fixing", { attempt: 2 })).text).toBe("correction · 2/3");
    expect(improveBadge(at("testing", { attempt: 1 })).text).toBe("tests · 1/3");
    expect(improveBadge(at("awaiting_user", { attempt: 3, waiting: "attempts_exhausted" })).text).toBe(
      "en attente · 3/3",
    );
    expect(improveBadge(at("merged", { attempt: 2 })).text).toBe("fusionné");
  });

  it("chaque étape a un badge et un libellé", () => {
    for (const s of ALL_STEPS) {
      expect(improveBadge(at(s)).text.length).toBeGreaterThan(0);
      expect(STEP_LABEL[s].length).toBeGreaterThan(0);
    }
  });
});

describe("droits de l'onglet", () => {
  it("« Valider » actif seulement en awaiting_user", () => {
    for (const s of ALL_STEPS) expect(canValidate(at(s))).toBe(s === "awaiting_user");
    expect(canValidate(null)).toBe(false);
  });

  it("« Annuler l'amélioration » jusqu'à la bascule, jamais après un état final", () => {
    expect(canCancel(at("working"))).toBe(true);
    expect(canCancel(at("awaiting_user"))).toBe(true);
    expect(canCancel(at("testing"))).toBe(true);
    expect(canCancel(at("switching"))).toBe(false);
    for (const s of FINAL_STEPS) expect(canCancel(at(s))).toBe(false);
  });

  it("saisie ouverte pendant un tour, les contrôles, les tests et l'attente ; fermée sinon", () => {
    for (const s of ["working", "fixing", "checking", "testing", "awaiting_user"] as const)
      expect(sendBlocked(at(s))).toBeNull();
    expect(sendBlocked(at("switching"))).toBe(STEP_LABEL.switching);
    expect(sendBlocked(at("merged"))).toContain("terminée");
    expect(sendBlocked(null)).not.toBeNull();
  });

  it("Échap interrompt seulement un tour", () => {
    expect(canInterrupt(at("working"))).toBe(true);
    expect(canInterrupt(at("fixing"))).toBe(true);
    expect(canInterrupt(at("testing"))).toBe(false);
    expect(canInterrupt(at("awaiting_user"))).toBe(false);
  });
});

describe("fermeture de l'onglet", () => {
  it("cycle en cours : confirmation dans la page ; bascule : refusée ; état final : directe", () => {
    expect(closeAction(at("working"))).toBe("confirm");
    expect(closeAction(at("awaiting_user"))).toBe("confirm");
    expect(closeAction(at("testing"))).toBe("confirm");
    expect(closeAction(at("switching"))).toBe("refuse");
    for (const s of FINAL_STEPS) expect(closeAction(at(s))).toBe("dismiss");
    // État inconnu : confirmation, par prudence ; « Abandonner » reste possible.
    expect(closeAction(null)).toBe("confirm");
    expect(canAbandon(null)).toBe(true);
    expect(canAbandon(at("switching"))).toBe(false);
    expect(closeConfirmText(null)).toContain("gardée non fusionnée");
  });

  it("Échap pendant la confirmation vaut « Continuer », jamais une interruption", () => {
    expect(confirmKey("Escape", true)).toBe("continue");
    expect(confirmKey("Escape", false)).toBeNull();
    expect(confirmKey("Enter", true)).toBeNull();
  });

  it("montage : improve_dismiss de l'onglet fermé, tout de suite ou à l'état final", () => {
    expect(dismissOnMount(null, at("working"))).toBeNull();
    // « Abandonner » puis rechargement avant `cancelled` : en attente.
    expect(dismissOnMount("improve-1", at("working"))).toBe("wait");
    expect(dismissOnMount("improve-1", null)).toBe("wait");
    expect(dismissOnMount("improve-1", at("cancelled"))).toBe("dismiss");
    // Un cycle plus récent existe : l'ancien est fini.
    expect(dismissOnMount("improve-0", at("working"))).toBe("dismiss");
  });

  it("texte de la confirmation : la branche gardée non fusionnée", () => {
    expect(closeConfirmText(base)).toBe(
      "Fermer abandonne l'amélioration ; la branche auto/20261119-1200-onglet est gardée non fusionnée.",
    );
  });
});

describe("libellés de l'onglet et de l'état", () => {
  it("« Amélioration · <sujet> » sur une ligne, tronqué", () => {
    expect(improveTabLabel("Onglet\n  d'amélioration")).toBe("Amélioration · Onglet d'amélioration");
    expect(improveTabLabel("x".repeat(100), 10)).toBe(`Amélioration · ${"x".repeat(9)}…`);
    expect(improveTabLabel(null)).toBe("Amélioration");
    expect(improveTabTitle(base)).toContain(base.branch);
  });

  it("état final et attente", () => {
    expect(isFinal("merged")).toBe(true);
    expect(isFinal("awaiting_user")).toBe(false);
    expect(bannerText(at("merged", { commit: "abcdef123" }), null)).toBe(
      "Amélioration fusionnée abcdef1 dans main du clone ; interface active.",
    );
    expect(bannerText(at("awaiting_user", { waiting: "no_new_commit" }), null)).toContain("sans nouveau commit");
    expect(bannerText(at("restart_required", { commit: "abc1234" }), "C:\\clone")).toContain("(C:\\clone)");
  });

  it("le plus récent l'emporte", () => {
    expect(isNewer(at("testing", { t: 2 }), at("checking", { t: 1 }))).toBe(true);
    expect(isNewer(at("checking", { t: 1 }), at("testing", { t: 2 }))).toBe(false);
    expect(isNewer(base, null)).toBe(true);
  });

  it("envoi GitHub : une ligne du fil par changement", () => {
    const pushing = at("merged", { push: { state: "pushing", branch: base.branch } });
    const l1 = pushLine(pushing, null);
    expect(l1).toContain("envoi sur GitHub");
    expect(pushLine(pushing, l1)).toBeNull();
    expect(pushLine(at("merged", { push: { state: "pushed", branch: base.branch } }), l1)).toBe(
      `poussé sur GitHub : ${base.branch}`,
    );
    expect(pushLine(at("merged"), null)).toBeNull();
  });
});

describe("rechargement : réouverture de l'onglet", () => {
  const live = [
    { session: "main-1", role: "main" as const },
    { session: "improve-1", role: "improve" as const },
  ];

  it("conversation d'amélioration rendue par attach_sessions et connue : rouverte", () => {
    expect(improveToReopen(live, at("awaiting_user"), null)).toEqual({ session: "improve-1", status: at("awaiting_user") });
    // État final, onglet pas encore fermé : rouvert aussi (« fusionné » après la bascule).
    expect(improveToReopen(live, at("merged"), null)?.session).toBe("improve-1");
  });

  it("onglet fermé, autre session, ou aucune conversation : pas rouvert", () => {
    expect(improveToReopen(live, at("awaiting_user"), "improve-1")).toBeNull();
    expect(improveToReopen(live, at("working", { session: "improve-9" }), null)).toBeNull();
    expect(improveToReopen(live, at("preparing", { session: null }), null)).toBeNull();
    expect(improveToReopen([live[0]], at("working"), null)).toBeNull();
    expect(improveToReopen(live, null, null)).toBeNull();
  });
});

describe("magasin de la conversation d'amélioration", () => {
  it("les notices improve.* sont des lignes du fil, dans l'ordre ; warn en ligne colorée", () => {
    const s = improveStore();
    s.apply(
      events("improve-1", [
        {
          kind: "notice",
          level: "Warn",
          code: "improve.unmerged_worktrees",
          message: "amélioration(s) précédente(s) non fusionnée(s) : auto/a, auto/b",
          detail: { branches: ["auto/a", "auto/b"] },
        },
        { kind: "notice", level: "Info", code: "improve.preparing", message: "✻ Préparation", detail: null },
        { kind: "notice", level: "Info", code: "improve.check_failed", message: "✗ Contrôle en échec", detail: null },
      ]),
    );
    expect(isImproveNotice("improve.x")).toBe(true);
    expect(s.rows.map((r) => r.kind)).toEqual(["notice", "plain", "plain"]);
    expect(s.rows[0]).toMatchObject({ kind: "notice", level: "Warn", code: "branches non fusionnées" });
    expect(s.rows[2]).toMatchObject({ kind: "plain", text: "✗ Contrôle en échec" });
    expect(s.banners).toEqual([]);
  });

  it("premier message : le prompt du pipeline replié en une ligne ; les suivants tels quels", () => {
    const prompt = ["# Amélioration", "", "## Demande de l'utilisateur", "", "Bouton", "plus discret", "", "## Ce que tu dois faire", "", "1. …"].join("\n");
    expect(improvePromptLine(prompt)).toBe("Consigne d'amélioration · Bouton plus discret");
    expect(improvePromptLine("demande simple")).toBe("Consigne d'amélioration · demande simple");
    const s = improveStore();
    s.apply(
      events("improve-1", [
        { kind: "user_message", thread: "main", uuid: null, text: prompt, origin: "Typed" },
        { kind: "user_message", thread: "main", uuid: null, text: "précise", origin: "Typed" },
      ]),
    );
    expect(s.rows.map((r) => (r.kind === "user" ? r.text : r.kind))).toEqual([
      "Consigne d'amélioration · Bouton plus discret",
      "précise",
    ]);
    expect(s.improvePrompt).toBe(prompt);
  });

  it("arrêt voulu (après improve.turn_end) : silencieux, le tour suivant repart ; sans turn_end : vraie erreur", () => {
    const s = improveStore();
    s.apply(
      events("improve-1", [
        { kind: "starting", pid: 1, cwd: "C:\\wt", args: [] },
        { kind: "status", state: "Idle" },
        { kind: "notice", level: "Info", code: "improve.turn_end", message: "tour terminé : session en pause", detail: { turn: 1, end: "done", pid: 1 } },
        { kind: "status", state: "Exited" },
        { kind: "exited", code: 0, during_turn: false, stderr_tail: [] },
        { kind: "starting", pid: 2, cwd: "C:\\wt", args: ["--resume", "improve-1"] },
        { kind: "status", state: "Streaming" },
      ]),
    );
    expect(s.banners).toEqual([]);
    expect(s.rows.map((r) => r.kind)).toEqual(["plain"]);
    expect(s.status).toBe("Streaming");
    expect(s.gaps).toBe(0);
    // Le processus du tour 2 meurt sans `improve.turn_end` : bandeau d'erreur.
    s.apply(events("improve-1", [{ kind: "exited", code: 1, during_turn: true, stderr_tail: [] }], 7));
    expect(s.banners.map((b) => b.code)).toEqual(["exited"]);
  });

  it("magasin principal inchangé : bandeau exited, notice improve.* hors du fil", () => {
    const s = new SessionStore((cb) => cb());
    s.apply(
      events("main-1", [
        { kind: "notice", level: "Warn", code: "improve.x", message: "m", detail: null },
        { kind: "exited", code: 0, during_turn: false, stderr_tail: [] },
      ]),
    );
    expect(s.rows).toEqual([]);
    expect(s.banners.map((b) => b.code)).toEqual(["improve.x", "exited"]);
  });

  it("aucune limite d'événements quand l'app donne un magasin (plus de TAIL_MAX)", () => {
    const r = new SessionRouter((_k, role) => (role === "improve" ? improveStore() : null));
    r.resync([]);
    const many = events(
      "improve-1",
      Array.from({ length: 500 }, (_, i): EventBody => ({
        kind: "notice",
        level: "Info",
        code: "improve.step",
        message: `étape ${i}`,
        detail: null,
      })),
    );
    r.onBatch(many);
    const s = r.get("improve-1")!;
    expect(s.role).toBe("improve");
    expect(s.rows).toHaveLength(500);
    expect(s.lastSeq).toBe(499);
    expect(r.tail("improve-1")).toEqual([]);
  });
});
