<script lang="ts">
  // Barre de contrôle de l'onglet d'amélioration (J16), au-dessus de la
  // saisie, dans le style du terminal :
  //   ✻ tests (check, unitaires, e2e) · tentative 1/3      [Valider] [Annuler l'amélioration]
  // « Valider » n'est actif qu'en `awaiting_user` (contrôles et tests
  // relancés) ; « Annuler l'amélioration » jusqu'à la bascule. Fermer l'onglet
  // pendant un cycle remplace la barre par une confirmation dans la page
  // (jamais `confirm()`) : [Abandonner] [Continuer].
  // Le modèle (textes, droits) est dans `state/improve.ts`.
  import { tick } from "svelte";
  import type { ImproveStatus } from "../core/bindings/ImproveStatus";
  import {
    CANCEL_TIMEOUT_MS,
    bannerText,
    canCancel,
    canValidate,
    closeConfirmText,
    isFinal,
    oneLine,
  } from "../state/improve";

  let {
    status,
    repoPath = null,
    confirming = false,
    onvalidate,
    oncancel,
    onabandon,
    oncontinue,
  }: {
    status: ImproveStatus | null;
    /** Clone du dépôt (texte « relance requise »). */
    repoPath?: string | null;
    /** Confirmation de fermeture affichée à la place des boutons. */
    confirming?: boolean;
    onvalidate: () => Promise<void>;
    oncancel: () => Promise<void>;
    /** « Abandonner » de la confirmation : annule le cycle et ferme l'onglet. */
    onabandon: () => Promise<void>;
    /** « Continuer » (ou Échap) : la confirmation disparaît. */
    oncontinue: () => void;
  } = $props();

  let validating = $state(false);
  let cancelling = $state(false);
  let abandoning = $state(false);
  let error = $state<string | null>(null);
  let cancelTimer: ReturnType<typeof setTimeout> | null = null;
  let continueBtn: HTMLButtonElement | undefined = $state();

  const final = $derived(status ? isFinal(status.step) : false);
  const tone = $derived(
    !status
      ? ""
      : status.step === "failed"
        ? "err"
        : status.step === "restart_required" || status.step === "cancelled"
          ? "warn"
          : status.step === "merged"
            ? "ok"
            : status.step === "awaiting_user"
              ? "wait"
              : "",
  );
  const validateOn = $derived(canValidate(status) && !validating);
  const cancelOn = $derived(canCancel(status) && !cancelling);
  // La bascule a commencé pendant la confirmation : abandon impossible.
  const abandonOn = $derived(canCancel(status) && !abandoning);

  function stopCancelTimer(): void {
    if (cancelTimer !== null) clearTimeout(cancelTimer);
    cancelTimer = null;
  }

  // Un état final répond à « annuler » ; un nouvel état répond à « valider ».
  $effect(() => {
    const s = status;
    if (s && isFinal(s.step)) {
      cancelling = false;
      stopCancelTimer();
    }
    if (s && s.step !== "awaiting_user") validating = false;
  });

  $effect(() => () => stopCancelTimer());

  // Confirmation affichée : le focus va sur « Continuer » (le choix sans effet).
  $effect(() => {
    if (confirming) void tick().then(() => continueBtn?.focus());
  });

  function describe(e: unknown): string {
    if (e && typeof e === "object" && "kind" in e) {
      const ce = e as { kind: string; message?: string };
      return ce.message ? `${ce.kind} : ${ce.message}` : ce.kind;
    }
    return String(e);
  }

  async function validate(): Promise<void> {
    if (!validateOn) return;
    validating = true;
    error = null;
    try {
      await onvalidate();
    } catch (e) {
      validating = false;
      error = `Valider : ${describe(e)}`;
    }
  }

  async function cancel(): Promise<void> {
    if (!cancelOn) return;
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
      await oncancel();
    } catch (e) {
      stopCancelTimer();
      cancelling = false;
      error = `Annuler : ${describe(e)}`;
    }
  }

  async function abandon(): Promise<void> {
    if (!abandonOn) return;
    abandoning = true;
    error = null;
    try {
      await onabandon();
    } catch (e) {
      error = `Abandonner : ${describe(e)}`;
    } finally {
      abandoning = false;
    }
  }

  /**
   * Entrée et Espace agissent ici : le gestionnaire global renverrait la
   * frappe vers la saisie avant le bouton (comme `TabBar`).
   */
  function onkeydown(e: KeyboardEvent, run: () => void): void {
    if (e.key === "Escape" && confirming) {
      e.preventDefault();
      e.stopPropagation();
      oncontinue();
      return;
    }
    if (e.key !== "Enter" && e.key !== " ") return;
    e.preventDefault();
    if (e.ctrlKey || e.altKey || e.metaKey || e.shiftKey) return;
    e.stopPropagation();
    run();
  }
</script>

<div
  class="controls term {tone}"
  data-testid="improve-controls"
  data-step={status?.step ?? ""}
  data-attempt={status?.attempt ?? 0}
  role="status"
  aria-live="polite"
>
  {#if confirming && status}
    <div class="row" data-testid="improve-close-confirm" role="alertdialog" aria-label="Fermer l'onglet d'amélioration">
      <span class="glyph warn">!</span>
      <span class="text warnc">{closeConfirmText(status)}</span>
      <span class="actions">
        <button
          type="button"
          data-testid="improve-abandon"
          disabled={!abandonOn}
          title={canCancel(status) ? "Annuler l'amélioration et fermer l'onglet" : "Bascule en cours : abandon impossible"}
          onclick={() => void abandon()}
          onkeydown={(e) => onkeydown(e, () => void abandon())}>{abandoning ? "abandon…" : "Abandonner"}</button
        >
        <button
          type="button"
          data-testid="improve-continue"
          bind:this={continueBtn}
          onclick={oncontinue}
          onkeydown={(e) => onkeydown(e, oncontinue)}>Continuer</button
        >
      </span>
    </div>
  {:else}
    <div class="row">
      <span class="glyph" class:spin={!final && status?.step !== "awaiting_user"}
        >{status?.step === "failed" ? "✗" : "✻"}</span
      >
      <span class="text" data-testid="improve-status" title={status ? `/ameliorer ${oneLine(status.request, 400)}` : ""}
        >{status ? bannerText(status, repoPath) : "amélioration : état en cours de lecture"}{#if status && status.attempt > 0 && !final}<span
            class="dim"> · tentative {status.attempt}/{status.max_attempts}</span
          >{/if}</span
      >
      {#if status && !final}
        <span class="actions">
          <button
            type="button"
            data-testid="improve-validate"
            disabled={!validateOn}
            title={canValidate(status) ? "Relancer contrôles et tests" : "Actif quand l'amélioration attend l'utilisateur"}
            onclick={() => void validate()}
            onkeydown={(e) => onkeydown(e, () => void validate())}>{validating ? "validation…" : "Valider"}</button
          >
          <button
            type="button"
            data-testid="improve-cancel"
            disabled={!cancelOn}
            onclick={() => void cancel()}
            onkeydown={(e) => onkeydown(e, () => void cancel())}
            >{cancelling ? "annulation…" : "Annuler l'amélioration"}</button
          >
        </span>
      {/if}
    </div>
    {#if status}
      <div class="sub dim" data-testid="improve-branch">⎿  {status.branch}{status.commit ? ` · ${status.commit.slice(0, 7)}` : ""}</div>
    {/if}
  {/if}
  {#if error}
    <div class="sub err" data-testid="improve-error">⎿  {error}</div>
  {/if}
</div>

<style>
  .controls {
    flex: none;
    padding: 0 1ch;
    border-top: 1px solid var(--rule);
    background: var(--t-bg);
  }
  .row {
    display: flex;
    align-items: baseline;
    min-width: 0;
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
  .text {
    color: var(--t-spinner-text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .actions {
    margin-left: auto;
    display: flex;
    gap: 1ch;
    padding-left: 2ch;
    flex: none;
  }
  .sub {
    padding-left: 2ch;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .dim {
    color: var(--t-dim);
  }
  .ok .glyph,
  .ok .text {
    color: var(--t-ok);
  }
  .warn .glyph,
  .warn .text,
  .glyph.warn,
  .warnc {
    color: var(--warn);
  }
  .wait .glyph,
  .wait .text {
    color: var(--t-lavender);
  }
  .err .glyph,
  .err .text,
  .sub.err {
    color: var(--t-error);
  }
  button {
    background: var(--t-bg);
    color: var(--t-fg);
    border: 1px solid var(--tree);
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
