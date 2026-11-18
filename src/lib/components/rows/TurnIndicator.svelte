<script lang="ts">
  // Indicateur pendant le tour (`k1`) : « ✽ Choreographing… (8s · ↓ 255
  // tokens) », au-dessus de la saisie ; glyphe animé et verbe en orange
  // saumon, le reste en gris.
  //
  // Tokens : le flux ne donne pas le compte de sortie en cours de tour ;
  // on l'estime à 4 caractères par token sur le texte reçu pendant le tour.
  // « thought for Ns » : durée des blocs Thinking du tour (début et fin du
  // bloc dans le flux). La ligne « ◐ medium · /effort » n'est pas reproduite :
  // le niveau d'effort n'est dans aucun événement du contrat.
  import { ACTIVE_VERBS, SPINNER, pickStable } from "../../render/glyphs";
  import { frames, seconds as clock } from "../../render/clock.svelte";
  import { seconds } from "../../render/tools";

  let { since, chars, thoughtMs = 0 }: { since: number; chars: number; thoughtMs?: number } = $props();

  const verb = $derived(pickStable(String(since), ACTIVE_VERBS));
  const glyph = $derived(SPINNER[Math.floor(frames.now / 120) % SPINNER.length]);
  const tokens = $derived(Math.round(chars / 4));
</script>

<div class="l indicator">
  <span class="g grad">{glyph}</span>
  <span class="x"
    ><span class="verb">{verb}…</span>
    <span class="dim"
      >({seconds(clock.now - since)} · ↓ {tokens} tokens{thoughtMs > 0 ? ` · thought for ${seconds(thoughtMs)}` : ""})</span
    ></span
  >
</div>

<style>
  .verb {
    color: var(--t-spinner-text);
  }
</style>
