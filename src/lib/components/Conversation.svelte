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
  // - Aucun défilement par `scrollToIndex` de virtua (bas, haut, bascule) : celui-ci
  //   reste actif tant que des tailles changent à moins de 150 ms d'écart
  //   (il refait `scrollTo(cible)` à chaque mesure, `virtua/lib/core`, `m`)
  //   et aucun geste ne l'annule ; pendant un streaming, la ligne vivante
  //   grandit sans arrêt et il ramenait la liste en bas contre la molette.
  //   Le recollage est direct (`toBottom`) et chaque essai revérifie `sticky`.
  import { tick, untrack } from "svelte";
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
  const items: Item[] = $derived(cache.build(store.rows, verbose, store.question?.toolUseId ?? null));

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
    stopRestore();
    if (e.deltaY < 0) gestureUp();
    else markIntent();
  }

  function onkeydown(e: KeyboardEvent) {
    if (["Home", "End", "PageUp", "PageDown", "ArrowUp", "ArrowDown", " "].includes(e.key)) stopRestore();
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

  /**
   * Recolle en bas, tout de suite, puis après le rendu Svelte (microtâche :
   * virtua applique la nouvelle hauteur totale) et à l'image suivante (les
   * lignes montées entre-temps ne sont mesurées qu'alors). Chaque essai
   * revérifie `sticky` : un geste de l'utilisateur entre deux essais gagne.
   */
  function toBottom() {
    const el = root?.querySelector<HTMLElement>(".vlist");
    if (!el) return;
    const go = () => {
      if (sticky && el.scrollHeight - el.clientHeight - el.scrollTop > 0.5) el.scrollTop = el.scrollHeight;
    };
    go();
    tick().then(go);
    requestAnimationFrame(go);
  }

  /**
   * Après un changement des données : recollage direct. Les hauteurs encore
   * estimées des nouvelles lignes sont corrigées à leur mesure
   * (`followResize`).
   */
  function follow() {
    if (!sticky || items.length === 0) return;
    if (distanceFromBottom() > 0.5) toBottom();
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
    // Défilement de l'élément lui-même ; la microtâche passe avant la
    // peinture, l'essai de l'image suivante borne l'écart à une image.
    toBottom();
  }

  export function scrollToTop() {
    // Même règle que `gestureUp` : on ne décroche que s'il y a de quoi
    // défiler (sinon aucun `scroll` ne rétablirait le collage).
    if (canScroll()) sticky = false;
    // Décalage 0 : exact sans mesure, pas besoin de `scrollToIndex` (qui
    // resterait actif pendant un streaming, voir l'en-tête).
    const el = root?.querySelector<HTMLElement>(".vlist");
    if (el) el.scrollTop = 0;
  }

  export function scrollToBottom() {
    sticky = true;
    toBottom();
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
    const down = () => {
      pointerDown = true;
      stopRestore();
    };
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
  /**
   * Jeton de génération de `restore` : un nouveau `restore` (ou la
   * destruction de sa vue) l'incrémente, et l'ancien s'arrête à sa prochaine
   * image, mise en place comprise. Sans lui, deux Ctrl+O rapprochés faisaient
   * tourner deux boucles.
   */
  let restoreGen = 0;
  /**
   * Jeton des gestes de l'utilisateur (molette, touche de défilement, clic) :
   * un geste l'incrémente et la boucle de recalage (`settle`) s'arrête à sa
   * prochaine image. Sans lui, la boucle réécrivait `scrollTop` pendant ses
   * 10 images contre la molette. Seuls comptent les gestes faits après la
   * mise en place (première image de `restore`) : un geste antérieur visait
   * une vue pas encore placée, et l'annulation laissait la nouvelle vue à une
   * position quelconque (hauteurs encore estimées).
   */
  let gestureGen = 0;
  /**
   * Ligne source du recalage en cours (`null` : aucun). Une bascule qui
   * arrive avant la fin la reprend, au lieu de lire le haut d'une vue pas
   * encore recalée.
   */
  let settling: string | null = null;

  function stopRestore() {
    gestureGen++;
    settling = null;
  }

  $effect.pre(() => {
    void verbose;
    // Avant la mise à jour du DOM : la liste montre encore l'ancienne vue.
    if (!list || sticky || shown.length === 0) {
      anchor = null;
      return;
    }
    if (settling !== null) {
      anchor = settling;
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
    const gen = ++restoreGen;
    const g0 = gestureGen;
    /**
     * Collé en bas au moment de la bascule. Lu hors suivi : l'attache tourne
     * dans un effet, et lire `sticky` (état réactif) l'y abonnait ; chaque
     * collage ou décollage relançait alors `restore` (nettoyage compris) avec
     * l'ancre de la dernière bascule, qui ramenait la vue sur elle.
     */
    const stuck = untrack(() => sticky);
    settling = stuck ? null : a;
    requestAnimationFrame(() => {
      if (gen !== restoreGen) return;
      // Gestes comptés à partir d'ici : un geste antérieur (entre la bascule
      // et cette image) visait une vue pas encore placée, il est sans effet.
      const g = gestureGen;
      /** Recalage encore voulu : ni nouvelle bascule, ni geste depuis la mise en place. */
      const live = () => gen === restoreGen && g === gestureGen;
      if (!list) {
        settling = null;
        return;
      }
      if (stuck && g !== g0) {
        // Collé à la bascule : la nouvelle vue part du bas, même si un geste
        // vers le haut l'a décrochée avant cette image, et son intention ne
        // doit pas décrocher sur les mesures qui suivent.
        sticky = true;
        intentUntil = 0;
      }
      if (sticky || a === null) {
        settling = null;
        toBottom();
        return;
      }
      settling = a;
      const j = itemIndexOfRow(items, a);
      if (j < 0) settling = null;
      else {
        markIntent(400);
        // Défilement direct au décalage de l'item (pas de `scrollToIndex`,
        // qui resterait actif pendant un streaming et ramènerait la vue sur
        // la ligne j contre la molette, voir l'en-tête).
        const el0 = root?.querySelector<HTMLElement>(".vlist");
        if (el0) el0.scrollTop = list.getItemOffset(j);
        // La nouvelle liste n'a que des hauteurs estimées : une fois les
        // lignes mesurées, on recale l'item en haut. Tant qu'il n'est pas
        // rendu, son décalage (que virtua affine à chaque mesure) sert de
        // cible. Au plus 10 images, et arrêt dès un geste de l'utilisateur
        // (`gestureGen`) ou une nouvelle bascule (`restoreGen`) : un geste
        // n'est jamais contré.
        let n = 0;
        let aligned = 0;
        const settle = () => {
          if (!live()) return;
          const el = root?.querySelector<HTMLElement>(".vlist");
          const it = el?.querySelector<HTMLElement>(`[data-index="${j}"]`);
          if (el && it) {
            const d = it.getBoundingClientRect().top - el.getBoundingClientRect().top;
            if (Math.abs(d) > 0.5) {
              markIntent(200);
              el.scrollTop += d;
              aligned = 0;
            } else aligned++;
          } else if (el && list) {
            markIntent(200);
            el.scrollTop = list.getItemOffset(j);
            aligned = 0;
          }
          if (++n < 10 && aligned < 2) requestAnimationFrame(settle);
          else if (live()) settling = null;
        };
        requestAnimationFrame(settle);
      }
    });
    // Vue détruite (bascule suivante, fermeture) : sa mise en place et son
    // recalage s'arrêtent. Seulement si aucun `restore` plus récent n'a pris
    // le jeton : l'ordre entre ce nettoyage et l'attache de la vue suivante
    // n'a alors pas d'importance.
    return () => {
      if (gen === restoreGen) {
        restoreGen++;
        settling = null;
      }
    };
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
  <!-- Question ouverte (`07`) : la carte remplace l'indicateur de tour. -->
  {#if busy && turnSince > 0 && !store.question}
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
    border: 1px solid var(--tree);
    font: inherit;
    padding: 0 1ch;
    cursor: pointer;
  }
</style>
