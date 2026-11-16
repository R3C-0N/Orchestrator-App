<script lang="ts">
  // Appels regroupés en vue normale : une ligne grise en retrait de 2, sans
  // puce ni résultat (« Read 1 file, ran 1 shell command », `03`, `04`, `09`).
  // Appel en cours : puce grise, libellé et durée vivante, puis
  // « ⎿  $ <commande> » (`k1`). Appels coupés par une interruption :
  // « No result recorded for N call » et « ⎿  $ <commande> » (`08`).
  import { BULLET } from "../../render/glyphs";
  import { seconds as clock } from "../../render/clock.svelte";
  import { groupLabel } from "../../render/group";
  import { bashCommand, runningLabel, seconds } from "../../render/tools";
  import type { ToolCallRow } from "../../state/session.svelte";
  import ToolResult from "./ToolResult.svelte";

  let { rows, noResult = false }: { rows: ToolCallRow[]; noResult?: boolean } = $props();

  const done = $derived(rows.filter((r) => r.result !== null));
  const running = $derived(noResult ? [] : rows.filter((r) => r.result === null));
  const commands = $derived(rows.map(bashCommand).filter((c): c is string => c !== null));
</script>

{#if noResult}
  <div class="l"><span></span><span class="x dim">No result recorded for {rows.length} {rows.length === 1 ? "call" : "calls"}</span></div>
  {#each commands as c, i (i)}
    <ToolResult lines={[`$ ${c}`]} tone="dim" />
  {/each}
{:else}
  {#if done.length > 0}
    <div class="l"><span></span><span class="x dim">{groupLabel(done)}</span></div>
  {/if}
  {#each running as r (r.id)}
    <div class="l">
      <span class="g dim">{BULLET}</span>
      <span class="x">{runningLabel(r)} <span class="dim">· {seconds(clock.now - r.at)}</span></span>
    </div>
    {#if bashCommand(r) !== null}
      <ToolResult lines={[`$ ${bashCommand(r)}`]} tone="dim" />
    {/if}
  {/each}
{/if}
