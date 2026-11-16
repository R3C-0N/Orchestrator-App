<script lang="ts">
  // Conversation : liste virtualisée (virtua) du fil principal.
  //
  // - Collée en bas tant que l'utilisateur n'a pas remonté : chaque ajout et
  //   chaque croissance de la ligne vivante ramène en bas.
  // - Le collage ne se perd que sur une intention de l'utilisateur (molette,
  //   glisser, clavier) qui l'éloigne du bas, jamais sur un saut de mesure ;
  //   il revient dès qu'il redescend au bas.
  // - Rendu provisoire « terminal » (texte mono, puce ●) ; le rendu fidèle
  //   des lignes est le jalon J5.
  import { VList } from "virtua/svelte";
  import type { Row, SessionStore } from "../state/session.svelte";

  let { store }: { store: SessionStore } = $props();

  let list: VList<Row> | undefined = $state();
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

  function onwheel(e: WheelEvent) {
    markIntent();
    // Remonter à la molette décroche immédiatement, sans attendre l'événement scroll.
    if (e.deltaY < 0) sticky = false;
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Home" || (e.ctrlKey && e.key === "Home")) {
      e.preventDefault();
      scrollToTop();
    } else if (e.key === "End" || (e.ctrlKey && e.key === "End")) {
      e.preventDefault();
      scrollToBottom();
    } else if (["PageUp", "PageDown", "ArrowUp", "ArrowDown"].includes(e.key)) {
      markIntent();
      if (e.key === "PageUp" || e.key === "ArrowUp") sticky = false;
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
    if (!sticky || !list || store.rows.length === 0) return;
    if (distanceFromBottom() > 0.5) list.scrollToIndex(store.rows.length - 1, { align: "end" });
  }

  /**
   * Après une mesure (rappel du ResizeObserver de virtua) : défilement direct
   * de l'élément, sans relancer un rendu de la liste dans le rappel (sinon
   * « ResizeObserver loop completed with undelivered notifications »).
   */
  function followResize() {
    if (!sticky) return;
    const el = root?.querySelector<HTMLElement>(".vlist");
    if (el && el.scrollHeight - el.clientHeight - el.scrollTop > 0.5) el.scrollTop = el.scrollHeight;
  }

  export function scrollToTop() {
    sticky = false;
    list?.scrollToIndex(0, { align: "start" });
  }

  export function scrollToBottom() {
    sticky = true;
    list?.scrollToIndex(store.rows.length - 1, { align: "end" });
  }

  export function isSticky() {
    return sticky;
  }

  export function handle() {
    return list;
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
    return () => {
      el.removeEventListener("wheel", onwheel);
      el.removeEventListener("keydown", onkeydown);
      el.removeEventListener("pointerdown", down);
      window.removeEventListener("pointerup", up);
    };
  }

  // Nouvelle ligne, ou croissance de la ligne vivante : suivre si collé.
  $effect(() => {
    void store.rows.length;
    void store.live?.text.length;
    follow();
  });

  function toolLabel(row: Extract<Row, { kind: "tool" }>): string {
    const inp = row.input as Record<string, unknown> | null;
    const arg =
      inp && typeof inp === "object"
        ? (inp.file_path ?? inp.command ?? inp.pattern ?? inp.path ?? "")
        : "";
    return `${row.name}(${String(arg)})`;
  }

  function resultSummary(text: string): string {
    const lines = text.split("\n");
    const first = lines[0] ?? "";
    return lines.length > 1 ? `${first} … +${lines.length - 1} lines` : first;
  }

  function seconds(ms: number): string {
    const s = Math.round(ms / 1000);
    return s >= 60 ? `${Math.floor(s / 60)}m ${s % 60}s` : `${s}s`;
  }
</script>

<div class="conversation" role="log" aria-live="off" {@attach listen}>
  <VList
    bind:this={list}
    data={store.rows}
    getKey={(r) => r.id}
    class="vlist"
    tabindex={0}
    {onscroll}
    onresize={followResize}
  >
    {#snippet children(row: Row, index: number)}
      <div class="row {row.kind}" data-row-kind={row.kind} data-index={index}>
        {#if row.kind === "user"}
          <div class="band"><span class="glyph dim">❯</span><span class="text strong">{row.text}</span></div>
        {:else if row.kind === "assistant"}
          <div class="line"><span class="glyph">●</span><span class="text" class:streaming={!row.done}>{row.text}</span></div>
        {:else if row.kind === "tool"}
          <div class="line">
            <span class="glyph" class:ok={row.result && !row.result.isError} class:err={row.result?.isError}>●</span>
            <span class="text">{toolLabel(row)}</span>
          </div>
          {#if row.result}
            <div class="line dim" class:errtext={row.result.isError}>
              <span class="glyph"></span><span class="text">⎿  {resultSummary(row.result.text)}</span>
            </div>
          {/if}
        {:else if row.kind === "agent"}
          <div class="line">
            <span class="glyph ok">●</span>
            <span class="text">Agent "{row.agent.description}" · {row.agent.status}</span>
          </div>
        {:else if row.kind === "turn"}
          <div class="line dim">
            <span class="glyph">✻</span><span class="text">Worked for {seconds(row.durationMs)}</span>
          </div>
        {:else if row.kind === "interrupted"}
          <div class="line dim"><span class="glyph"></span><span class="text">⎿  {row.text}</span></div>
        {:else}
          <div class="line" class:errtext={row.level === "Error"}>
            <span class="glyph">!</span><span class="text">{row.code} : {row.message}</span>
          </div>
        {/if}
      </div>
    {/snippet}
  </VList>
  {#if !sticky}
    <button class="jump" onclick={scrollToBottom} title="Revenir en bas (Fin)">↓ bas</button>
  {/if}
</div>

<style>
  .conversation {
    position: relative;
    flex: 1;
    min-height: 0;
    background: #0c0c0c;
    color: #cccccc;
    font-family: "Cascadia Mono", Consolas, monospace;
    font-size: 14px;
    line-height: 18px;
  }
  .conversation :global(.vlist) {
    outline: none;
    overscroll-behavior: contain;
  }
  .row {
    padding: 18px 1ch 0;
  }
  .line,
  .band {
    display: grid;
    grid-template-columns: 2ch 1fr;
  }
  .band {
    background: #373737;
    margin: 0 -1ch;
    padding: 0 1ch;
  }
  .text {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    min-width: 0;
  }
  .strong {
    color: #ffffff;
    font-weight: bold;
  }
  .glyph {
    color: #ffffff;
  }
  .dim,
  .dim .glyph,
  .glyph.dim {
    color: #8b8b8b;
  }
  .ok {
    color: #16c60c;
  }
  .err,
  .errtext,
  .errtext .glyph {
    color: #ff6b80;
  }
  .streaming::after {
    content: "▍";
    color: #8b8b8b;
  }
  .jump {
    position: absolute;
    right: 2ch;
    bottom: 8px;
    background: #0c0c0c;
    color: #cccccc;
    border: 1px solid #555;
    font: inherit;
    padding: 0 1ch;
    cursor: pointer;
  }
</style>
