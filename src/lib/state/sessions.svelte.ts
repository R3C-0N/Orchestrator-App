// Projets récents, sessions d'un projet et reprise avec historique (J11,
// plan 1.8, décisions 9 et 11).
//
// - `ProjectList` / `SessionList` : magasins des deux sélecteurs (chargement,
//   filtre, sélection au clavier), alimentés par `list_projects` et
//   `list_sessions` (lecture seule de `~/.claude/projects` côté Rust).
// - `applyHistory` : applique au magasin de session les `CoreEvent` relus
//   dans le transcript (`load_history`), sans compter de trou de `seq`.
// - `resumeSession` : enchaînement complet de la reprise.

import { invoke } from "@tauri-apps/api/core";
import type { CoreEvent } from "../core/bindings/CoreEvent";
import type { ExitInfo } from "../core/bindings/ExitInfo";
import type { HistoryLoaded } from "../core/bindings/HistoryLoaded";
import type { ProjectSummary } from "../core/bindings/ProjectSummary";
import type { SessionKey } from "../core/bindings/SessionKey";
import type { SessionStarted } from "../core/bindings/SessionStarted";
import type { SessionSummary } from "../core/bindings/SessionSummary";
import type { SessionStore } from "./session.svelte";

export type { HistoryLoaded, ProjectSummary, SessionSummary };

// --- commandes ----------------------------------------------------------------

export const listProjects = () => invoke<ProjectSummary[]>("list_projects");

export const listSessions = (cwd: string) => invoke<SessionSummary[]>("list_sessions", { cwd });

export const loadHistory = (session: SessionKey, session_id: string) =>
  invoke<HistoryLoaded>("load_history", { session, session_id });

/** Message lisible d'une erreur de commande (`CoreError` = `{ kind, message? }`). */
export function errorText(e: unknown): string {
  if (e && typeof e === "object" && "kind" in e) {
    const ce = e as { kind: string; message?: string };
    return ce.message ? `${ce.kind} : ${ce.message}` : ce.kind;
  }
  return String(e);
}

// --- mise en forme ------------------------------------------------------------

/** « à l'instant », « il y a 5 min », « il y a 3 h », « hier », « il y a 4 j », puis la date. */
export function relativeTime(ms: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 60) return "à l'instant";
  const m = Math.floor(s / 60);
  if (m < 60) return `il y a ${m} min`;
  const h = Math.floor(m / 60);
  if (h < 24) return `il y a ${h} h`;
  const d = Math.floor(h / 24);
  if (d === 1) return "hier";
  if (d < 30) return `il y a ${d} j`;
  const dt = new Date(ms);
  return `${dt.getFullYear()}-${String(dt.getMonth() + 1).padStart(2, "0")}-${String(dt.getDate()).padStart(2, "0")}`;
}

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} o`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} ko`;
  return `${(bytes / 1024 / 1024).toFixed(1)} Mo`;
}

/** Libellé d'un projet : son vrai chemin, sinon le nom encodé du dossier. */
export const projectLabel = (p: ProjectSummary) => p.path ?? p.encoded;

/** Libellé d'une session : titre, sinon dernier prompt (session `-p`), sinon l'id. */
export const sessionLabel = (s: SessionSummary) => s.title ?? s.last_prompt ?? `(sans titre) ${s.id.slice(0, 8)}`;

// --- magasins des sélecteurs --------------------------------------------------

type Loader<T> = () => Promise<T[]>;

/** Liste filtrable avec sélection au clavier, commune aux deux sélecteurs. */
class PickerList<T> {
  items: T[] = $state.raw([]);
  filter = $state("");
  selected = $state(0);
  loading = $state(false);
  error: string | null = $state(null);
  /** Durée du dernier chargement (ms), affichée au pied du sélecteur. */
  loadMs: number | null = $state(null);

  readonly visible: T[] = $derived.by(() => {
    const f = this.filter.trim().toLowerCase();
    if (!f) return this.items;
    return this.items.filter((x) => this.#text(x).toLowerCase().includes(f));
  });

  readonly #load: Loader<T>;
  readonly #text: (x: T) => string;
  readonly #enabled: (x: T) => boolean;

  constructor(load: Loader<T>, text: (x: T) => string, enabled: (x: T) => boolean = () => true) {
    this.#load = load;
    this.#text = text;
    this.#enabled = enabled;
  }

  async refresh(): Promise<void> {
    this.loading = true;
    this.error = null;
    const t0 = performance.now();
    try {
      this.items = await this.#load();
      this.loadMs = Math.round(performance.now() - t0);
      this.selected = 0;
      this.#skipDisabled(1);
    } catch (e) {
      this.error = errorText(e);
      this.items = [];
    } finally {
      this.loading = false;
    }
  }

  setFilter(f: string): void {
    this.filter = f;
    this.selected = 0;
    this.#skipDisabled(1);
  }

  /** Déplace la sélection (`delta` = ±1), en sautant les entrées grisées. */
  move(delta: number): void {
    const n = this.visible.length;
    if (n === 0) return;
    let i = this.selected;
    for (let k = 0; k < n; k++) {
      i = Math.min(n - 1, Math.max(0, i + delta));
      if (this.#enabled(this.visible[i])) {
        this.selected = i;
        return;
      }
      if (i === 0 || i === n - 1) return;
    }
  }

  isEnabled(x: T): boolean {
    return this.#enabled(x);
  }

  get current(): T | null {
    const x = this.visible[this.selected];
    return x !== undefined && this.#enabled(x) ? x : null;
  }

  #skipDisabled(dir: number): void {
    const v = this.visible;
    let i = this.selected;
    while (i >= 0 && i < v.length && !this.#enabled(v[i])) i += dir;
    if (i >= 0 && i < v.length) this.selected = i;
  }
}

/** Projets récents de `~/.claude/projects` ; un dossier disparu est grisé, non sélectionnable. */
export class ProjectList extends PickerList<ProjectSummary> {
  constructor(load: Loader<ProjectSummary> = listProjects) {
    super(load, projectLabel, (p) => p.exists && p.path !== null);
  }
}

/** Sessions d'un projet, de la plus récente à la plus ancienne (200 au plus). */
export class SessionList extends PickerList<SessionSummary> {
  readonly cwd: string;

  constructor(cwd: string, load: Loader<SessionSummary> = () => listSessions(cwd)) {
    super(load, (s) => `${sessionLabel(s)} ${s.last_prompt ?? ""} ${s.id}`);
    this.cwd = cwd;
  }
}

// --- reprise -----------------------------------------------------------------

/**
 * Applique l'historique relu au magasin d'une session, **avant** les
 * événements vivants (appeler dès le retour de `start_session`). Les `seq`
 * de l'historique repartent de 0 : le compteur de trous n'en tient pas compte
 * et le dernier `seq` vivant est rétabli.
 */
export function applyHistory(store: SessionStore, h: HistoryLoaded): void {
  if (h.events.length === 0) return;
  const lastSeq = store.lastSeq;
  const gaps = store.gaps;
  store.lastSeq = -1;
  store.apply(h.events as CoreEvent[]);
  store.lastSeq = lastSeq;
  store.gaps = gaps;
}

export type ResumeDeps = {
  stopSession: (session: SessionKey) => Promise<ExitInfo>;
  startSession: (
    cwd: string,
    onEvents: (batch: CoreEvent[]) => void,
    opts: { resume?: string | null; model?: string | null },
  ) => Promise<SessionStarted>;
  loadHistory?: (session: SessionKey, sessionId: string) => Promise<HistoryLoaded>;
};

/**
 * Reprise (plan 1.5) : `stop_session` de la session courante, puis
 * `start_session(cwd, resume = id)` sur un magasin **neuf**, puis
 * `load_history`. Un historique illisible n'empêche pas la reprise : bandeau
 * d'avertissement `history_unavailable`, la session continue (décision 9).
 */
export async function resumeSession(
  deps: ResumeDeps,
  args: { current: SessionKey | null; cwd: string; sessionId: string; store: SessionStore },
): Promise<{ started: SessionStarted; history: HistoryLoaded | null }> {
  if (args.current) {
    try {
      await deps.stopSession(args.current);
    } catch {
      // Déjà arrêtée : rien à faire.
    }
  }
  const started = await deps.startSession(args.cwd, args.store.push, { resume: args.sessionId });
  let history: HistoryLoaded | null = null;
  try {
    history = await (deps.loadHistory ?? loadHistory)(started.session, args.sessionId);
    applyHistory(args.store, history);
  } catch (e) {
    args.store.addBanner(
      "Warn",
      "history_unavailable",
      `Session reprise sans historique affiché : ${errorText(e)}`,
    );
  }
  return { started, history };
}
