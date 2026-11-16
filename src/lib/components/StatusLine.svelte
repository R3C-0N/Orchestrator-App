<script lang="ts">
  // Ligne d'état sous la saisie, celle de Claude Code **par défaut** (sans
  // `statusLine` réglée). Dans les captures de J1, la première ligne sous
  // la saisie (emojis, branche, contexte, heure) est la `statusLine`
  // personnalisée du poste : hors référence. La seconde est la ligne
  // intégrée du TUI, reproduite ici :
  //   « ⏵⏵ auto mode on (shift+tab to cycle) · ← 2 agents » (`02`, `03`…),
  // glyphe et mode en jaune `#FFC107`, la suite en gris (mesuré sur `02`).
  //
  // Écarts assumés (rien d'inventé) :
  // - « (shift+tab to cycle) » n'est pas affiché : l'app ne change pas le
  //   mode de permission au clavier (décision 7, toujours `bypassPermissions`) ;
  // - « ← N agents » et « /tasks to see subagents » renvoient à des vues du
  //   TUI que l'app remplace par la colonne des agents (décision 4, J9).
  // Mode `default` : « ? for shortcuts », comme le TUI hors mode spécial.
  import type { SessionStore } from "../state/session.svelte";

  let { store }: { store: SessionStore } = $props();

  /** Libellé et couleur de chaque mode (seul `auto` est mesuré ; les autres suivent le thème `dark`). */
  const MODES: Record<string, { label: string; color: string }> = {
    auto: { label: "auto mode on", color: "#FFC107" },
    bypassPermissions: { label: "bypass permissions on", color: "var(--t-error)" },
    acceptEdits: { label: "accept edits on", color: "#AF87FF" },
    plan: { label: "plan mode on", color: "#48968C" },
  };

  const mode = $derived(store.meta.permissionMode ? MODES[store.meta.permissionMode] : undefined);
</script>

<div class="status-line term" data-testid="status-line">
  {#if mode}
    <span style:color={mode.color}>⏵⏵ {mode.label}</span>
  {:else}
    <span class="dim">? for shortcuts</span>
  {/if}
</div>

<style>
  .status-line {
    padding: 0 1ch 0 3ch;
    white-space: pre;
    overflow: hidden;
  }
</style>
