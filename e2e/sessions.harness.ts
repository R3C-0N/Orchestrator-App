// Banc des sélecteurs de J11 (mode simulé) : monte `ProjectPicker` ou
// `SessionPicker` seuls, avec l'IPC Tauri simulé par `mockIPC`, sans passer
// par `App.svelte`. Chargé par `sessions.spec.ts` dans une page servie par
// Vite (`page.route`), exposé sur `window.__sessions`.
import { mount, unmount } from "svelte";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import "../src/app.css";
import ProjectPicker from "../src/lib/components/ProjectPicker.svelte";
import SessionPicker from "../src/lib/components/SessionPicker.svelte";
import { SessionStore } from "../src/lib/state/session.svelte";
import { resumeSession, type HistoryLoaded } from "../src/lib/state/sessions.svelte";
import type { ProjectSummary } from "../src/lib/core/bindings/ProjectSummary";
import type { SessionSummary } from "../src/lib/core/bindings/SessionSummary";

type Call = { cmd: string; args: unknown };

export type Harness = {
  calls: Call[];
  picked: unknown[];
  cancelled: number;
  mountProjects(projects: ProjectSummary[] | { error: unknown }): void;
  mountSessions(cwd: string, sessions: SessionSummary[], opts?: { delayMs?: number }): number;
  resume(history: HistoryLoaded | { error: unknown }): Promise<{
    rows: { kind: string; text?: string; origin?: string }[];
    gaps: number;
    lastSeq: number;
    banners: { code: string }[];
    calls: string[];
  }>;
};

const target = document.getElementById("app")!;
let current: ReturnType<typeof mount> | null = null;

const h: Harness = {
  calls: [],
  picked: [],
  cancelled: 0,

  mountProjects(projects) {
    reset();
    mockIPC((cmd, args) => {
      h.calls.push({ cmd, args });
      if (cmd === "list_projects") {
        if ("error" in projects) throw projects.error;
        return projects;
      }
      throw { kind: "Unsupported", message: cmd };
    });
    current = mount(ProjectPicker, {
      target,
      props: {
        onpick: (path: string) => h.picked.push(path),
        oncancel: () => h.cancelled++,
      },
    });
  },

  mountSessions(cwd, sessions, opts = {}) {
    reset();
    mockIPC(async (cmd, args) => {
      h.calls.push({ cmd, args });
      if (cmd === "list_sessions") {
        if (opts.delayMs) await new Promise((r) => setTimeout(r, opts.delayMs));
        return sessions;
      }
      throw { kind: "Unsupported", message: cmd };
    });
    const t0 = performance.now();
    current = mount(SessionPicker, {
      target,
      props: {
        cwd,
        onpick: (id: string) => h.picked.push(id),
        onnew: () => h.picked.push("new"),
        oncancel: () => h.cancelled++,
      },
    });
    return t0;
  },

  async resume(history) {
    reset();
    const store = new SessionStore((cb) => cb());
    const order: string[] = [];
    await resumeSession(
      {
        stopSession: async () => {
          order.push("stop_session");
          return { code: 0, killed: false };
        },
        startSession: async (_cwd, onEvents, o) => {
          order.push(`start_session:${o.resume}`);
          // Le cœur a déjà émis `starting` (seq 0) sur le canal.
          onEvents([{ seq: 0, t: 1, session: "k-1", kind: "starting", pid: 1, cwd: _cwd, args: [] }]);
          return { session: "k-1", pid: 1, claude_path: "claude.exe" };
        },
        loadHistory: async (_s, id) => {
          order.push(`load_history:${id}`);
          if ("error" in history) throw history.error;
          return history;
        },
      },
      { current: "k-0", cwd: "C:\\demo", sessionId: "0b8f3c2e-1111-4a6b-9c1d-000000000001", store },
    );
    // Événement vivant suivant (seq 1) : aucun trou ne doit être compté.
    store.apply([{ seq: 1, t: 2, session: "k-1", kind: "status", state: "Idle" }]);
    return {
      rows: store.rows.map((r) => ({
        kind: r.kind,
        text: "text" in r ? r.text : undefined,
        origin: "origin" in r ? r.origin : undefined,
      })),
      gaps: store.gaps,
      lastSeq: store.lastSeq,
      banners: store.banners.map((b) => ({ code: b.code })),
      calls: order,
    };
  },
};

function reset() {
  if (current) unmount(current);
  current = null;
  clearMocks();
  h.calls = [];
  h.picked = [];
  h.cancelled = 0;
}

(window as unknown as { __sessions: Harness }).__sessions = h;
