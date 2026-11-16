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
    expect(s.rows.length).toBe(3000);
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
    expect(s.rows).toHaveLength(1);
    expect(s.rows[0]).toMatchObject({ kind: "agent", agent: { status: "Completed" } });
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
    expect(s.rows.length).toBe(500);
    expect(s.status).toBe("Idle");
  });
});
