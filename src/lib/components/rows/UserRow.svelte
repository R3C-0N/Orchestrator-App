<script lang="ts">
  // Message utilisateur (`02`, restylé J15) : « ❯ » en dégradé, texte blanc gras sur une bande
  // gris foncé pleine largeur, lignes suivantes en retrait de 2, markdown rendu.
  import { markdownCached } from "../../render/markdown";
  import { PROMPT } from "../../render/glyphs";
  import type { UserRow } from "../../state/session.svelte";

  let { row }: { row: UserRow } = $props();
  const html = $derived(markdownCached(row.text));
</script>

<div class="user-band l">
  <span class="g grad">{PROMPT}</span>
  <div class="md">{@html html}</div>
</div>

<style>
  /* Maquette J15 : bandeau arrondi, « ❯ » en dégradé, texte gras. */
  .user-band {
    background: var(--t-band);
    margin: 0 -1ch;
    padding: 6px 1ch;
    border-radius: 6px;
  }
</style>
