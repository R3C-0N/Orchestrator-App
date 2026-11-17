// Client du cœur Rust : appels typés des commandes Tauri (plan 1.5).
// Les types viennent de ./bindings, générés depuis Rust par ts-rs
// (`npm run bindings`) : ne pas les éditer à la main.

import { Channel, invoke } from "@tauri-apps/api/core";
import type { Answer } from "./bindings/Answer";
import type { CoreError } from "./bindings/CoreError";
import type { CoreEvent } from "./bindings/CoreEvent";
import type { Diagnostics } from "./bindings/Diagnostics";
import type { ExitInfo } from "./bindings/ExitInfo";
import type { HistoryLoaded } from "./bindings/HistoryLoaded";
import type { ImproveStatus } from "./bindings/ImproveStatus";
import type { InterruptAck } from "./bindings/InterruptAck";
import type { LaunchInfo } from "./bindings/LaunchInfo";
import type { SessionKey } from "./bindings/SessionKey";
import type { SessionResync } from "./bindings/SessionResync";
import type { SessionStarted } from "./bindings/SessionStarted";
import type { SessionSummary } from "./bindings/SessionSummary";
import type { SkipReason } from "./bindings/SkipReason";
import type { UiInfo } from "./bindings/UiInfo";

export type {
  Answer,
  CoreError,
  CoreEvent,
  Diagnostics,
  ExitInfo,
  HistoryLoaded,
  ImproveStatus,
  InterruptAck,
  LaunchInfo,
  SessionKey,
  SessionResync,
  SessionStarted,
  SessionSummary,
  SkipReason,
  UiInfo,
};

/** Erreur d'une commande : `{ kind, message? }` (CoreError côté Rust). */
export function describeError(e: unknown): string {
  if (e && typeof e === "object" && "kind" in e) {
    const ce = e as CoreError;
    return "message" in ce && ce.message ? `${ce.kind} : ${ce.message}` : ce.kind;
  }
  return String(e);
}

export const getLaunchInfo = () => invoke<LaunchInfo>("get_launch_info");

export function startSession(
  cwd: string,
  onEvents: (batch: CoreEvent[]) => void,
  opts: { resume?: string | null; model?: string | null } = {},
): Promise<SessionStarted> {
  const events = new Channel<CoreEvent[]>();
  events.onmessage = onEvents;
  return invoke<SessionStarted>("start_session", {
    cwd,
    resume: opts.resume ?? null,
    model: opts.model ?? null,
    events,
  });
}

export const sendMessage = (session: SessionKey, text: string) =>
  invoke<void>("send_message", { session, text });

export const interrupt = (session: SessionKey, cancel_queued = false) =>
  invoke<InterruptAck>("interrupt", { session, cancel_queued });

export const setModel = (session: SessionKey, model: string) =>
  invoke<string>("set_model", { session, model });

export const answerQuestion = (
  session: SessionKey,
  request_id: string,
  answers: Record<string, Answer>,
  response: string | null = null,
) => invoke<void>("answer_question", { session, request_id, answers, response });

export const skipQuestion = (session: SessionKey, request_id: string, reason: SkipReason) =>
  invoke<void>("skip_question", { session, request_id, reason });

export const listSessions = (cwd: string) => invoke<SessionSummary[]>("list_sessions", { cwd });

/** Historique relu dans le transcript (J11) : `CoreEvent` déjà réduits et compteurs. */
export const loadHistory = (session: SessionKey, session_id: string) =>
  invoke<HistoryLoaded>("load_history", { session, session_id });

export const stopSession = (session: SessionKey) => invoke<ExitInfo>("stop_session", { session });

export const getDiagnostics = () => invoke<Diagnostics>("get_diagnostics");

// --- J14 : auto-amélioration (spec, décisions 15 à 19) ------------------------

/**
 * Rattache l'interface (re)chargée aux sessions vivantes du cœur : `onEvents`
 * reçoit ensuite les lots de **toutes** les sessions (chaque `CoreEvent`
 * porte sa `session`), y compris la session d'amélioration. La réponse donne
 * de quoi reconstruire chaque session (historique repris, puis journal).
 */
export function attachSessions(onEvents: (batch: CoreEvent[]) => void): Promise<SessionResync[]> {
  const events = new Channel<CoreEvent[]>();
  events.onmessage = onEvents;
  return invoke<SessionResync[]>("attach_sessions", { events });
}

/** Signal « prêt » (watchdog de 5 s) ; `hash` : `__UI_HASH__`, `null` pour l'interface embarquée. */
export const uiReady = (hash: string | null) => invoke<boolean>("ui_ready", { hash });

export const getUiInfo = () => invoke<UiInfo>("get_ui_info");

/** `/ameliorer <request>` ; l'avancement arrive par l'événement global `IMPROVE_EVENT`. */
export const improveStart = (request: string) => invoke<ImproveStatus>("improve_start", { request });

export const improveCancel = (id: string) => invoke<void>("improve_cancel", { id });

/** Revient à la version précédente de l'interface. */
export const uiRollback = () => invoke<UiInfo>("ui_rollback");

/** Nom de l'événement global Tauri qui porte un `ImproveStatus` (`IMPROVE_EVENT` côté Rust). */
export const IMPROVE_EVENT = "improve_status";

/**
 * Commandes d'auto-amélioration vues par l'interface : réelles (`tauriSelfUpdate`)
 * ou simulées (`MockSelfUpdate`, `mock.ts`), même forme. `onImproveStatus`
 * abonne à l'avancement (`IMPROVE_EVENT`) et rend de quoi se désabonner.
 */
export type SelfUpdateApi = {
  attachSessions(onEvents: (batch: CoreEvent[]) => void): Promise<SessionResync[]>;
  uiReady(hash: string | null): Promise<boolean>;
  getUiInfo(): Promise<UiInfo>;
  improveStart(request: string): Promise<ImproveStatus>;
  improveCancel(id: string): Promise<void>;
  uiRollback(): Promise<UiInfo>;
  onImproveStatus(cb: (s: ImproveStatus) => void): Promise<() => void>;
};

export const tauriSelfUpdate: SelfUpdateApi = {
  attachSessions,
  uiReady,
  getUiInfo,
  improveStart,
  improveCancel,
  uiRollback,
  async onImproveStatus(cb) {
    const { listen } = await import("@tauri-apps/api/event");
    const un = await listen<ImproveStatus>(IMPROVE_EVENT, (e) => cb(e.payload));
    return () => {
      try {
        un?.();
      } catch {
        // Déjà désabonné (page déchargée).
      }
    };
  },
};
