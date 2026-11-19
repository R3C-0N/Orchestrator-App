<script lang="ts">
  // Barre des onglets, fine (même hauteur que la barre du bas), sous la barre
  // de titre : un onglet par conversation ouverte (un orchestrateur chacune),
  // puis « + » qui ouvre un onglet sur le sélecteur des projets. Un point
  // signale un tour en cours (accent) ou une question en attente (lavande)
  // dans un onglet. La croix ferme l'onglet (et sa session) ; absente quand il
  // n'en reste qu'un. J16 : l'onglet d'amélioration porte un badge d'étape
  // (« tour 1 », « tests · 1/3 », « en attente », « fusionné »…) ; sa croix
  // passe par l'app (confirmation dans la page pendant un cycle).
  import type { TabView } from "../state/tabs";

  let {
    tabs,
    active,
    onselect,
    onnew,
    onclose,
  }: {
    tabs: TabView[];
    active: number;
    onselect: (id: number) => void;
    onnew: () => void;
    onclose: (id: number) => void;
  } = $props();

  /**
   * Entrée et Espace agissent ici : le gestionnaire global renverrait la
   * frappe vers la saisie avant le bouton (comme `TitleBar`).
   */
  function onkeydown(e: KeyboardEvent, run: () => void): void {
    if (e.key !== "Enter" && e.key !== " ") return;
    e.preventDefault();
    if (e.ctrlKey || e.altKey || e.metaKey || e.shiftKey) return;
    e.stopPropagation();
    run();
  }
</script>

<nav class="tab-bar" data-testid="tab-bar" aria-label="Conversations">
  <div class="tabs" role="tablist">
    {#each tabs as t (t.id)}
      <div
        class="tab"
        class:active={t.id === active}
        class:improve={!!t.improve}
        data-testid="tab"
        data-kind={t.improve ? "improve" : "conversation"}
        data-step={t.improve?.step ?? null}
      >
        <button
          type="button"
          class="pick"
          role="tab"
          aria-selected={t.id === active}
          title={t.title}
          onclick={() => onselect(t.id)}
          onkeydown={(e) => onkeydown(e, () => onselect(t.id))}
        >
          {#if t.waiting}
            <span class="dot waiting" aria-label="question en attente"></span>
          {:else if t.busy}
            <span class="dot busy" aria-label="tour en cours"></span>
          {/if}
          <span class="label">{t.label}</span>
          {#if t.improve?.badge}
            <span class="badge {t.improve.badge.tone}" data-testid="tab-badge">{t.improve.badge.text}</span>
          {/if}
        </button>
        {#if tabs.length > 1 || t.improve}
          <button
            type="button"
            class="close"
            aria-label="Fermer l'onglet {t.label}"
            title="Fermer l'onglet"
            data-testid="tab-close"
            onclick={() => onclose(t.id)}
            onkeydown={(e) => onkeydown(e, () => onclose(t.id))}
          >
            <svg width="8" height="8" viewBox="0 0 8 8" aria-hidden="true"><path d="M0.5 0.5l7 7M7.5 0.5l-7 7" /></svg>
          </button>
        {/if}
      </div>
    {/each}
  </div>
  <button
    type="button"
    class="new"
    aria-label="Nouvel onglet"
    title="Nouvel onglet"
    data-testid="tab-new"
    onclick={onnew}
    onkeydown={(e) => onkeydown(e, onnew)}>+</button
  >
</nav>

<style>
  .tab-bar {
    flex: 0 0 auto;
    display: flex;
    align-items: stretch;
    height: 24px;
    font-size: 12px;
    background: var(--bar);
    border-top: 1px solid var(--rule);
    white-space: nowrap;
    user-select: none;
    min-width: 0;
  }
  .tabs {
    display: flex;
    min-width: 0;
    overflow: hidden;
  }
  .tab {
    display: flex;
    align-items: stretch;
    min-width: 0;
    max-width: 220px;
    border-right: 1px solid var(--rule);
    color: var(--muted);
  }
  .tab.active {
    background: var(--surface);
    color: var(--fg);
    box-shadow: inset 0 -2px 0 var(--accent);
  }
  button {
    border: 0;
    padding: 0;
    background: transparent;
    color: inherit;
    font: inherit;
    cursor: pointer;
  }
  button:focus-visible {
    outline: 1px solid var(--accent);
    outline-offset: -2px;
  }
  .pick {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    padding: 0 10px 0 12px;
  }
  .tab:not(.active) .pick:hover {
    color: var(--fg);
  }
  .label {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  /* Onglet d'amélioration (J16) : plus large pour le badge d'étape. */
  .tab.improve {
    max-width: 340px;
  }
  .badge {
    flex: 0 0 auto;
    padding: 0 5px;
    border: 1px solid currentColor;
    border-radius: 2px;
    font-size: 11px;
    line-height: 14px;
    color: var(--muted);
  }
  .badge.run {
    color: var(--accent);
  }
  .badge.wait {
    color: var(--t-lavender);
  }
  .badge.ok {
    color: var(--t-ok);
  }
  .badge.warn {
    color: var(--warn);
  }
  .badge.err {
    color: var(--danger);
  }
  .dot {
    width: 6px;
    height: 6px;
    flex: 0 0 auto;
    border-radius: 50%;
  }
  .dot.busy {
    background: var(--accent);
  }
  .dot.waiting {
    background: var(--t-lavender);
  }
  .close {
    width: 22px;
    flex: 0 0 auto;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--dim);
  }
  .close:hover {
    color: var(--fg);
    background: var(--rule);
  }
  .close svg {
    stroke: currentColor;
    stroke-width: 1.2;
  }
  .new {
    flex: 0 0 auto;
    width: 28px;
    font-size: 15px;
    line-height: 1;
    color: var(--muted);
  }
  .new:hover {
    color: var(--fg);
    background: var(--surface);
  }
</style>
