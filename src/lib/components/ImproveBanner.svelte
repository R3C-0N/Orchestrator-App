<script module lang="ts">
  // Modèle pur du bandeau (libellés, états finaux), testable sans le DOM.
  import type { ImproveStatus } from "../core/bindings/ImproveStatus";
  import type { ImproveStep } from "../core/bindings/ImproveStep";

  /** Étapes d'un cycle en cours, dans l'ordre (décision 17). */
  export const RUNNING_STEPS: ImproveStep[] = [
    "preparing",
    "working",
    "checking",
    "testing",
    "building",
    "waiting_idle",
    "switching",
  ];

  export const STEP_LABEL: Record<ImproveStep, string> = {
    preparing: "préparation du worktree",
    working: "Claude modifie le code (session dédiée)",
    checking: "contrôle de la branche",
    testing: "tests (check, unitaires, e2e)",
    building: "build de l'interface",
    waiting_idle: "attente de la fin du tour en cours",
    switching: "bascule vers la nouvelle interface",
    merged: "fusionné",
    restart_required: "fusionné, relance requise",
    failed: "échec",
    cancelled: "annulé",
    rolled_back: "retour à la version précédente",
  };

  /** Demande sur une ligne pour l'en-tête (les retours à la ligne deviennent des espaces). */
  export function oneLine(request: string, max = 120): string {
    const s = request.replace(/\s+/g, " ").trim();
    return s.length > max ? `${s.slice(0, max - 1)}…` : s;
  }

  /** Délai sans état final après « annuler » avant de le signaler. */
  export const CANCEL_TIMEOUT_MS = 15_000;

  export function isFinal(step: ImproveStep): boolean {
    return !RUNNING_STEPS.includes(step);
  }

  /** Position « n/7 » d'une étape en cours, `null` pour un état final. */
  export function stepIndex(step: ImproveStep): number | null {
    const i = RUNNING_STEPS.indexOf(step);
    return i < 0 ? null : i + 1;
  }

  /** Texte principal du bandeau pour un état donné. */
  export function bannerText(s: ImproveStatus, repoPath: string | null): string {
    const commit = s.commit ? ` ${s.commit.slice(0, 7)}` : "";
    switch (s.step) {
      case "merged":
        return `Amélioration fusionnée${commit} dans main du clone ; interface active.`;
      case "restart_required":
        return (
          `Relance requise : le cœur Rust a changé (commit${commit || " ?"} fusionné, non activé). ` +
          `Recompiler avec \`npm run tauri build\` dans le clone${repoPath ? ` (${repoPath})` : ""}, ` +
          `réinstaller puis relancer l'app : la session sera reprise.`
        );
      case "failed":
        return `Échec de l'amélioration${s.detail ? ` : ${s.detail}` : ""}. Branche ${s.branch} laissée non fusionnée.`;
      case "cancelled":
        return `Amélioration annulée. Branche ${s.branch} laissée non fusionnée.`;
      case "rolled_back":
        return `Retour à la version précédente de l'interface${commit ? ` (annule${commit})` : ""}.`;
      default:
        return `${STEP_LABEL[s.step]}${s.detail ? ` · ${s.detail}` : ""}`;
    }
  }
</script>

<script lang="ts">
  // Bandeau d'avancement d'une amélioration (J14, décision 17), au-dessus de la
  // conversation, dans le style du terminal : « ✻ /ameliorer … · étape n/7 ».
  // Il s'abonne lui-même à l'avancement (`subscribe` : `IMPROVE_EVENT` réel,
  // ou `MockSelfUpdate` en mode simulé) ; `initial` vient de `get_ui_info`
  // (l'événement global survit au rechargement, pas l'état de la page).
  import { onMount } from "svelte";

  let {
    initial = null,
    subscribe,
    repoPath = null,
    dismissed = null,
    oncancel,
    onrollback,
    ondismiss,
    onstatus,
  }: {
    initial?: ImproveStatus | null;
    subscribe: (cb: (s: ImproveStatus) => void) => Promise<() => void>;
    /** Clone du dépôt (message « relance requise »). */
    repoPath?: string | null;
    /** Id d'une amélioration dont le bandeau a été fermé. */
    dismissed?: string | null;
    oncancel: (id: string) => Promise<void>;
    onrollback: () => void;
    ondismiss: (id: string) => void;
    /** Chaque nouvel état reçu (l'app rafraîchit `get_ui_info`). */
    onstatus?: (s: ImproveStatus) => void;
  } = $props();

  let status = $state<ImproveStatus | null>(null);
  let cancelling = $state(false);
  let error = $state<string | null>(null);
  let cancelTimer: ReturnType<typeof setTimeout> | null = null;

  function stopCancelTimer(): void {
    if (cancelTimer !== null) clearTimeout(cancelTimer);
    cancelTimer = null;
  }

  // État initial (avant tout événement) ; un événement plus récent l'emporte.
  $effect(() => {
    const i = initial;
    if (i && (!status || (status.id === i.id ? i.t > status.t : i.t >= status.t))) status = i;
  });

  onMount(() => {
    let un: (() => void) | null = null;
    let gone = false;
    subscribe((s) => {
      status = s;
      if (isFinal(s.step)) {
        cancelling = false;
        stopCancelTimer();
      }
      onstatus?.(s);
    })
      .then((u) => {
        if (gone) u();
        else un = u;
      })
      .catch((e) => (error = `avancement indisponible : ${String(e)}`));
    return () => {
      gone = true;
      stopCancelTimer();
      un?.();
    };
  });

  const shown = $derived(status !== null && status.id !== dismissed);
  const final = $derived(status ? isFinal(status.step) : false);
  const n = $derived(status ? stepIndex(status.step) : null);
  const tone = $derived(
    !status
      ? ""
      : status.step === "failed"
        ? "err"
        : status.step === "restart_required"
          ? "warn"
          : status.step === "merged"
            ? "ok"
            : "",
  );

  async function cancel(): Promise<void> {
    if (!status || cancelling) return;
    cancelling = true;
    error = null;
    // Sans état final dans le délai : le bouton revient, avec une explication.
    stopCancelTimer();
    cancelTimer = setTimeout(() => {
      cancelTimer = null;
      if (!cancelling) return;
      cancelling = false;
      error = `annulation sans réponse après ${CANCEL_TIMEOUT_MS / 1000} s : le cycle continue peut-être, réessayer`;
    }, CANCEL_TIMEOUT_MS);
    try {
      await oncancel(status.id);
    } catch (e) {
      stopCancelTimer();
      cancelling = false;
      error = e && typeof e === "object" && "kind" in e ? JSON.stringify(e) : String(e);
    }
  }
</script>

{#if shown && status}
  <div
    class="improve term {tone}"
    data-testid="improve-banner"
    data-step={status.step}
    role="status"
    aria-live="polite"
  >
    <div class="head">
      <span class="glyph" class:spin={!final}>{final ? (status.step === "failed" ? "✗" : "✻") : "✻"}</span>
      <span class="req" title={status.request}>/ameliorer {oneLine(status.request)}</span>
      <span class="dim"> · {status.branch}</span>
      {#if n !== null}
        <span class="dim"> · étape {n}/{RUNNING_STEPS.length}</span>
      {/if}
      <span class="actions">
        {#if !final}
          <button data-testid="improve-cancel" disabled={cancelling} onclick={cancel}>
            {cancelling ? "annulation…" : "annuler"}
          </button>
        {:else}
          {#if status.step === "merged"}
            <button data-testid="improve-rollback" onclick={onrollback}>revenir à la version précédente</button>
          {/if}
          <button data-testid="improve-close" aria-label="Fermer" onclick={() => status && ondismiss(status.id)}>×</button>
        {/if}
      </span>
    </div>
    <div class="msg" data-testid="improve-text">⎿  {bannerText(status, repoPath)}</div>
    {#if error}
      <div class="msg err" data-testid="improve-error">⎿  {error}</div>
    {/if}
  </div>
{/if}

<style>
  .improve {
    flex: none;
    padding: 0 1ch;
    border-bottom: 1px solid #333;
    background: var(--t-bg);
  }
  .head {
    display: flex;
    align-items: baseline;
    white-space: pre;
    overflow: hidden;
  }
  .glyph {
    color: var(--t-spinner);
    width: 2ch;
    flex: none;
  }
  .glyph.spin {
    animation: pulse 1.2s ease-in-out infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }
  .req {
    color: var(--t-spinner-text);
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .actions {
    margin-left: auto;
    display: flex;
    gap: 1ch;
    padding-left: 2ch;
  }
  .msg {
    padding-left: 2ch;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    color: var(--t-dim);
  }
  .ok .glyph,
  .ok .msg {
    color: var(--t-ok);
  }
  .warn .glyph,
  .warn .msg {
    color: var(--warn);
  }
  .err .glyph,
  .err .msg,
  .msg.err {
    color: var(--t-error);
  }
  button {
    background: var(--t-bg);
    color: var(--t-fg);
    border: 1px solid #555;
    font: inherit;
    padding: 0 1ch;
    cursor: pointer;
  }
  button:disabled {
    opacity: 0.5;
    cursor: default;
  }
  button:focus-visible {
    outline: 1px solid var(--t-fg);
  }
</style>
