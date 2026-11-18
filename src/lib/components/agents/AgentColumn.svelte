<script lang="ts">
  // Colonne des agents (jalon J9, décision 4), restylée d'après la maquette
  // J15 : en-tête « ── agents ── », trois groupes repliables avec compteur
  // (« En attente d'action », « En cours », « Terminés »), un filet vertical
  // à gauche des agents de chaque groupe, et un pied « 2 en cours · 5 agents ».
  //
  // Groupes : agents en attente d'une réponse (question en suspens dans leur
  // fil), puis en cours, puis terminés (repliés par défaut). Ordre de
  // lancement dans les deux premiers, du plus récent au plus ancien dans
  // « Terminés ». Dans un groupe replié, un agent dont le fil est déroulé
  // reste affiché jusqu'à ce que son fil soit refermé.
  //
  // Lit seulement le magasin (`agents`, `questions`, `threads`, `threadsRev`,
  // `status`, `meta.cwd`) : aucune commande Tauri, aucune capability.
  //
  // Props :
  // - `store` (obligatoire) : le `SessionStore` de la session affichée.
  // - `width` : largeur de la colonne (CSS), `"430px"` par défaut.
  // - `expandable` : clic sur un agent → son fil déroulé (bonus), vrai par défaut.
  // - `open` (bindable) : `tool_use_id` des agents dépliés.
  // - `doneOpen` (bindable) : groupe « Terminés » ouvert, faux par défaut ;
  //   l'app le garde d'une session à l'autre tant qu'elle tourne.
  // - `waitingOpen`, `runningOpen` (bindables) : groupes « En attente » et
  //   « En cours » ouverts, vrais par défaut ; gardés par l'app de même, pour
  //   survivre au remontage de la colonne.
  //
  // Coût : l'horloge d'une seconde (`render/clock`) n'est lue que tant qu'un
  // agent tourne ; une colonne d'agents finis ne se redessine plus.
  import { seconds } from "../../render/clock.svelte";
  import { BULLET } from "../../render/glyphs";
  import type { AgentEvent } from "../../core/bindings/AgentEvent";
  import type { SessionStore } from "../../state/session.svelte";
  import AgentCard from "./AgentCard.svelte";
  import { WAITING_GLYPH, agentSummary, groupAgents, isLive, waitingIds } from "./agentView";

  let {
    store,
    width = "430px",
    expandable = true,
    open = $bindable([]),
    doneOpen = $bindable(false),
    waitingOpen = $bindable(true),
    runningOpen = $bindable(true),
  }: {
    store: SessionStore;
    width?: string;
    expandable?: boolean;
    open?: string[];
    doneOpen?: boolean;
    waitingOpen?: boolean;
    runningOpen?: boolean;
  } = $props();

  const agents = $derived(store.agents);
  const anyLive = $derived(agents.some(isLive));
  // `seconds.now` abonne à l'horloge ; `Date.now()` seul ne relance rien.
  const now = $derived(anyLive ? seconds.now : Date.now());
  const waiting = $derived(waitingIds(store.questions));
  const groups = $derived(groupAgents(agents, waiting));
  // « Terminés » : le plus récent en haut, comme la maquette.
  const over = $derived(groups.over.slice().reverse());
  /** Groupe replié : seuls ses agents dont le fil est déroulé restent affichés. */
  const shown = (list: AgentEvent[], isOpen: boolean) =>
    isOpen ? list : list.filter((a) => open.includes(a.tool_use_id));
  const summary = $derived(agentSummary(groups.waiting.length, groups.running.length, agents.length));

  function toggle(id: string) {
    open = open.includes(id) ? open.filter((x) => x !== id) : [...open, id];
  }

  // Focus clavier : un agent qui change de groupe change de `{#each}`, son
  // bouton est recréé et le focus perdu. Noté avant la mise à jour du DOM,
  // il est rendu après : à la carte si elle reste affichée, sinon à
  // l'en-tête du panneau. Rien si le focus est parti ailleurs.
  let column: HTMLElement | undefined = $state();
  let refocus: string | null = null;
  $effect.pre(() => {
    void groups;
    const el = document.activeElement;
    refocus =
      column && el instanceof HTMLElement && column.contains(el)
        ? (el.closest<HTMLElement>("[data-agent]")?.dataset.agent ?? null)
        : null;
  });
  $effect(() => {
    void groups;
    const id = refocus;
    refocus = null;
    if (!id || !column) return;
    const el = document.activeElement;
    if (el && el !== document.body) return;
    const target =
      column.querySelector<HTMLElement>(`[data-agent="${CSS.escape(id)}"] button.head`) ??
      column.querySelector<HTMLElement>('[data-testid="agents-done-toggle"]');
    target?.focus();
  });

  /** Replier un groupe referme aussi les fils de ses agents. */
  function closeThreads(list: AgentEvent[]) {
    const ids = new Set(list.map((a) => a.tool_use_id));
    if (open.some((id) => ids.has(id))) open = open.filter((id) => !ids.has(id));
  }
  function toggleDone() {
    doneOpen = !doneOpen;
    if (!doneOpen) closeThreads(groups.over);
  }
  function toggleWaiting() {
    waitingOpen = !waitingOpen;
    if (!waitingOpen) closeThreads(groups.waiting);
  }
  function toggleRunning() {
    runningOpen = !runningOpen;
    if (!runningOpen) closeThreads(groups.running);
  }

  /**
   * Entrée et Espace sur un en-tête de groupe : traités ici, car le
   * gestionnaire global de la fenêtre renverrait l'Espace vers la saisie
   * avant que le bouton ne le reçoive. Avec un modificateur, la touche est
   * ignorée (ni bascule, ni activation native du bouton) et continue vers
   * les raccourcis globaux.
   */
  function headKey(e: KeyboardEvent, toggle: () => void): void {
    if (e.key !== "Enter" && e.key !== " ") return;
    e.preventDefault();
    if (e.ctrlKey || e.altKey || e.metaKey || e.shiftKey) return;
    e.stopPropagation();
    toggle();
  }

  function threadOf(id: string) {
    // Relire à chaque lot qui touche un fil (les lignes sont mutées sur place).
    // Copie : un tableau neuf par révision, pour que le fil recalcule ses lignes.
    void store.threadsRev;
    return store.threads.get(id)?.slice() ?? null;
  }
</script>

<aside class="agents term" style:width aria-label="Agents" data-testid="agent-column" bind:this={column}>
  <div class="title">
    <span class="dim">── agents ──────────────</span>
    <span class="state dim" data-testid="agent-main-state">{store.status}</span>
  </div>
  {#snippet group(
    key: string,
    label: string,
    icon: string,
    iconClass: string,
    list: AgentEvent[],
    isOpen: boolean,
    onclick: () => void,
    empty: string,
    highlight: boolean,
  )}
    {@const visible = shown(list, isOpen)}
    <section class="group" data-group={key}>
      <button
        type="button"
        class="ghead"
        class:hl={highlight && list.length > 0}
        aria-expanded={isOpen}
        aria-controls="agents-{key}"
        data-testid="agents-{key}-toggle"
        {onclick}
        onkeydown={(e) => headKey(e, onclick)}
      >
        <svg class="chev" class:open={isOpen} width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
          <path d="M4.5 2.5L8 6l-3.5 3.5" />
        </svg>
        <span class={iconClass} aria-hidden="true">{icon}</span>
        <span class="glabel">{label}</span>
        <span class="count" data-testid="agents-{key}-count">{list.length}</span>
      </button>
      <div id="agents-{key}" data-testid="agents-{key}" data-open={isOpen}>
        {#if isOpen && list.length === 0}
          <div class="empty dim">{empty}</div>
        {/if}
        {#if visible.length > 0}
          <div class="tree">
            {#each visible as a (a.tool_use_id)}
              <AgentCard
                agent={a}
                {now}
                waiting={waiting.has(a.tool_use_id)}
                open={expandable && open.includes(a.tool_use_id)}
                thread={expandable && open.includes(a.tool_use_id) ? threadOf(a.tool_use_id) : null}
                cwd={store.meta.cwd}
                ontoggle={expandable ? toggle : undefined}
              />
            {/each}
          </div>
        {/if}
      </div>
    </section>
  {/snippet}
  <div class="groups">
    {@render group(
      "waiting",
      "En attente d'action",
      WAITING_GLYPH,
      "warn",
      groups.waiting,
      waitingOpen,
      toggleWaiting,
      "Aucun agent en attente",
      true,
    )}
    {@render group(
      "running",
      "En cours",
      BULLET,
      "grad",
      groups.running,
      runningOpen,
      toggleRunning,
      "Aucun agent en cours",
      false,
    )}
    {@render group("done", "Terminés", BULLET, "ok", over, doneOpen, toggleDone, "Aucun agent terminé", false)}
  </div>
  <div class="foot" data-testid="agent-count">{summary}</div>
</aside>

<style>
  .agents {
    flex: 0 1 auto;
    min-width: 300px;
    height: 100%;
    display: flex;
    flex-direction: column;
    border-left: 1px solid var(--rule);
    background: var(--bar);
    box-sizing: border-box;
    line-height: 1.45;
  }
  .title {
    flex: 0 0 auto;
    display: flex;
    justify-content: space-between;
    gap: 1ch;
    padding: 10px 16px 8px 16px;
    white-space: pre;
    overflow: hidden;
  }
  .groups {
    flex: 1 1 auto;
    min-height: 0;
    overflow-x: hidden;
    overflow-y: auto;
    padding: 0 6px 8px 10px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .group {
    display: flex;
    flex-direction: column;
  }
  .ghead {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    min-height: 30px;
    padding: 4px 8px 4px 2px;
    border: 0;
    border-radius: 4px;
    background: transparent;
    color: var(--strong);
    font: inherit;
    font-weight: 700;
    text-align: left;
    cursor: pointer;
  }
  .ghead.hl {
    background: var(--warn-bg);
  }
  .ghead:focus-visible {
    outline: 1px dotted var(--dim);
    outline-offset: -1px;
  }
  .chev {
    flex: 0 0 auto;
    fill: none;
    stroke: currentColor;
    stroke-width: 1.6;
    stroke-linecap: round;
    stroke-linejoin: round;
    transition: transform 0.15s;
  }
  .chev.open {
    transform: rotate(90deg);
  }
  .glabel {
    flex: 1 1 auto;
  }
  .count {
    min-width: 22px;
    padding: 0 6px;
    border-radius: 9px;
    background: var(--surface);
    color: var(--muted);
    font-weight: 500;
    font-size: 12px;
    text-align: center;
  }
  .empty {
    padding: 2px 0 6px 26px;
  }
  .tree {
    border-left: 1px solid var(--tree);
    margin: 2px 0 4px 8px;
    padding-left: 10px;
    display: flex;
    flex-direction: column;
  }
  .foot {
    flex: 0 0 auto;
    padding: 8px 16px;
    color: var(--fg);
    white-space: pre;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .dim {
    color: var(--dim);
  }
  .warn {
    color: var(--warn);
  }
  .ok {
    color: var(--ok);
  }
</style>
