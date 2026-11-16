<script lang="ts">
  // Ligne de l'app dans le fil : notice (niveau Warn ou Error ; les bandeaux
  // en tête de la conversation restent ceux d'App, J6), ou ligne grise sans
  // puce (`plain`, « ⎿  Début de l'historique omis (N lignes) »).
  import type { NoticeRow, PlainTextRow } from "../../state/session.svelte";

  let { row }: { row: NoticeRow | PlainTextRow } = $props();
</script>

{#if row.kind === "plain"}
  <div class="l plain" data-testid="plain-row"><span class="x">{`  ⎿  ${row.text}`}</span></div>
{:else}
  <div class="l" class:err={row.level === "Error"} class:warn={row.level === "Warn"}>
    <span class="g">!</span><span class="x">{row.code} : {row.message}</span>
  </div>
{/if}

<style>
  .warn {
    color: #c19c00;
  }
  .plain {
    color: var(--t-dim);
    white-space: pre;
  }
</style>
