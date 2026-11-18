<script lang="ts">
  // Bandeau d'écarts de l'interface (J14, décision 15), en haut de la fenêtre
  // comme l'avertissement de version (maquette J15 : il quitte la ligne
  // d'état). Affiché seulement s'il y a un écart :
  // - un commit fusionné attend une relance de l'exe (« relance requise ») ;
  // - le hash actif diffère de celui de cette page ;
  // - l'empreinte du contrat diffère de celle de l'exe.
  // Le hash de l'interface exécutée reste dans la barre d'état (`ui-hash`).
  import type { UiInfo } from "../core/bindings/UiInfo";

  let {
    uiHash = null,
    uiInfo = null,
    contract = null,
  }: {
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
  const restart = $derived(uiInfo?.restart_required ?? null);
</script>

{#if uiHash && (restart || activeDiffers || contractDiffers)}
  <div class="banner" data-testid="ui-notice">
    {#if restart}
      <span data-testid="ui-restart">relance requise ({short(restart)})</span>
    {/if}
    {#if activeDiffers && uiInfo?.active_hash}
      <span data-testid="ui-mismatch">actif {short(uiInfo.active_hash)} ≠ ui {short(uiHash)}</span>
    {/if}
    {#if contractDiffers}
      <span data-testid="ui-contract">contrat ≠ exe</span>
    {/if}
  </div>
{/if}

<style>
  .banner {
    display: flex;
    flex-wrap: wrap;
    gap: 0 2ch;
    padding: 2px 1ch;
    color: var(--warn);
    background: var(--warn-bg);
    border-bottom: 1px solid var(--rule);
  }
</style>
