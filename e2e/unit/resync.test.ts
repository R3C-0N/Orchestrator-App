// J14, étape 4 : rattachement de l'interface rechargée aux sessions vivantes.
// Rejouer l'historique repris puis le journal (`SessionResync`) doit donner
// exactement l'état obtenu par le flux direct, sans trou ni doublon, même
// quand le canal livre avant la réponse ou recouvre le journal.
import { describe, expect, it } from "vitest";
import type { CoreEvent } from "../../src/lib/core/bindings/CoreEvent";
import type { HistoryLoaded } from "../../src/lib/core/bindings/HistoryLoaded";
import type { SessionResync } from "../../src/lib/core/bindings/SessionResync";
import { EventFactory, buildHistory, mockLiveSessions } from "../../src/lib/core/mock";
import { SessionStore } from "../../src/lib/state/session.svelte";
import {
  SessionRouter,
  UI_SNAPSHOT_KEY,
  applyHistory,
  loadUiSnapshot,
  mainOf,
  resyncStore,
  saveUiSnapshot,
} from "../../src/lib/state/sessions.svelte";

/** Magasin appliqué tout de suite : `push` passe par un planificateur synchrone. */
const syncStore = () => new SessionStore((cb) => cb());

/** Ce qui se voit d'une session (les `id` d'affichage compris). */
function view(s: SessionStore) {
  return JSON.parse(
    JSON.stringify({
      rows: s.rows,
      agents: s.agents,
      questions: s.questions,
      meta: s.meta,
      status: s.status,
      banners: s.banners.map(({ id: _id, ...b }) => b),
      threads: [...s.threads.entries()],
      live: s.live,
      lastSeq: s.lastSeq,
      gaps: s.gaps,
      failed: s.failed,
    }),
  );
}

function history(rows: number): HistoryLoaded {
  const f = new EventFactory();
  return {
    session_id: "s-hist",
    events: buildHistory(f, rows, 99).events,
    lines: rows,
    used: rows,
    ignored: 0,
    unknown: 0,
    unknown_elements: 0,
    bad: 0,
    sidechain: 0,
    truncated: false,
    subagent_threads: 0,
    omitted_lines: 0,
    omitted_bytes: 0,
    clipped: 0,
    dropped_events: 0,
  };
}

/** Journal vivant : démarrage, tours complets, puis un bloc en streaming resté ouvert. */
function journal(rows: number, key = "k1"): CoreEvent[] {
  const f = new EventFactory();
  const ev = buildHistory(f, rows, 7).events.map((e) => ({ ...e, session: key }));
  const mk = (body: Parameters<EventFactory["make"]>[0]) => ({ ...f.make(body), session: key });
  ev.push(mk({ kind: "status", state: "Streaming" }));
  ev.push(mk({ kind: "block_start", thread: "main", msg: "m-live", index: 0, block: { type: "Text" } }));
  for (let i = 0; i < 20; i++) ev.push(mk({ kind: "text_delta", thread: "main", msg: "m-live", index: 0, text: `d${i} ` }));
  return ev;
}

/** Découpe en lots de tailles variées (comme le regroupeur du cœur). */
function batches<T>(xs: T[], sizes = [1, 7, 256, 3, 31]): T[][] {
  const out: T[][] = [];
  let i = 0;
  let k = 0;
  while (i < xs.length) {
    const n = sizes[k++ % sizes.length];
    out.push(xs.slice(i, i + n));
    i += n;
  }
  return out;
}

function resync(key: string, events: CoreEvent[], h: HistoryLoaded | null = null): SessionResync {
  return { session: key, role: "main", cwd: "C:\\p", pid: 1, resumed_from: h?.session_id ?? null, history: h, events, running: true };
}

describe("resyncStore", () => {
  it("historique + journal = même état que le flux direct", () => {
    const h = history(300);
    const events = journal(500);
    // Flux direct : historique appliqué au début, puis les lots du canal.
    const direct = syncStore();
    applyHistory(direct, h);
    for (const b of batches(events)) direct.push(b);
    // Rattachement : le tout d'un coup.
    const again = syncStore();
    resyncStore(again, resync("k1", events, h));
    expect(view(again)).toEqual(view(direct));
    expect(again.gaps).toBe(0);
    expect(again.lastSeq).toBe(events.length - 1);
    expect(again.live?.text).toContain("d19");
  });

  it("ne déclenche pas la mesure de latence pendant le rejeu", () => {
    const s = syncStore();
    let calls = 0;
    s.onApplied = () => calls++;
    resyncStore(s, resync("k1", journal(50)));
    expect(calls).toBe(0);
    expect(s.onApplied).not.toBeNull();
  });
});

describe("SessionRouter", () => {
  it("lots du canal avant la réponse et recouvrant le journal : 0 trou, 0 doublon", () => {
    const all = journal(400);
    const cut = Math.floor(all.length * 0.7);
    const logged = all.slice(0, cut);
    const direct = syncStore();
    for (const b of batches(all)) direct.push(b);

    const r = new SessionRouter(() => syncStore());
    // Le canal livre avant la réponse, en recouvrant la fin du journal.
    for (const b of batches(all.slice(cut - 40))) r.onBatch(b);
    expect(r.attached).toBe(false);
    r.resync([resync("k1", logged)]);
    // Puis le flux continue (un lot rejoué en double compris).
    const s = r.get("k1")!;
    expect(view(s)).toEqual(view(direct));
    r.onBatch(all.slice(-5));
    expect(view(s)).toEqual(view(direct));
    expect(s.gaps).toBe(0);
    expect(r.duplicates).toBe(40 + 5);
  });

  it("garde la session d'amélioration à part de la principale", () => {
    const live = mockLiveSessions(30);
    const r = new SessionRouter(() => syncStore());
    r.resync(live);
    const main = mainOf(live)!;
    expect(main.session).toBe("mock-1");
    expect(r.roles.get("improve-0")).toBe("improve");
    const before = r.get("mock-1")!.rows.length;
    // Événements de la session dédiée, puis d'une session inconnue (nouvelle amélioration).
    const f = new EventFactory();
    r.onBatch([
      { ...f.make({ kind: "user_message", thread: "main", uuid: null, text: "x", origin: "Typed" }), session: "improve-0", seq: 2 },
      { ...f.make({ kind: "status", state: "Streaming" }), session: "improve-7", seq: 0 },
    ]);
    expect(r.get("mock-1")!.rows.length).toBe(before);
    expect(r.get("improve-0")!.rows.map((x) => x.kind)).toEqual(["user"]);
    expect(r.roles.get("improve-7")).toBe("improve");
    expect(r.get("improve-7")!.status).toBe("Streaming");
  });

  it("aucune session : les lots gardés passent quand même", () => {
    const r = new SessionRouter(() => syncStore());
    const f = new EventFactory();
    r.onBatch([{ ...f.make({ kind: "status", state: "Idle" }), session: "z" }]);
    r.resync([]);
    expect(r.get("z")?.status).toBe("Idle");
    expect(mainOf([])).toBeNull();
  });

  it("10 000 événements rapides : rattachés en moins d'une seconde, sans trou", () => {
    // Tours complets, puis une réponse en streaming (deltas) jusqu'à 10 000 événements.
    const f = new EventFactory();
    const { events } = buildHistory(f, 2500, 3);
    events.push(f.make({ kind: "block_start", thread: "main", msg: "m-fast", index: 0, block: { type: "Text" } }));
    while (events.length < 10_000)
      events.push(f.make({ kind: "text_delta", thread: "main", msg: "m-fast", index: 0, text: "x" }));
    expect(events.length).toBe(10_000);
    const r = new SessionRouter(() => syncStore());
    const t0 = performance.now();
    r.resync([resync("mock-1", events)]);
    const ms = performance.now() - t0;
    const s = r.get("mock-1")!;
    expect(s.lastSeq).toBe(events.length - 1);
    expect(s.gaps).toBe(0);
    expect(s.failed).toBe(0);
    expect(ms).toBeLessThan(1000);
  });
});

describe("UiSnapshot", () => {
  it("aller-retour par sessionStorage, valeurs invalides ignorées", () => {
    const mem = new Map<string, string>();
    const fake = {
      getItem: (k: string) => mem.get(k) ?? null,
      setItem: (k: string, v: string) => void mem.set(k, v),
    };
    const g = globalThis as unknown as { sessionStorage?: unknown };
    const prev = g.sessionStorage;
    g.sessionStorage = fake;
    try {
      expect(loadUiSnapshot()).toBeNull();
      const snap = {
        session: "k1",
        draft: "en cours",
        sent: ["a", "b"],
        agentsOpen: ["toolu_1"],
        agentsDoneOpen: true,
        verbose: true,
        improveDismissed: null,
      };
      saveUiSnapshot(snap);
      expect(loadUiSnapshot()).toEqual(snap);
      mem.set(UI_SNAPSHOT_KEY, JSON.stringify({ draft: 3, sent: ["x", 4], verbose: "oui" }));
      expect(loadUiSnapshot()).toEqual({
        session: null,
        draft: "",
        sent: ["x"],
        agentsOpen: [],
        agentsDoneOpen: false,
        verbose: false,
        improveDismissed: null,
      });
      mem.set(UI_SNAPSHOT_KEY, "{pas du json");
      expect(loadUiSnapshot()).toBeNull();
      // Stockage qui lève : ni exception, ni état.
      g.sessionStorage = {
        getItem: () => {
          throw new Error("refusé");
        },
        setItem: () => {
          throw new Error("plein");
        },
      };
      expect(() => saveUiSnapshot(snap)).not.toThrow();
      expect(loadUiSnapshot()).toBeNull();
    } finally {
      g.sessionStorage = prev;
    }
  });
});
