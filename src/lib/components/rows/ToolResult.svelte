<script lang="ts">
  // Lignes de résultat : « ⎿  » (2 espaces, ⎿, 2 espaces) en gris, puis le
  // texte ; les lignes suivantes en retrait de 5 sous le texte (`03b`, `09b`).
  import { ELBOW } from "../../render/glyphs";

  let {
    lines,
    tone = "",
    more = 0,
    hint = true,
    html = false,
  }: {
    /** Lignes de texte (ou de HTML déjà échappé si `html`). */
    lines: string[];
    /** Classe de couleur : "", "dim", "err". */
    tone?: string;
    /** Lignes masquées (vue normale) : « … +N lines (ctrl+o to expand) ». */
    more?: number;
    /** « (ctrl+o to expand) » après « … +N lines » ; absent sous un Write (`w1`). */
    hint?: boolean;
    html?: boolean;
  } = $props();
</script>

<div class="r">
  <span class="g">{ELBOW}</span>
  <span class="x {tone}">{#if html}{@html lines[0] ?? ""}{:else}{lines[0] ?? ""}{/if}</span>
</div>
{#if lines.length > 1}
  <div class="rc x {tone}">{#if html}{@html lines.slice(1).join("\n")}{:else}{lines.slice(1).join("\n")}{/if}</div>
{/if}
{#if more > 0}
  <div class="rc x dim">… +{more} lines{#if hint}{" (ctrl+o to expand)"}{/if}</div>
{/if}

<style>
  /* Numéro de ligne d'un Write (`w1`, `w1b`) : gris foncé #666666, plus
     sombre que le gris des éléments secondaires (#999999). */
  .rc :global(.ln) {
    color: var(--t-rule-detail);
  }
</style>
