// J14, étape 0 : commandes d'auto-amélioration simulées (`MockSelfUpdate`),
// conformes au contrat, pour travailler sur l'interface sans le cœur.
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ImproveStatus } from "../../src/lib/core/bindings/ImproveStatus";
import {
  MOCK_FAIL_DETAIL,
  MOCK_IMPROVE_STEPS,
  MockSelfUpdate,
  mockImproveSteps,
  mockSelfUpdateFromUrl,
} from "../../src/lib/core/mock";

afterEach(() => {
  vi.useRealTimers();
});

describe("MockSelfUpdate", () => {
  it("répond neutre : interface embarquée, aucune session à rattacher", async () => {
    const m = new MockSelfUpdate();
    expect(await m.attachSessions()).toEqual([]);
    const info = await m.getUiInfo();
    expect(info.source).toBe("embedded");
    expect(info.active_hash).toBeNull();
    expect(await m.uiReady(null)).toBe(false);
    expect(m.readyCalls).toEqual([null]);
  });

  it("déroule toutes les étapes d'un cycle, jusqu'à la fusion", async () => {
    vi.useFakeTimers();
    const seen: ImproveStatus[] = [];
    const m = new MockSelfUpdate((s) => seen.push(s), 100);
    const first = await m.improveStart("Bandeau plus discret");
    expect(first.step).toBe("preparing");
    expect(first.branch).toMatch(/^auto\/\d{8}-\d{4}-bandeau-plus-discret$/);
    vi.advanceTimersByTime(100 * MOCK_IMPROVE_STEPS.length);
    expect([first, ...seen].map((s) => s.step)).toEqual(MOCK_IMPROVE_STEPS);
    const info = await m.getUiInfo();
    expect(info.source).toBe("disk");
    expect(info.active_hash).toBe(seen.at(-1)?.commit);
    const back = await m.uiRollback();
    expect(back.source).toBe("embedded");
  });

  it("annule un cycle en cours et refuse un clone indisponible", async () => {
    vi.useFakeTimers();
    const seen: ImproveStatus[] = [];
    const m = new MockSelfUpdate((s) => seen.push(s), 100);
    const first = await m.improveStart("x");
    await m.improveCancel(first.id);
    vi.advanceTimersByTime(10_000);
    expect(seen.map((s) => s.step)).toEqual(["cancelled"]);
    m.info.repo = { state: "disabled", reason: "git absent" };
    await expect(m.improveStart("y")).rejects.toMatchObject({ kind: "unsupported" });
  });

  it("relance requise : ni build ni bascule, commit marqué dans UiInfo", async () => {
    vi.useFakeTimers();
    const seen: ImproveStatus[] = [];
    const m = new MockSelfUpdate(() => {}, 100, { outcome: "restart_required" });
    await m.onImproveStatus((s) => seen.push(s));
    await m.improveStart("cœur");
    vi.advanceTimersByTime(1000);
    expect(seen.map((s) => s.step)).toEqual(mockImproveSteps("restart_required").slice(1));
    const info = await m.getUiInfo();
    expect(info.restart_required).toBe(seen.at(-1)?.commit);
    expect(info.source).toBe("embedded");
  });

  it("échec avec détail ; retour arrière émis comme `rolled_back`", async () => {
    vi.useFakeTimers();
    const seen: ImproveStatus[] = [];
    const m = new MockSelfUpdate((s) => seen.push(s), 100, { outcome: "failed" });
    await m.improveStart("x");
    vi.advanceTimersByTime(1000);
    expect(seen.at(-1)).toMatchObject({ step: "failed", detail: MOCK_FAIL_DETAIL });
    await m.uiRollback();
    expect(seen.at(-1)?.step).toBe("rolled_back");
  });

  it("dépôt désactivé : la raison remonte ; options lues dans l'URL", async () => {
    const o = mockSelfUpdateFromUrl("?mock=demo&improve=restart&repo=disabled&attach=12&step=50");
    expect(o.outcome).toBe("restart_required");
    expect(o.stepMs).toBe(50);
    expect(o.live?.map((r) => r.role)).toEqual(["main", "improve"]);
    const m = new MockSelfUpdate(() => {}, 100, o);
    await expect(m.improveStart("y")).rejects.toMatchObject({ kind: "unsupported", message: expect.stringContaining("git") });
    const live = await m.attachSessions(() => {});
    expect(live[0].events[0].kind).toBe("starting");
    expect(live[0].events.every((e, i) => e.seq === i)).toBe(true);
    expect(mockSelfUpdateFromUrl("?mock=demo")).toEqual({});
  });
});

describe("bandeau d'amélioration", () => {
  it("demande sur plusieurs lignes : une seule ligne, tronquée", async () => {
    const { oneLine } = await import("../../src/lib/components/ImproveBanner.svelte");
    expect(oneLine("Bandeau\n  plus\r\ndiscret\t")).toBe("Bandeau plus discret");
    expect(oneLine("x".repeat(300), 10)).toBe(`${"x".repeat(9)}…`);
  });

  it("attachFail : les n premiers rattachements échouent", async () => {
    const m = new MockSelfUpdate(() => {}, 300, mockSelfUpdateFromUrl("?attachFail=2"));
    await expect(m.attachSessions()).rejects.toMatchObject({ kind: "internal" });
    await expect(m.attachSessions()).rejects.toMatchObject({ kind: "internal" });
    expect(await m.attachSessions()).toEqual([]);
  });
});
