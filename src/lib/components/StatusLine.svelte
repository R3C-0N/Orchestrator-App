<script lang="ts">
  // Ligne sous la saisie, celle de Claude Code **par défaut** (sans
  // `statusLine` réglée) :
  //   « ⏵⏵ auto mode on (shift+tab to cycle) · ← 2 agents » (`02`, `03`…).
  //
  // Écarts assumés (rien d'inventé) :
  // - « (shift+tab to cycle) » n'est pas affiché : l'app ne change pas le
  //   mode de permission au clavier (décision 7, toujours `bypassPermissions`) ;
  // - « ← N agents » et « /tasks to see subagents » renvoient à des vues du
  //   TUI que l'app remplace par la colonne des agents (décision 4, J9).
  // Mode `default` : « ? for shortcuts », comme le TUI hors mode spécial.
  //
  // J15 (maquette) : le hash de l'interface passe dans la barre d'état et ses
  // écarts dans un bandeau en haut (`UiNotice`).
  import type { SessionStore } from "../state/session.svelte";

  let { store }: { store: SessionStore } = $props();

  /**
   * Libellé et couleur de chaque mode (seul `auto` est mesuré ; les autres
   * suivent le thème `dark` du TUI). Couleurs en variables (`--mode-*` dans
   * app.css) : le thème clair les assombrit.
   */
  const MODES: Record<string, { label: string; color: string }> = {
    auto: { label: "auto mode on", color: "var(--mode-auto)" },
    bypassPermissions: { label: "bypass permissions on", color: "var(--danger)" },
    acceptEdits: { label: "accept edits on", color: "var(--mode-accept)" },
    plan: { label: "plan mode on", color: "var(--mode-plan)" },
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
    display: flex;
    padding: 8px 1ch 8px 30px;
    white-space: pre;
    overflow: hidden;
  }
</style>
