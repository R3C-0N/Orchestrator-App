<script lang="ts">
  // Notice de l'app dans le fil (niveau Warn ou Error) ; les bandeaux en tête
  // de la conversation restent ceux d'App (J6).
  import type { NoticeRow } from "../../state/session.svelte";

  let { row }: { row: NoticeRow } = $props();
</script>

<div class="l" class:err={row.level === "Error"} class:warn={row.level === "Warn"}>
  <span class="g">!</span><span class="x">{row.code} : {row.message}</span>
</div>

<style>
  .warn {
    color: #c19c00;
  }
</style>
