<script lang="ts">
  // Bouton Clair/Sombre de la barre d'état (maquette « Orchestrator », J15),
  // posé sur le segment en dégradé du p95 : fond sombre translucide, texte
  // de la couleur du segment.
  // Il montre le thème vers lequel il bascule : en sombre, soleil et
  // « Clair » ; en clair, lune et « Sombre ». L'état et sa mémoire sont dans
  // `state/theme.svelte.ts`.
  import { currentTheme, toggleTheme } from "../state/theme.svelte";

  const light = $derived(currentTheme() === "light");
  const label = $derived(light ? "Passer au thème sombre" : "Passer au thème clair");

  function onkeydown(e: KeyboardEvent): void {
    // Entrée et Espace basculent ici : le gestionnaire global de la fenêtre
    // renverrait l'Espace vers la saisie avant que le bouton ne le reçoive.
    if (e.key !== "Enter" && e.key !== " ") return;
    // Avec un modificateur, la touche est ignorée : pas de bascule, ni par
    // l'activation native du bouton (Maj+Entrée le cliquerait) ; elle
    // continue vers les raccourcis globaux.
    e.preventDefault();
    if (e.ctrlKey || e.altKey || e.metaKey || e.shiftKey) return;
    e.stopPropagation();
    toggleTheme();
  }
</script>

<span class="theme">
  <button type="button" data-testid="theme-toggle" aria-label={label} title={label} onclick={() => toggleTheme()}
    {onkeydown}>
    {#if light}
      <svg width="12" height="12" viewBox="0 0 16 16" aria-hidden="true" class="moon">
        <path d="M13.5 9.8A5.8 5.8 0 0 1 6.2 2.5a5.8 5.8 0 1 0 7.3 7.3z" />
      </svg>
      <span>Sombre</span>
    {:else}
      <svg width="12" height="12" viewBox="0 0 16 16" aria-hidden="true">
        <circle cx="8" cy="8" r="3" />
        <path d="M8 1v1.6M8 13.4V15M1 8h1.6M13.4 8H15M3 3l1.1 1.1M11.9 11.9L13 13M3 13l1.1-1.1M11.9 4.1L13 3" />
      </svg>
      <span>Clair</span>
    {/if}
  </button>
</span>

<style>
  .theme {
    display: flex;
    align-items: center;
  }
  button {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 18px;
    padding: 0 8px;
    border: 1px solid rgba(0, 0, 0, 0.22);
    border-radius: 4px;
    background: rgba(0, 0, 0, 0.14);
    color: var(--on-accent);
    font: inherit;
    font-size: 11px;
    font-weight: 700;
    cursor: pointer;
  }
  svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.5;
    stroke-linecap: round;
  }
  /* La lune de la maquette : angles arrondis, pas de bouts arrondis. */
  svg.moon {
    stroke-linecap: butt;
    stroke-linejoin: round;
  }
</style>
