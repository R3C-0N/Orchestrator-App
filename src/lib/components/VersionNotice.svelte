<script lang="ts">
  // Bandeau « version non testée » (notice `unexpected_version`) avec une
  // croix : fermé, il reste masqué pour cette version de claude.exe, y
  // compris aux lancements suivants (`state/versionNotice.ts`) ; il revient
  // au prochain changement de version (spec, décision 12).
  import { dismissVersionNotice, isVersionNoticeDismissed } from "../state/versionNotice";

  let {
    message,
    version,
    onclose,
  }: { message: string; version: string | null; onclose?: () => void } = $props();

  /** Version fermée pendant cette exécution (vaut même si le stockage échoue). */
  let closed: string | null = $state(null);
  const hidden = $derived(version !== null && (closed === version || isVersionNoticeDismissed(version)));

  function close(): void {
    dismissVersionNotice(version);
    closed = version;
    // Le bouton disparaît : le focus revient à la saisie.
    onclose?.();
  }

  function onkeydown(e: KeyboardEvent): void {
    // Entrée et Espace ferment ici : le gestionnaire global renverrait
    // l'Espace vers la saisie avant que le bouton ne le reçoive.
    if (e.key !== "Enter" && e.key !== " ") return;
    // Avec un modificateur, la touche est ignorée : pas de fermeture, ni par
    // l'activation native du bouton (Maj+Entrée le cliquerait) ; elle
    // continue vers les raccourcis globaux.
    if (e.ctrlKey || e.altKey || e.metaKey || e.shiftKey) {
      e.preventDefault();
      return;
    }
    e.preventDefault();
    e.stopPropagation();
    close();
  }
</script>

{#if !hidden}
  <div class="banner warn" data-testid="version-notice">
    <span class="msg">{message}</span>
    <button
      type="button"
      class="close"
      data-testid="version-notice-close"
      aria-label="Fermer l'avertissement de version"
      title="Masquer pour cette version de claude.exe"
      onclick={close}
      {onkeydown}>✕</button
    >
  </div>
{/if}

<style>
  .banner {
    display: flex;
    gap: 1ch;
    padding: 0 1ch;
    color: #0c0c0c;
    background: var(--warn);
  }
  .msg {
    flex: 1;
  }
  .close {
    background: none;
    border: none;
    color: inherit;
    font: inherit;
    padding: 0;
    width: 2ch;
    cursor: pointer;
    opacity: 0.7;
  }
  .close:hover,
  .close:focus-visible {
    opacity: 1;
  }
  .close:focus-visible {
    outline: 1px solid #0c0c0c;
  }
</style>
