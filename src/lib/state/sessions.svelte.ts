// Projets récents, sessions d'un projet et reprise avec historique (J11,
// plan 1.8, décisions 9 et 11).
//
// - `ProjectList` / `SessionList` : magasins des deux sélecteurs (chargement,
//   filtre, sélection au clavier), alimentés par `list_projects` et
//   `list_sessions` (lecture seule de `~/.claude/projects` côté Rust).
// - `applyHistory` : applique au magasin de session les `CoreEvent` relus
//   dans le transcript (`load_history`), sans compter de trou de `seq`.
// - `resumeSession` : enchaînement complet de la reprise.
// - J14 : `resyncStore` / `SessionRouter` (rattachement aux sessions vivantes
//   après un rechargement de l'interface) et `UiSnapshot` (saisie, historique
//   ↑↓, agents dépliés, Ctrl+O gardés dans `sessionStorage`).

import { invoke } from "@tauri-apps/api/core";
import type { CoreEvent } from "../core/bindings/CoreEvent";
import type { ExitInfo } from "../core/bindings/ExitInfo";
import type { HistoryLoaded } from "../core/bindings/HistoryLoaded";
import type { ProjectSummary } from "../core/bindings/ProjectSummary";
import type { SessionKey } from "../core/bindings/SessionKey";
import type { SessionResync } from "../core/bindings/SessionResync";
import type { SessionRole } from "../core/bindings/SessionRole";
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

/** Texte de la ligne « début omis » d'après les compteurs de `load_history`, ou `null`. */
export function historyCutText(h: Pick<HistoryLoaded, "omitted_lines" | "dropped_events">): string | null {
  if (h.omitted_lines > 0) return `Début de l'historique omis (${h.omitted_lines} lignes)`;
  if (h.dropped_events > 0) return `Début de l'historique omis (${h.dropped_events} événements)`;
  return null;
}

/**
 * Applique l'historique relu au magasin d'une session, **avant** les
 * événements vivants (appeler dès le retour de `start_session`). Les `seq`
 * de l'historique repartent de 0 : le compteur de trous n'en tient pas compte
 * et le dernier `seq` vivant est rétabli. Historique tronqué : une ligne
 * grise « Début de l'historique omis (…) » le précède dans le fil (elle
 * défile avec lui).
 */
export function applyHistory(store: SessionStore, h: HistoryLoaded): void {
  const cut = historyCutText(h);
  if (cut) store.addPlain(cut);
  if (h.events.length === 0) return;
  const lastSeq = store.lastSeq;
  const gaps = store.gaps;
  store.lastSeq = -1;
  try {
    store.apply(h.events as CoreEvent[]);
  } finally {
    // Rétablis même si l'application lève une exception.
    store.lastSeq = lastSeq;
    store.gaps = gaps;
  }
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

// --- rattachement après un rechargement (J14, décision 15) ---------------------

/**
 * Reconstruit le magasin d'une session vivante à partir de `attach_sessions` :
 * historique repris (comme `applyHistory`), puis journal complet des
 * `CoreEvent` (`seq` continus depuis 0), appliqués tout de suite. La mesure
 * de latence (`onApplied`) est suspendue : ces événements ne sont pas peints
 * « en direct ».
 */
export function resyncStore(store: SessionStore, r: Pick<SessionResync, "history" | "events">): void {
  const hook = store.onApplied;
  store.onApplied = null;
  try {
    if (r.history) applyHistory(store, r.history);
    if (r.events.length > 0) store.apply(r.events);
  } finally {
    store.onApplied = hook;
  }
}

/**
 * Aiguillage des lots du canal de `attach_sessions` (toutes les sessions) vers
 * le magasin de chacune.
 *
 * - Avant `resync`, les lots reçus sont gardés : le canal peut livrer avant la
 *   réponse de la commande.
 * - Après, un événement dont le `seq` est déjà couvert par le journal (ou déjà
 *   reçu) est écarté : ni trou, ni doublon, quel que soit le recouvrement
 *   entre le journal et le canal.
 * - Une session inconnue (lancée après le rattachement, une amélioration par
 *   exemple) reçoit un magasin à la volée, de rôle `improve` sauf indication.
 * - `make` peut rendre `null` : la session n'a alors pas de magasin (jamais
 *   affichée, une amélioration) ; seuls ses `TAIL_MAX` derniers événements
 *   sont gardés (`tail`), la mémoire reste bornée.
 * - `dispose` : les lots arrivés ensuite sont ignorés (rattachement abandonné).
 */
export class SessionRouter {
  readonly stores = new Map<SessionKey, SessionStore>();
  readonly roles = new Map<SessionKey, SessionRole>();
  readonly info = new Map<SessionKey, SessionResync>();
  /** Événements écartés parce que déjà appliqués (recouvrement journal / canal). */
  duplicates = 0;
  static readonly TAIL_MAX = 50;
  #last = new Map<SessionKey, number>();
  #tails = new Map<SessionKey, CoreEvent[]>();
  #buffer: CoreEvent[][] | null = [];
  #disposed = false;
  readonly #make: (key: SessionKey, role: SessionRole) => SessionStore | null;

  constructor(make: (key: SessionKey, role: SessionRole) => SessionStore | null) {
    this.#make = make;
  }

  /** Derniers événements d'une session sans magasin (au plus `TAIL_MAX`). */
  tail(key: SessionKey): CoreEvent[] {
    return this.#tails.get(key) ?? [];
  }

  /** Abandon du rattachement : plus rien n'est gardé ni aiguillé. */
  dispose(): void {
    this.#disposed = true;
    this.#buffer = null;
    this.#tails.clear();
  }

  /** Vrai une fois `resync` fait (les lots passent alors directement). */
  get attached(): boolean {
    return this.#buffer === null;
  }

  /** À passer comme `onEvents` de `attachSessions`. */
  onBatch = (batch: CoreEvent[]): void => {
    if (this.#disposed) return;
    if (this.#buffer) this.#buffer.push(batch);
    else this.#route(batch);
  };

  /** Reconstruit chaque session, puis applique les lots gardés depuis l'appel. */
  resync(list: SessionResync[]): void {
    for (const r of list) {
      const s = this.#make(r.session, r.role);
      if (s) {
        resyncStore(s, r);
        this.stores.set(r.session, s);
      } else {
        this.#keepTail(r.session, r.events);
      }
      this.roles.set(r.session, r.role);
      this.info.set(r.session, r);
      const last = r.events.length > 0 ? r.events[r.events.length - 1].seq : -1;
      this.#last.set(r.session, last);
    }
    const buf = this.#buffer ?? [];
    this.#buffer = null;
    for (const b of buf) this.#route(b);
  }

  /** Magasin de la session `key`, s'il existe. */
  get(key: SessionKey): SessionStore | undefined {
    return this.stores.get(key);
  }

  #route(batch: CoreEvent[]): void {
    let key: SessionKey | null = null;
    let run: CoreEvent[] = [];
    const flush = () => {
      if (key !== null && run.length > 0) {
        const s = this.#storeOf(key);
        if (s) s.push(run);
        else this.#keepTail(key, run);
      }
      run = [];
    };
    for (const ev of batch) {
      const last = this.#last.get(ev.session);
      if (last !== undefined && ev.seq <= last) {
        this.duplicates++;
        continue;
      }
      this.#last.set(ev.session, ev.seq);
      if (ev.session !== key) {
        flush();
        key = ev.session;
      }
      run.push(ev);
    }
    flush();
  }

  #storeOf(key: SessionKey): SessionStore | null {
    const s = this.stores.get(key);
    if (s) return s;
    if (this.roles.has(key)) {
      // Déjà vue : avec un magasin, il serait dans `stores`.
      return null;
    }
    const role: SessionRole = "improve";
    this.roles.set(key, role);
    const made = this.#make(key, role);
    if (made) this.stores.set(key, made);
    return made;
  }

  #keepTail(key: SessionKey, evs: CoreEvent[]): void {
    const t = (this.#tails.get(key) ?? []).concat(evs);
    this.#tails.set(key, t.length > SessionRouter.TAIL_MAX ? t.slice(-SessionRouter.TAIL_MAX) : t);
  }
}

/**
 * Session principale à afficher après un rattachement : la dernière de rôle
 * `main` encore en cours, sinon la dernière de rôle `main`.
 *
 * Les autres sessions `main` (cas anormal : le cœur n'en lance qu'une) ne sont
 * ni affichées ni arrêtées : elles gardent leur magasin dans le routeur et
 * continuent côté cœur jusqu'à leur fin.
 */
export function mainOf(list: SessionResync[]): SessionResync | null {
  const mains = list.filter((r) => r.role === "main");
  return mains.filter((r) => r.running).at(-1) ?? mains.at(-1) ?? null;
}

// --- état de l'interface gardé à travers un rechargement -------------------------

/** Ce que l'interface garde d'une page à l'autre (`sessionStorage`, décision 15). */
export type UiSnapshot = {
  /** Session principale à laquelle cet état appartient. */
  session: SessionKey | null;
  draft: string;
  /** Historique ↑↓ des envois de cette saisie. */
  sent: string[];
  agentsOpen: string[];
  agentsDoneOpen: boolean;
  /** Vue détaillée (Ctrl+O). */
  verbose: boolean;
  /** Bandeau d'amélioration fermé (id de l'amélioration). */
  improveDismissed: string | null;
};

export const UI_SNAPSHOT_KEY = "orchestrator.ui.v1";

function sessionStore(): Storage | null {
  try {
    return globalThis.sessionStorage ?? null;
  } catch {
    return null;
  }
}

/** Plafonds de l'état gardé (caractères) : par entrée, et au total. */
export type UiSnapshotLimits = { entry: number; total: number };
export const UI_SNAPSHOT_LIMITS: Readonly<UiSnapshotLimits> = { entry: 100_000, total: 1_000_000 };

/**
 * Forme plafonnée de l'état : chaque texte (brouillon, envois) coupé à
 * `entry` caractères, puis les envois les plus anciens retirés tant que le
 * total dépasse `total`. Le brouillon passe toujours.
 */
export function capUiSnapshot(s: UiSnapshot, limits: UiSnapshotLimits = UI_SNAPSHOT_LIMITS): UiSnapshot {
  const cut = (x: string) => (x.length > limits.entry ? x.slice(0, limits.entry) : x);
  const out: UiSnapshot = {
    ...s,
    draft: cut(s.draft),
    sent: s.sent.map(cut),
    agentsOpen: s.agentsOpen.slice(-200).map(cut),
  };
  let size = JSON.stringify(out).length;
  while (size > limits.total && out.sent.length > 0) {
    size -= JSON.stringify(out.sent[0]).length + 1;
    out.sent = out.sent.slice(1);
  }
  return out;
}

/**
 * Écrit l'état, plafonné (`capUiSnapshot`). Stockage plein : nouvel essai avec
 * le minimum (brouillon et réglages, sans l'historique ↑↓ ni les agents
 * dépliés). Sans effet si le stockage est inaccessible. Rend ce qui a été
 * écrit (`full`, `minimal`) ou `none`.
 */
export function saveUiSnapshot(s: UiSnapshot, limits: UiSnapshotLimits = UI_SNAPSHOT_LIMITS): "full" | "minimal" | "none" {
  const st = sessionStore();
  if (!st) return "none";
  const capped = capUiSnapshot(s, limits);
  try {
    st.setItem(UI_SNAPSHOT_KEY, JSON.stringify(capped));
    return "full";
  } catch {
    // Quota dépassé (ou refus) : on garde au moins le brouillon.
  }
  try {
    st.removeItem(UI_SNAPSHOT_KEY);
    st.setItem(UI_SNAPSHOT_KEY, JSON.stringify({ ...capped, sent: [], agentsOpen: [] }));
    return "minimal";
  } catch {
    return "none";
  }
}

/** Relit l'état, validé champ par champ ; `null` s'il manque ou est illisible. */
export function loadUiSnapshot(): UiSnapshot | null {
  try {
    const raw = sessionStore()?.getItem(UI_SNAPSHOT_KEY);
    if (!raw) return null;
    const v = JSON.parse(raw) as Partial<UiSnapshot>;
    if (!v || typeof v !== "object") return null;
    const strings = (x: unknown): string[] => (Array.isArray(x) ? x.filter((y): y is string => typeof y === "string") : []);
    return {
      session: typeof v.session === "string" ? v.session : null,
      draft: typeof v.draft === "string" ? v.draft : "",
      sent: strings(v.sent).slice(-500),
      agentsOpen: strings(v.agentsOpen),
      agentsDoneOpen: v.agentsDoneOpen === true,
      verbose: v.verbose === true,
      improveDismissed: typeof v.improveDismissed === "string" ? v.improveDismissed : null,
    };
  } catch {
    return null;
  }
}
