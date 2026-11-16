// Tests du magasin de session (plan 1.10, point 5), sur des événements
// écrits à la main conformes au contrat, en attendant `fixtures/ui-events/`.
import { describe, expect, it } from "vitest";
import type { CoreEvent } from "../../src/lib/core/bindings/CoreEvent";
import type { EventBody } from "../../src/lib/core/bindings/EventBody";
import { EventFactory, MockCore, buildHistory } from "../../src/lib/core/mock";
import { SessionStore } from "../../src/lib/state/session.svelte";

function setup() {
  const f = new EventFactory();
  const s = new SessionStore(() => {
    throw new Error("planificateur non attendu");
  });
  const ev = (...bodies: EventBody[]): CoreEvent[] => bodies.map((b) => f.make(b));
  return { f, s, ev };
}

const text = (msg: string, index = 0, thread = "main") => ({ thread, msg, index });

describe("SessionStore", () => {
  it("compte exactement une ligne par Row de l'historique synthétique", () => {
    const { f, s } = setup();
    const { events, stats } = buildHistory(f, 3000);
    s.apply(events);
    expect(stats.rows).toBe(3000);
    // J5 : chaque agent d'arrière-plan qui finit ajoute sa ligne « Agent … finished » (capture 06).
    expect(s.rows.length).toBe(3000 + stats.agents);
    expect(s.gaps).toBe(0);
    expect(s.live).toBeNull();
  });

  it("accumule les text_delta dans la ligne vivante sans remplacer le tableau", () => {
    const { s, ev } = setup();
    s.apply(ev({ kind: "block_start", ...text("m1"), block: { type: "Text" } }));
    const rowsAfterStart = s.rows;
    expect(s.rows).toHaveLength(1);
    s.apply(ev({ kind: "text_delta", ...text("m1"), text: "Bon" }, { kind: "text_delta", ...text("m1"), text: "jour" }));
    expect(s.live?.text).toBe("Bonjour");
    // Même tableau : les deltas ne coûtent pas une copie de `rows`.
    expect(s.rows).toBe(rowsAfterStart);
    const r0 = s.rows[0];
    expect(r0.kind === "assistant" && r0.text).toBe("Bonjour");
  });

  it("block_done fait foi et remplace le texte accumulé", () => {
    const { s, ev } = setup();
    s.apply(
      ev(
        { kind: "block_start", ...text("m1"), block: { type: "Text" } },
        { kind: "text_delta", ...text("m1"), text: "brouillon" },
        { kind: "block_done", ...text("m1"), block: { type: "Text", text: "texte final" } },
      ),
    );
    expect(s.live).toBeNull();
    expect(s.rows).toHaveLength(1);
    expect(s.rows[0]).toMatchObject({ kind: "assistant", text: "texte final", done: true });
  });

  it("applique plusieurs lots reçus dans la même image en une seule fois", () => {
    const f = new EventFactory();
    const frames: Array<() => void> = [];
    const s = new SessionStore((cb) => frames.push(cb));
    let applied = 0;
    s.onApplied = () => applied++;
    s.push([f.make({ kind: "block_start", ...text("m1"), block: { type: "Text" } })]);
    s.push([f.make({ kind: "text_delta", ...text("m1"), text: "a" })]);
    s.push([f.make({ kind: "text_delta", ...text("m1"), text: "b" })]);
    expect(frames).toHaveLength(1);
    expect(s.rows).toHaveLength(0);
    expect(s.pending).toBe(3);
    frames[0]();
    expect(applied).toBe(1);
    expect(s.live?.text).toBe("ab");
  });

  it("rattache tool_result à l'appel et garde une ligne par outil", () => {
    const { s, ev } = setup();
    s.apply(
      ev(
        { kind: "block_start", ...text("m1"), block: { type: "ToolUse", tool_use_id: "tu1", name: "Read" } },
        {
          kind: "block_done",
          ...text("m1"),
          block: { type: "ToolUse", tool_use_id: "tu1", name: "Read", input: { file_path: "a.ts" } },
        },
        { kind: "tool_result", thread: "main", tool_use_id: "tu1", content: [{ type: "Text", text: "ok" }], is_error: false, meta: null },
      ),
    );
    expect(s.rows).toHaveLength(1);
    expect(s.rows[0]).toMatchObject({ kind: "tool", name: "Read", input: { file_path: "a.ts" }, result: { text: "ok", isError: false } });
  });

  it("met à jour l'appel Agent sur une seule ligne et range le sous-agent dans threads", () => {
    const { s, ev } = setup();
    const agent = {
      tool_use_id: "ta",
      task_id: null,
      agent_type: "Explore",
      color: "blue",
      description: "chercher",
      status: "Running" as const,
      background: true,
      started_at: 1,
      ended_at: null,
      last_tool: null,
      tokens: null,
      tool_uses: null,
      summary: null,
    };
    s.apply(
      ev(
        { kind: "block_start", ...text("m1"), block: { type: "ToolUse", tool_use_id: "ta", name: "Agent" } },
        { kind: "agent", ...agent },
        { kind: "block_start", ...text("s1", 0, "ta"), block: { type: "Text" } },
        { kind: "text_delta", ...text("s1", 0, "ta"), text: "sous" },
        { kind: "block_done", ...text("s1", 0, "ta"), block: { type: "Text", text: "sous-agent" } },
        { kind: "agent", ...agent, status: "Completed" },
      ),
    );
    // L'appel reste une seule ligne ; J5 ajoute la ligne de fin de l'agent d'arrière-plan (06).
    expect(s.rows).toHaveLength(2);
    expect(s.rows[0]).toMatchObject({ kind: "agent", agent: { status: "Completed" } });
    expect(s.rows[1]).toMatchObject({ kind: "agentDone", toolUseId: "ta" });
    expect(s.agents).toHaveLength(1);
    expect(s.live).toBeNull();
    expect(s.threads.get("ta")?.[0]).toMatchObject({ kind: "assistant", text: "sous-agent", done: true });
  });

  it("ignore les Replay, affiche les interruptions et fige le bloc ouvert à la fin du tour", () => {
    const { s, ev } = setup();
    s.apply(
      ev(
        { kind: "user_message", thread: "main", uuid: null, text: "salut", origin: "Typed" },
        { kind: "user_message", thread: "main", uuid: "u", text: "salut", origin: "Replay" },
        { kind: "block_start", ...text("m1"), block: { type: "Text" } },
        { kind: "text_delta", ...text("m1"), text: "coupé" },
        { kind: "user_message", thread: "main", uuid: null, text: "[Request interrupted by user]", origin: "Interrupted" },
        {
          kind: "turn_result",
          subtype: "error_during_execution",
          is_error: true,
          terminal_reason: "aborted_streaming",
          duration_ms: 1000,
          cost_usd: 0,
          num_turns: 1,
          context_used: null,
          context_window: null,
          permission_denials: [],
          result: null,
        },
      ),
    );
    expect(s.rows.map((r) => r.kind)).toEqual(["user", "assistant", "interrupted", "turn"]);
    expect(s.rows[1]).toMatchObject({ text: "coupé", done: true });
    expect(s.live).toBeNull();
  });

  it("met les notices Warn en bandeau, Info hors du fil, et signale un trou de seq", () => {
    const { f, s } = setup();
    s.apply([f.make({ kind: "notice", level: "Info", code: "auto_allow", message: "Bash", detail: null })]);
    f.seq += 3;
    s.apply([f.make({ kind: "notice", level: "Warn", code: "unexpected_version", message: "v", detail: null })]);
    expect(s.infos).toHaveLength(1);
    expect(s.banners).toHaveLength(1);
    expect(s.rows).toHaveLength(0);
    expect(s.gaps).toBe(1);
  });

  it("le faux cœur regroupe les événements en lots ≤ 256 avec seq continu", () => {
    const m = new MockCore({ rows: 500 });
    const batches: CoreEvent[][] = [];
    m.start((b) => batches.push(b));
    expect(Math.max(...batches.map((b) => b.length))).toBeLessThanOrEqual(256);
    const all = batches.flat();
    expect(all.every((e, i) => e.seq === i)).toBe(true);
    const s = new SessionStore();
    s.apply(all);
    expect(s.rows.length).toBe(500 + (m.stats?.agents ?? 0));
    expect(s.status).toBe("Idle");
  });

  it("B2 : deux bandeaux identiques ont des id distincts", () => {
    const { s } = setup();
    s.addBanner("Error", "send", "échec");
    s.addBanner("Error", "send", "échec");
    expect(new Set(s.banners.map((b) => b.id)).size).toBe(2);
  });

  it("B3 : un événement qui lève une exception est ignoré sans désaligner les index", () => {
    const { s, ev } = setup();
    const errors: unknown[] = [];
    const orig = console.error;
    console.error = (...a: unknown[]) => errors.push(a);
    try {
      s.apply(
        ev(
          { kind: "block_start", ...text("m1"), block: { type: "ToolUse", tool_use_id: "tu1", name: "Read" } },
          // `content` invalide : l'application de cet événement lève une exception.
          { kind: "tool_result", thread: "main", tool_use_id: "tu1", content: null as never, is_error: false, meta: null },
          { kind: "user_message", thread: "main", uuid: null, text: "après", origin: "Typed" },
          { kind: "tool_result", thread: "main", tool_use_id: "tu1", content: [{ type: "Text", text: "ok" }], is_error: false, meta: null },
        ),
      );
    } finally {
      console.error = orig;
    }
    expect(s.failed).toBe(1);
    expect(errors).toHaveLength(1);
    // Le lot est publié : les lignes créées avant et après l'erreur sont là.
    expect(s.rows.map((r) => r.kind)).toEqual(["tool", "user"]);
    expect(s.byToolUse.get("tu1")).toBe(0);
    expect(s.rows[0]).toMatchObject({ kind: "tool", result: { text: "ok" } });
  });

  it("B3 : un événement agent n'écrase jamais une ligne d'un autre genre", () => {
    const { s, ev } = setup();
    s.apply(ev({ kind: "user_message", thread: "main", uuid: null, text: "u", origin: "Typed" }));
    // Index corrompu volontairement : tu9 pointe sur la ligne utilisateur.
    s.byToolUse.set("tu9", 0);
    s.apply(
      ev({
        kind: "agent",
        tool_use_id: "tu9",
        task_id: null,
        agent_type: "Explore",
        color: null,
        description: "d",
        status: "Running",
        background: true,
        started_at: null,
        ended_at: null,
        last_tool: null,
        tokens: null,
        tool_uses: null,
        summary: null,
      }),
    );
    expect(s.rows[0]).toMatchObject({ kind: "user", text: "u" });
  });

  it("transmet le t de chaque text_delta du lot (latence par delta)", () => {
    const { f, s } = setup();
    let ts: number[] = [];
    s.onApplied = (i) => (ts = i.deltaTs);
    s.apply([
      f.make({ kind: "block_start", ...text("m1"), block: { type: "Text" } }, 1),
      f.make({ kind: "text_delta", ...text("m1"), text: "a" }, 10),
      f.make({ kind: "text_delta", ...text("m1"), text: "b" }, 20),
    ]);
    expect(ts).toEqual([10, 20]);
  });
});
