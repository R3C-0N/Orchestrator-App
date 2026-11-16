<script lang="ts">
  // Message utilisateur (`02`) : « ❯ » gris, texte blanc gras sur une bande
  // gris foncé pleine largeur, lignes suivantes en retrait de 2, markdown rendu.
  import { markdownCached } from "../../render/markdown";
  import { PROMPT } from "../../render/glyphs";
  import type { UserRow } from "../../state/session.svelte";

  let { row }: { row: UserRow } = $props();
  const html = $derived(markdownCached(row.text));
</script>

<div class="user-band l">
  <span class="g dim">{PROMPT}</span>
  <div class="md">{@html html}</div>
</div>

<style>
  .user-band {
    background: var(--t-band);
    /* Pleine largeur : la bande couvre aussi la marge de la ligne. */
    margin: 0 -1ch;
    padding: 0 1ch;
  }
</style>
