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
  //
  // J14 (décision 15) : à droite, le hash de l'interface exécutée
  // (`__UI_HASH__`), « embarquée » ou « disque » d'après `get_ui_info`, et les
  // écarts : hash actif différent de celui de cette page, empreinte du contrat
  // différente de celle de l'exe, commit fusionné qui attend une relance.
  import type { UiInfo } from "../core/bindings/UiInfo";
  import type { SessionStore } from "../state/session.svelte";

  let {
    store,
    uiHash = null,
    uiInfo = null,
    contract = null,
  }: {
    store: SessionStore;
    /** `__UI_HASH__` de cette interface. */
    uiHash?: string | null;
    uiInfo?: UiInfo | null;
    /** `__CONTRACT__` de cette interface. */
    contract?: string | null;
  } = $props();

  const short = (h: string) => h.slice(0, 7);
  /** Même commit, l'un des deux pouvant être abrégé. */
  const same = (a: string, b: string) => a.startsWith(b) || b.startsWith(a);
  const activeDiffers = $derived(
    !!uiHash && !!uiInfo?.active_hash && uiInfo.source === "disk" && !same(uiHash, uiInfo.active_hash),
  );
  const contractDiffers = $derived(!!contract && !!uiInfo && uiInfo.contract !== contract);

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
  {#if uiHash}
    <span class="ui" data-testid="ui-hash">
      {#if uiInfo?.restart_required}
        <span class="warn" data-testid="ui-restart">relance requise ({short(uiInfo.restart_required)}) · </span>
      {/if}
      {#if activeDiffers && uiInfo?.active_hash}
        <span class="warn" data-testid="ui-mismatch">actif {short(uiInfo.active_hash)} ≠ </span>
      {/if}
      {#if contractDiffers}
        <span class="warn" data-testid="ui-contract">contrat ≠ exe · </span>
      {/if}
      <span class="dim">ui {short(uiHash)}{uiInfo ? (uiInfo.source === "disk" ? " (disque)" : " (embarquée)") : ""}</span>
    </span>
  {/if}
</div>

<style>
  .status-line {
    display: flex;
    padding: 0 1ch 0 3ch;
    white-space: pre;
    overflow: hidden;
  }
  .ui {
    margin-left: auto;
    padding-left: 2ch;
  }
  .warn {
    color: var(--warn);
  }
</style>
