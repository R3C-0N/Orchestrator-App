<script lang="ts">
  // Lignes de résultat : « ⎿  » (2 espaces, ⎿, 2 espaces) en gris, puis le
  // texte ; les lignes suivantes en retrait de 5 sous le texte (`03b`, `09b`).
  import { ELBOW } from "../../render/glyphs";

  let {
    lines,
    tone = "",
    more = 0,
    html = false,
  }: {
    /** Lignes de texte (ou de HTML déjà échappé si `html`). */
    lines: string[];
    /** Classe de couleur : "", "dim", "err". */
    tone?: string;
    /** Lignes masquées (vue normale) : « … +N lines (ctrl+o to expand) ». */
    more?: number;
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
  <div class="rc x dim">… +{more} lines (ctrl+o to expand)</div>
{/if}
