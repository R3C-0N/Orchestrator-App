<script lang="ts">
  // Diff d'un Edit (`04`) : « ⎿  Added 3 lines, removed 2 lines » (nombres
  // en gras), puis les lignes : numéro aligné à droite, espace, signe, texte ;
  // suppressions sur fond rouge sombre (ancien fichier), ajouts sur fond vert
  // sombre (nouveau fichier), contexte en coloration syntaxique. Couleurs
  // en variables (`--diff-*`, `--hl-diff-fg` dans app.css) : le thème clair
  // les remplace par des fonds pâles.
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
          >{(l.no === null ? "" : String(l.no)).padStart(diff.gutter)}</span
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
    color: var(--diff-no);
  }
  .ctx .sg {
    color: transparent;
  }
  .del {
    background: var(--diff-del-bg);
  }
  .del .no,
  .del .sg {
    color: var(--diff-del-fg);
  }
  .del .tx {
    color: var(--hl-diff-fg);
  }
  .add {
    background: var(--diff-add-bg);
  }
  .add .no,
  .add .sg {
    color: var(--diff-add-fg);
  }
  /* Mots changés, sur un fond plus soutenu (`04` : « 60 », « 5 »). */
  .del :global(.dw) {
    background: var(--diff-del-word);
  }
  .add :global(.dw) {
    background: var(--diff-add-word);
  }
</style>
