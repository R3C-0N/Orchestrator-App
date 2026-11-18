<script lang="ts">
  // Fin de tour (`02`, `06`) : « ✻ <Verbe> for <durée> · done <HH:MM> » en
  // gris ; tour fini avec des agents d'arrière-plan en cours :
  // « ✻ Waiting for N background agents to finish » (`05`).
  import { DONE_VERBS, TURN_DONE, pickStable } from "../../render/glyphs";
  import { hhmm, seconds } from "../../render/tools";
  import type { TurnResultRow } from "../../state/session.svelte";

  let { row }: { row: TurnResultRow } = $props();
  const verb = $derived(pickStable(row.id, DONE_VERBS));
</script>

<div class="l dim">
  <span class="g grad">{TURN_DONE}</span>
  {#if row.waiting > 0}
    <span class="x"
      >Waiting for <span class="b">{row.waiting}</span> background {row.waiting === 1 ? "agent" : "agents"} to finish</span
    >
  {:else}
    <span class="x">{verb} for {seconds(row.durationMs)} · done {hhmm(row.at)}</span>
  {/if}
</div>
