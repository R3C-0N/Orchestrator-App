<script lang="ts">
  // Fil déroulé d'un sous-agent (bonus J9), sous sa ligne dans la colonne.
  // Même grammaire que le fil principal, en compact : consigne `❯` grise,
  // texte `●` blanc, appel `● Read(a.txt)` (puce verte, rouge en erreur)
  // puis `⎿  résultat` gris. Pas de markdown ni de diff : le détail complet
  // reste l'affaire de la conversation.
  import { BULLET, ELBOW, PROMPT } from "../../render/glyphs";
  import type { Row } from "../../state/session.svelte";
  import { threadLines } from "./agentView";

  let {
    rows,
    cwd = null,
    gutter = "│",
  }: {
    rows: readonly Row[];
    cwd?: string | null;
    /** Trait de l'arbre à gauche (`│`, ou espace sous le dernier agent). */
    gutter?: string;
  } = $props();

  const lines = $derived(threadLines(rows, cwd));
</script>

<div class="thread" data-testid="agent-thread">
  {#if lines.length === 0}
    <div class="l"><span class="g dim">{gutter}</span><span class="x dim">(fil vide)</span></div>
  {/if}
  {#each lines as l (l.key)}
    {#if l.kind === "prompt"}
      <div class="l"><span class="g dim">{gutter}</span><span class="b dim">{PROMPT}</span><span class="x dim">{l.text}</span></div>
    {:else if l.kind === "text"}
      <div class="l">
        <span class="g dim">{gutter}</span><span class="b">{BULLET}</span><span class="x white">{l.text}{l.done ? "" : "…"}</span>
      </div>
    {:else}
      <div class="l">
        <span class="g dim">{gutter}</span><span class="b {l.isError ? 'err' : l.result === null ? 'dim' : 'ok'}">{BULLET}</span><span
          class="x"><span class="t">{l.title}</span>{l.arg ? `(${l.arg})` : ""}</span
        >
      </div>
      {#if l.result !== null}
        <div class="l">
          <span class="g dim">{gutter}</span><span class="b"></span><span class="e dim">{ELBOW}</span><span
            class="x {l.isError ? 'err' : 'dim'}">{l.result}</span
          >
        </div>
      {/if}
    {/if}
  {/each}
</div>

<style>
  .l {
    display: flex;
    min-width: 0;
  }
  .g,
  .b {
    flex: 0 0 2ch;
    white-space: pre;
  }
  .g {
    /* Aligné sous le glyphe d'état de la ligne d'agent. */
    flex-basis: 4ch;
  }
  .e {
    flex: 0 0 3ch;
  }
  .x {
    flex: 1 1 auto;
    min-width: 0;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .t {
    font-weight: bold;
    color: var(--t-bold);
  }
  .white {
    color: var(--t-white);
  }
  .dim {
    color: var(--t-dim);
  }
  .ok {
    color: var(--t-ok);
  }
  .err {
    color: var(--t-error);
  }
</style>
