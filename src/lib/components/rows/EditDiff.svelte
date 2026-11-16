<script lang="ts">
  // Diff d'un Edit (`04`) : « ⎿  Added 3 lines, removed 2 lines » (nombres
  // en gras), puis les lignes : numéro aligné à droite, espace, signe, texte ;
  // suppressions sur fond rouge sombre (ancien fichier), ajouts sur fond vert
  // sombre (nouveau fichier), contexte en coloration syntaxique.
  import { diffForRow, diffSummaryHtml } from "../../render/diff";
  import type { ToolCallRow } from "../../state/session.svelte";
  import ToolResult from "./ToolResult.svelte";

  let { row }: { row: ToolCallRow } = $props();

  const path = $derived.by(() => {
    const inp = row.input as Record<string, unknown> | null;
    return inp && typeof inp.file_path === "string" ? inp.file_path : null;
  });
  const diff = $derived(diffForRow(row, path));
</script>

<ToolResult lines={[diffSummaryHtml(diff.added, diff.removed)]} html />
<div class="diff hl-diff">
  {#each diff.lines as l, i (i)}
    {#if l.kind === "gap"}
      <div class="dl gap"><span class="no">{"".padStart(diff.gutter)}</span> <span class="dim">…</span></div>
    {:else}
      <div class="dl {l.kind === '-' ? 'del' : l.kind === '+' ? 'add' : 'ctx'}"><span class="no"
          >{String(l.no).padStart(diff.gutter)}</span
        > <span class="sg">{l.kind}</span><span class="tx">{@html l.html}</span></div>
    {/if}
  {/each}
</div>

<style>
  .diff {
    /* Le fond des lignes part de la colonne 5 et s'arrête 7 colonnes avant le bord (`04`). */
    margin-left: 5ch;
    margin-right: 7ch;
  }
  .dl {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .no {
    color: #7c7c79;
  }
  .ctx .sg {
    color: transparent;
  }
  .del {
    background: #3d0100;
  }
  .del .no,
  .del .sg {
    color: #dc5a5a;
  }
  .del .tx {
    color: #f8f8f2;
  }
  .add {
    background: #022800;
  }
  .add .no,
  .add .sg {
    color: #50c850;
  }
  /* Mots changés, sur un fond plus soutenu (`04` : « 60 », « 5 »). */
  .del :global(.dw) {
    background: #5c0200;
  }
  .add :global(.dw) {
    background: #044700;
  }
</style>
