// Client du cœur Rust : appels typés des commandes Tauri (plan 1.5).
// Les types viennent de ./bindings, générés depuis Rust par ts-rs
// (`npm run bindings`) : ne pas les éditer à la main.

import { Channel, invoke } from "@tauri-apps/api/core";
import type { Answer } from "./bindings/Answer";
import type { CoreError } from "./bindings/CoreError";
import type { CoreEvent } from "./bindings/CoreEvent";
import type { Diagnostics } from "./bindings/Diagnostics";
import type { ExitInfo } from "./bindings/ExitInfo";
import type { InterruptAck } from "./bindings/InterruptAck";
import type { LaunchInfo } from "./bindings/LaunchInfo";
import type { SessionKey } from "./bindings/SessionKey";
import type { SessionStarted } from "./bindings/SessionStarted";
import type { SessionSummary } from "./bindings/SessionSummary";
import type { SkipReason } from "./bindings/SkipReason";

export type {
  Answer,
  CoreError,
  CoreEvent,
  Diagnostics,
  ExitInfo,
  InterruptAck,
  LaunchInfo,
  SessionKey,
  SessionStarted,
  SessionSummary,
  SkipReason,
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

export const loadHistory = (session: SessionKey, session_id: string) =>
  invoke<void>("load_history", { session, session_id });

export const stopSession = (session: SessionKey) => invoke<ExitInfo>("stop_session", { session });

export const getDiagnostics = () => invoke<Diagnostics>("get_diagnostics");
