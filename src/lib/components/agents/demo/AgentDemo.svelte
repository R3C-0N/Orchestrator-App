<script lang="ts">
  // Page de démonstration de la colonne des agents (J9) : la conversation à
  // gauche, la colonne à droite (décision 4), comme l'intégration prévue
  // dans `App.svelte`. Aucune commande Tauri : mode simulé seulement.
  import { onMount } from "svelte";
  import t3 from "../../../../../fixtures/ui-events/t3.json";
  import type { CoreEvent } from "../../../core/bindings/CoreEvent";
  import { EventFactory, buildHistory } from "../../../core/mock";
  import { SessionStore } from "../../../state/session.svelte";
  import Conversation from "../../Conversation.svelte";
  import AgentColumn from "../AgentColumn.svelte";
  import { fourAgents, play, retime, sortAgents } from "./replay";

  const params = new URLSearchParams(location.search);
  const scenario = params.get("mock") ?? "t3";
  const stretch = Number(params.get("stretch") ?? "20") || 20;
  const width = params.get("width") ?? "48ch";
  /** Échelle de temps du scénario `four`. */
  const scale = Number(params.get("scale") ?? "1") || 1;
  /** Historique synthétique avant le scénario (mesures de défilement). */
  const rows = Number(params.get("rows") ?? "0") || 0;

  const store = new SessionStore();
  let open: string[] = $state([]);
  let doneOpen = $state(false);
  let done = $state(false);

  // `seq` renuméroté à l'envoi : historique et scénario forment une seule suite.
  let seq = 0;
  const push = (batch: CoreEvent[]) => store.push(batch.map((e) => ({ ...e, seq: seq++ }) as CoreEvent));

  onMount(() => {
    if (rows > 0) {
      const { events } = buildHistory(new EventFactory(), rows);
      for (let i = 0; i < events.length; i += 256) store.apply(events.slice(i, i + 256).map((e) => ({ ...e, seq: seq++ })));
    }
    const base = Date.now();
    const timed =
      scenario === "t3"
        ? retime(t3 as unknown as CoreEvent[], base, stretch)
        : scenario === "four"
          ? fourAgents(base, scale)
          : scenario === "sort"
            ? sortAgents(base, scale)
            : [];
    const stop = play(timed, push, () => (done = true));
    (window as unknown as Record<string, unknown>).__agentsDemo = {
      store,
      scenario,
      /** Applique des événements tout de suite (tests), à la suite de la numérotation. */
      apply: (events: CoreEvent[]) => store.apply(events.map((e) => ({ ...e, seq: seq++ }) as CoreEvent)),
      done: () => done,
      open: () => open,
      doneOpen: () => doneOpen,
    };
    return stop;
  });
</script>

<main data-scenario={scenario} data-done={done}>
  <div class="left"><Conversation {store} /></div>
  <AgentColumn {store} {width} bind:open bind:doneOpen />
</main>

<style>
  main {
    display: flex;
    height: 100%;
    background: var(--t-bg);
  }
  .left {
    flex: 1 1 auto;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
</style>
