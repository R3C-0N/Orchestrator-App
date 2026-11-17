// J14, étape 0 : commandes d'auto-amélioration simulées (`MockSelfUpdate`),
// conformes au contrat, pour travailler sur l'interface sans le cœur.
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ImproveStatus } from "../../src/lib/core/bindings/ImproveStatus";
import { MOCK_IMPROVE_STEPS, MockSelfUpdate } from "../../src/lib/core/mock";

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
});
