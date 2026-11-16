<script lang="ts">
  // Fin d'un agent d'arrière-plan (`06`) : « ● Agent "<description>"
  // finished · 42s » (puce verte, durée grise), sans « Done (N tool uses…) » ;
  // puis « › Message from @<agent> (ctrl+o to expand) » en gris, déplié en
  // vue détaillée (le résumé renvoyé par l'agent).
  import { agentName } from "../../render/colors";
  import { BULLET, MESSAGE_FROM } from "../../render/glyphs";
  import { seconds } from "../../render/tools";
  import type { AgentDoneRow } from "../../state/session.svelte";

  let { row, verbose = false }: { row: AgentDoneRow; verbose?: boolean } = $props();

  const a = $derived(row.agent);
  const verb = $derived(a.status === "Completed" ? "finished" : a.status === "Failed" ? "failed" : "stopped");
  const dur = $derived(a.started_at !== null ? seconds((a.ended_at ?? row.at) - a.started_at) : null);
</script>

<div class="l">
  <span class="g {a.status === 'Failed' ? 'err' : 'ok'}">{BULLET}</span>
  <span class="x">Agent "{a.description}" {verb}{#if dur}<span class="dim">{` · ${dur}`}</span>{/if}</span>
</div>
<div class="gap"></div>
<div class="l dim">
  <span class="g">{MESSAGE_FROM}</span>
  <span class="x"
    >Message from @{agentName(a.agent_type)}{#if !verbose}&nbsp;(ctrl+o to expand){/if}</span
  >
</div>
{#if verbose && a.summary}
  <div class="l"><span></span><span class="x">{a.summary}</span></div>
{/if}

<style>
  .gap {
    height: var(--t-lh);
  }
</style>
