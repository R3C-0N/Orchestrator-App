<script lang="ts">
  // Conversation : liste virtualisée (virtua) du fil principal, rendu
  // « terminal » du TUI 2.1.325 (plan 1.7, jalon J5).
  //
  // - Les `Row` du magasin deviennent des `Item` affichés (render/group.ts) :
  //   vue normale avec appels regroupés, ou vue détaillée (Ctrl+O, `verbose`)
  //   avec un appel par ligne et les sorties complètes.
  // - Collée en bas tant que l'utilisateur n'a pas remonté : chaque ajout et
  //   chaque croissance de la ligne vivante ramène en bas.
  // - Le collage ne se perd que sur une intention de l'utilisateur (molette,
  //   glisser, clavier) qui l'éloigne du bas, jamais sur un saut de mesure ;
  //   il revient dès qu'il redescend au bas.
  // - La bascule de vue garde la position de lecture : la ligne source en
  //   haut de la vue reste en haut après la bascule.
  import { tick } from "svelte";
  import { VList } from "virtua/svelte";
  import { ItemCache, firstRowId, itemIndexOfRow, type Item } from "../render/group";
  import type { SessionStore } from "../state/session.svelte";
  import AgentFinished from "./rows/AgentFinished.svelte";
  import AgentLaunch from "./rows/AgentLaunch.svelte";
  import AssistantText from "./rows/AssistantText.svelte";
  import Interrupted from "./rows/Interrupted.svelte";
  import PlainRow from "./rows/PlainRow.svelte";
  import ToolCall from "./rows/ToolCall.svelte";
  import ToolGroup from "./rows/ToolGroup.svelte";
  import TurnIndicator from "./rows/TurnIndicator.svelte";
  import TurnResult from "./rows/TurnResult.svelte";
  import UserRow from "./rows/UserRow.svelte";

  let { store, verbose = false }: { store: SessionStore; verbose?: boolean } = $props();

  const cache = new ItemCache();
  const items: Item[] = $derived(cache.build(store.rows, verbose));

  let list: VList<Item> | undefined = $state();
  /** Collé en bas (suivi du streaming). */
  let sticky = $state(true);
  /** Fin de la dernière intention de défilement de l'utilisateur. */
  let intentUntil = 0;
  let pointerDown = false;
  /** Seuil « au bas » en px (moins d'une ligne). */
  const BOTTOM_EPS = 8;

  function distanceFromBottom(): number {
    if (!list) return 0;
    return list.getScrollSize() - list.getViewportSize() - list.getScrollOffset();
  }

  function markIntent(ms = 250) {
    intentUntil = performance.now() + ms;
  }

  /** Vrai si le contenu dépasse la vue : un geste vers le haut peut alors l'éloigner du bas. */
  function canScroll(): boolean {
    return !!list && list.getScrollSize() - list.getViewportSize() > BOTTOM_EPS;
  }

  /**
   * Geste vers le haut : décroche tout de suite (sans attendre l'événement
   * scroll, qu'une croissance de la ligne vivante pourrait devancer), mais
   * seulement s'il y a de quoi défiler. Sinon aucun `scroll` ne suivrait et
   * le collage ne reviendrait jamais.
   */
  function gestureUp() {
    markIntent();
    if (canScroll()) sticky = false;
  }

  function onwheel(e: WheelEvent) {
    if (e.deltaY < 0) gestureUp();
    else markIntent();
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Home" || (e.ctrlKey && e.key === "Home")) {
      e.preventDefault();
      scrollToTop();
    } else if (e.key === "End" || (e.ctrlKey && e.key === "End")) {
      e.preventDefault();
      scrollToBottom();
    } else if (["PageUp", "PageDown", "ArrowUp", "ArrowDown"].includes(e.key)) {
      if (e.key === "PageUp" || e.key === "ArrowUp") gestureUp();
      else markIntent();
    }
  }

  function onscroll() {
    const d = distanceFromBottom();
    if (d <= BOTTOM_EPS) {
      sticky = true;
    } else if (pointerDown || performance.now() < intentUntil) {
      sticky = false;
    }
  }

  let root: HTMLElement | undefined;

  /** Après un changement des données : virtua place la dernière ligne (hauteurs estimées comprises). */
  function follow() {
    if (!sticky || !list || items.length === 0) return;
    if (distanceFromBottom() > 0.5) list.scrollToIndex(items.length - 1, { align: "end" });
  }

  /**
   * Après une mesure (rappel du ResizeObserver de virtua) : défilement direct
   * de l'élément, sans relancer un rendu de la liste dans le rappel (sinon
   * « ResizeObserver loop completed with undelivered notifications »).
   */
  function followResize() {
    if (!sticky) {
      // Le contenu ne dépasse plus la vue (fenêtre agrandie, lignes repliées) :
      // on est au bas sans qu'aucun `scroll` ne le dise ; reprendre le collage.
      if (!pointerDown && performance.now() >= intentUntil && distanceFromBottom() <= BOTTOM_EPS) sticky = true;
      return;
    }
    const el = root?.querySelector<HTMLElement>(".vlist");
    if (!el) return;
    const toBottom = () => {
      if (sticky && el.scrollHeight - el.clientHeight - el.scrollTop > 0.5) el.scrollTop = el.scrollHeight;
    };
    toBottom();
    // virtua applique la nouvelle hauteur totale au prochain rendu Svelte
    // (microtâche) : on recommence après, toujours avant la peinture.
    tick().then(toBottom);
    // Les lignes montées pendant ce rappel ne sont mesurées par virtua qu'à
    // l'image suivante : dernière correction à ce moment-là (écart borné à une image).
    requestAnimationFrame(toBottom);
  }

  export function scrollToTop() {
    sticky = false;
    list?.scrollToIndex(0, { align: "start" });
  }

  export function scrollToBottom() {
    sticky = true;
    list?.scrollToIndex(items.length - 1, { align: "end" });
  }

  export function isSticky() {
    return sticky;
  }

  export function handle() {
    return list;
  }

  /** Nombre de lignes affichées (après regroupement). */
  export function itemCount() {
    return items.length;
  }

  // Écouteurs d'intention posés à la main (molette passive : ne bloque pas le défilement).
  function listen(el: HTMLElement) {
    root = el;
    const down = () => (pointerDown = true);
    const up = () => {
      pointerDown = false;
      markIntent();
    };
    el.addEventListener("wheel", onwheel, { passive: true });
    el.addEventListener("keydown", onkeydown);
    el.addEventListener("pointerdown", down);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
    return () => {
      el.removeEventListener("wheel", onwheel);
      el.removeEventListener("keydown", onkeydown);
      el.removeEventListener("pointerdown", down);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", up);
    };
  }

  // Nouvelle ligne, ou croissance de la ligne vivante : suivre si collé.
  $effect(() => {
    void items.length;
    void store.live?.text.length;
    follow();
  });

  // --- Bascule Ctrl+O sans perdre la lecture ---------------------------------

  /** Items affichés au dernier rendu (non réactif : lu avant la bascule). */
  let shown: Item[] = [];
  $effect(() => {
    shown = items;
  });
  /** Ligne source en haut de la vue au moment de la bascule. */
  let anchor: string | null = null;

  $effect.pre(() => {
    void verbose;
    // Avant la mise à jour du DOM : la liste montre encore l'ancienne vue.
    if (!list || sticky || shown.length === 0) {
      anchor = null;
      return;
    }
    // Premier item vraiment visible : un item dont il ne reste que quelques
    // pixels en haut ne compte pas comme « la ligne lue ».
    const i = list.findItemIndex(list.getScrollOffset() + 4);
    anchor = shown[i] ? firstRowId(shown[i]) : null;
  });

  /** Après le montage de la liste de la nouvelle vue : même ligne en haut, ou toujours collé en bas. */
  function restore(_el: HTMLElement) {
    const a = anchor;
    requestAnimationFrame(() => {
      if (!list) return;
      if (sticky || a === null) {
        if (items.length) list.scrollToIndex(items.length - 1, { align: "end" });
        return;
      }
      const j = itemIndexOfRow(items, a);
      if (j >= 0) {
        markIntent(400);
        list.scrollToIndex(j, { align: "start" });
        // La nouvelle liste n'a que des hauteurs estimées : une fois les
        // lignes mesurées, on recale l'item en haut (quelques images).
        let n = 0;
        const settle = () => {
          const el = root?.querySelector<HTMLElement>(".vlist");
          const it = el?.querySelector<HTMLElement>(`[data-index="${j}"]`);
          if (el && it) {
            const d = it.getBoundingClientRect().top - el.getBoundingClientRect().top;
            if (Math.abs(d) > 0.5) {
              markIntent(200);
              el.scrollTop += d;
            }
          }
          if (++n < 4) requestAnimationFrame(settle);
        };
        requestAnimationFrame(settle);
      }
    });
  }

  // --- Indicateur de tour (`k1`) ------------------------------------------------

  const busy = $derived(
    store.status === "Requesting" || store.status === "Streaming" || store.status === "Interrupting",
  );
  let turnSince = $state(0);
  let turnFirstRow = 0;
  $effect.pre(() => {
    if (busy) {
      if (turnSince === 0) {
        turnSince = Date.now();
        turnFirstRow = store.rows.length;
      }
    } else turnSince = 0;
  });
  const turnChars = $derived.by(() => {
    if (!busy) return 0;
    let n = 0;
    const rows = store.rows;
    for (let i = Math.max(0, turnFirstRow - 1); i < rows.length; i++) {
      const r = rows[i];
      if (r.kind === "assistant") n += r.text.length;
      else if (r.kind === "tool" && r.input) n += JSON.stringify(r.input).length;
    }
    return n;
  });
</script>

<div class="conversation term" role="log" aria-live="off" {@attach listen}>
  {#key verbose}
    <div class="host" {@attach restore}>
      <VList
        bind:this={list}
        data={items}
        getKey={(it) => it.key}
        class="vlist"
        tabindex={0}
        {onscroll}
        onresize={followResize}
      >
        {#snippet children(item: Item, index: number)}
          <div
            class="row"
            class:attached={item.kind === "interrupted" && item.attached}
            data-row-kind={item.kind}
            data-index={index}
            data-first-row={firstRowId(item)}
          >
            {#if item.kind === "user"}
              <UserRow row={item.row} />
            {:else if item.kind === "assistant"}
              <AssistantText row={item.row} {verbose} model={store.meta.model} />
            {:else if item.kind === "group"}
              <ToolGroup rows={item.rows} noResult={item.noResult} />
            {:else if item.kind === "tool"}
              <ToolCall row={item.row} {verbose} cwd={store.meta.cwd} noResult={item.noResult} />
            {:else if item.kind === "agents"}
              <AgentLaunch rows={item.rows} />
            {:else if item.kind === "agentDone"}
              <AgentFinished row={item.row} {verbose} />
            {:else if item.kind === "turn"}
              <TurnResult row={item.row} />
            {:else if item.kind === "interrupted"}
              <Interrupted row={item.row} />
            {:else}
              <PlainRow row={item.row} />
            {/if}
          </div>
        {/snippet}
      </VList>
    </div>
  {/key}
  {#if busy && turnSince > 0}
    <div class="indicator-line">
      <TurnIndicator since={turnSince} chars={turnChars} thoughtMs={store.thinkingMs} />
    </div>
  {/if}
  {#if verbose}
    <!-- Barre du TUI (03b) : « … · v to open in notepad · ? for shortcuts »
         n'est pas repris, l'app n'a ni export vers le Bloc-notes ni aide des
         raccourcis. -->
    <div class="detail-bar" data-testid="detail-bar">
      <span>Showing detailed transcript · ctrl+o to toggle · ↑↓ scroll</span>
      <span>verbose</span>
    </div>
  {/if}
  {#if !sticky}
    <button class="jump" onclick={scrollToBottom} title="Revenir en bas (Fin)">↓ bas</button>
  {/if}
</div>

<style>
  .conversation {
    position: relative;
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .host {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .conversation :global(.vlist) {
    outline: none;
    overscroll-behavior: contain;
    flex: 1;
    min-height: 0;
  }
  /* Une ligne vide entre deux blocs, comme le terminal. */
  .row {
    padding: var(--t-lh) 1ch 0;
  }
  .row.attached {
    padding-top: 0;
  }
  .indicator-line {
    padding: var(--t-lh) 1ch 0;
  }
  /* Barre de la vue détaillée (`03b`) : règle grise puis l'aide, « verbose » à droite. */
  .detail-bar {
    display: flex;
    justify-content: space-between;
    margin: var(--t-lh) 1ch 0;
    padding: 0 1ch 0 2ch;
    border-top: 1px solid var(--t-rule-detail);
    color: var(--t-dim);
  }
  .jump {
    position: absolute;
    right: 2ch;
    bottom: 8px;
    background: var(--t-bg);
    color: var(--t-fg);
    border: 1px solid #555;
    font: inherit;
    padding: 0 1ch;
    cursor: pointer;
  }
</style>
