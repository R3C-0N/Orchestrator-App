<script lang="ts">
  // Appels Agent (`05`) : « ● N background agents launched (↓ to manage) »
  // (puce verte, nombre en gras), puis une sous-ligne par agent, `├` / `└`
  // gris en retrait de 3, nom sur fond de sa couleur (texte noir),
  // « (description) ».
  // Agent au premier plan (non capturé dans la référence) : « ● nom(description) »
  // et son état sous `⎿`.
  import { agentColor, agentName } from "../../render/colors";
  import { BULLET, TREE_END, TREE_MID } from "../../render/glyphs";
  import { seconds } from "../../render/tools";
  import type { AgentCallRow } from "../../state/session.svelte";
  import ToolResult from "./ToolResult.svelte";

  let { rows }: { rows: AgentCallRow[] } = $props();

  const bg = $derived(rows.filter((r) => r.agent.background));
  const fg = $derived(rows.filter((r) => !r.agent.background));
</script>

{#if bg.length > 0}
  <div class="l">
    <span class="g ok">{BULLET}</span>
    <span class="x"
      ><span class="b">{bg.length}</span> background {bg.length === 1 ? "agent" : "agents"} launched
      <span class="dim">(↓ to manage)</span></span
    >
  </div>
  {#each bg as r, i (r.id)}
    <div class="tree">
      <span class="g dim">{i === bg.length - 1 ? TREE_END : TREE_MID}</span>
      <span class="name" style:background={agentColor(r.agent.color, r.agent.agent_type)}
        >{agentName(r.agent.agent_type)}</span
      > <span>({r.agent.description})</span>
    </div>
  {/each}
{/if}
{#each fg as r, i (r.id)}
  {#if i > 0 || bg.length > 0}<div class="gap"></div>{/if}
  {@const a = r.agent}
  <div class="l">
    <span class="g {a.status === 'Failed' ? 'err' : a.status === 'Completed' ? 'ok' : 'dim'}">{BULLET}</span>
    <span class="x"
      ><span class="name" style:background={agentColor(a.color, a.agent_type)}>{agentName(a.agent_type)}</span
      >({a.description})</span
    >
  </div>
  {#if a.status === "Completed" || a.status === "Failed" || a.status === "Stopped"}
    <ToolResult
      lines={[
        `${a.status === "Completed" ? "Done" : a.status}${a.started_at && a.ended_at ? ` · ${seconds(a.ended_at - a.started_at)}` : ""}`,
      ]}
      tone={a.status === "Failed" ? "err" : "dim"}
    />
  {:else}
    <ToolResult lines={[a.last_tool ? `Running ${a.last_tool}…` : "Running…"]} tone="dim" />
  {/if}
{/each}

<style>
  .tree {
    padding-left: 3ch;
    white-space: pre-wrap;
  }
  .name {
    color: #000000;
  }
  .gap {
    height: var(--t-lh);
  }
</style>
