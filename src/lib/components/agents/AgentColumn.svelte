<script lang="ts">
  // Colonne des agents (jalon J9, décision 4) : à droite de la conversation,
  // en style terminal, d'après le panneau d'agents du TUI 2.1.325 (`05`) :
  //
  //   ● main                                     Streaming
  //   ├ ◯ agent-bleu  Ping bleu agent
  //   │   ⎿  Read…                 6s · ↓ 38.9k tokens
  //   └ ● agent-rouge  Dire bonjour
  //       ⎿  Done · 0 tool uses    0s · ↓ 24.0k tokens
  //
  // Lit seulement le magasin (`agents`, `threads`, `threadsRev`, `status`,
  // `meta.cwd`) : aucune commande Tauri, aucune capability.
  //
  // Props :
  // - `store` (obligatoire) : le `SessionStore` de la session affichée.
  // - `width` : largeur de la colonne (CSS), `"48ch"` par défaut.
  // - `expandable` : clic sur un agent → son fil déroulé (bonus), vrai par défaut.
  // - `open` (bindable) : `tool_use_id` des agents dépliés.
  //
  // Coût : l'horloge d'une seconde (`render/clock`) n'est lue que tant qu'un
  // agent tourne ; une colonne d'agents finis ne se redessine plus.
  import { seconds } from "../../render/clock.svelte";
  import { BULLET } from "../../render/glyphs";
  import type { SessionStore } from "../../state/session.svelte";
  import AgentCard from "./AgentCard.svelte";
  import { isLive } from "./agentView";

  let {
    store,
    width = "48ch",
    expandable = true,
    open = $bindable([]),
  }: {
    store: SessionStore;
    width?: string;
    expandable?: boolean;
    open?: string[];
  } = $props();

  const agents = $derived(store.agents);
  const anyLive = $derived(agents.some(isLive));
  // `seconds.now` abonne à l'horloge ; `Date.now()` seul ne relance rien.
  const now = $derived(anyLive ? seconds.now : Date.now());
  const running = $derived(agents.filter(isLive).length);

  function toggle(id: string) {
    open = open.includes(id) ? open.filter((x) => x !== id) : [...open, id];
  }

  function threadOf(id: string) {
    // Relire à chaque lot qui touche un fil (les lignes sont mutées sur place).
    // Copie : un tableau neuf par révision, pour que le fil recalcule ses lignes.
    void store.threadsRev;
    return store.threads.get(id)?.slice() ?? null;
  }
</script>

<aside class="agents term" style:width aria-label="Agents" data-testid="agent-column">
  <div class="main">
    <span class="g">{BULLET}</span>
    <span class="b">main</span>
    <span class="state dim" data-testid="agent-main-state">{store.status}</span>
  </div>
  {#if agents.length === 0}
    <div class="empty dim">  no agents</div>
  {:else}
    {#each agents as a, i (a.tool_use_id)}
      <AgentCard
        agent={a}
        {now}
        last={i === agents.length - 1}
        open={expandable && open.includes(a.tool_use_id)}
        thread={expandable && open.includes(a.tool_use_id) ? threadOf(a.tool_use_id) : null}
        cwd={store.meta.cwd}
        ontoggle={expandable ? toggle : undefined}
      />
    {/each}
    <div class="foot dim" data-testid="agent-count">
      {running > 0 ? `${running} running · ` : ""}{agents.length} {agents.length === 1 ? "agent" : "agents"}
    </div>
  {/if}
</aside>

<style>
  .agents {
    flex: 0 0 auto;
    height: 100%;
    overflow-x: hidden;
    overflow-y: auto;
    padding: 0 1ch;
    border-left: 1px solid #3a3a3a;
    box-sizing: border-box;
  }
  .main {
    display: flex;
    white-space: pre;
  }
  .g {
    flex: 0 0 2ch;
    color: var(--t-white);
  }
  .b {
    flex: 1 1 auto;
    font-weight: bold;
    color: var(--t-bold);
  }
  .state {
    flex: 0 0 auto;
  }
  .empty {
    white-space: pre;
  }
  .foot {
    margin-top: var(--t-lh);
    white-space: pre;
  }
  .dim {
    color: var(--t-dim);
  }
</style>
