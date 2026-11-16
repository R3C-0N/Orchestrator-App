<script lang="ts">
  // Réponse de l'assistant (`02`) : puce « ● » blanche en colonne 0, texte
  // markdown en retrait de 2.
  //
  // Pendant le streaming, le markdown est réanalysé à chaque mise à jour tant
  // que l'analyse est rapide ; au-delà de 4 ms, au plus 10 fois par seconde,
  // et le texte arrivé entre deux analyses s'affiche brut à la suite
  // (plan 1.7) : chaque token est visible dès son image.
  import { markdownCached, renderMarkdown } from "../../render/markdown";
  import { BULLET } from "../../render/glyphs";
  import { clock12 } from "../../render/tools";
  import type { AssistantTextRow } from "../../state/session.svelte";

  let { row, verbose = false, model = null }: { row: AssistantTextRow; verbose?: boolean; model?: string | null } =
    $props();

  let html = $state("");
  let parsedLen = $state(0);
  let lastAt = 0;
  let cost = 0;
  let timer: ReturnType<typeof setTimeout> | null = null;

  function parse(text: string) {
    const t0 = performance.now();
    html = renderMarkdown(text);
    cost = performance.now() - t0;
    lastAt = performance.now();
    parsedLen = text.length;
  }

  $effect.pre(() => {
    const text = row.text;
    if (row.done) {
      if (timer) clearTimeout(timer);
      timer = null;
      html = markdownCached(text);
      parsedLen = text.length;
      return;
    }
    if (cost < 4 || performance.now() - lastAt >= 100) {
      parse(text);
    } else if (!timer) {
      timer = setTimeout(() => {
        timer = null;
        if (!row.done) parse(row.text);
      }, 100);
    }
  });

  $effect(() => () => {
    if (timer) clearTimeout(timer);
  });

  const tail = $derived(row.text.length > parsedLen ? row.text.slice(parsedLen) : "");
</script>

{#if verbose}
  <div class="stamp dim">{clock12(row.at)}{model ? ` ${model}` : ""}</div>
{/if}
<div class="l">
  <span class="g white">{BULLET}</span>
  <!-- Sans espace entre les balises : `.x` garde les espaces (pre-wrap). -->
  <div class="x"><div class="md">{@html html}</div>{#if tail}<span class="tail">{tail}</span>{/if}</div>
</div>

<style>
  /* Heure et modèle alignés à droite avant la réponse (vue détaillée, `03b`). */
  .stamp {
    text-align: right;
    padding-right: 7ch;
  }
  .tail {
    white-space: pre-wrap;
  }
</style>
