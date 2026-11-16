<script module lang="ts">
  // Modèle pur de la carte (testé dans `e2e/unit/question.test.ts`).
  import type { Answer } from "../core/bindings/Answer";
  import type { QuestionItem } from "../core/bindings/QuestionItem";

  /**
   * Entrées d'une question, dans l'ordre d'affichage et de numérotation :
   * les options, « Type something. » (réponse libre), « Submit » (multi-
   * sélection seulement), puis, après la règle, « Chat about this ».
   */
  export type Item =
    | { kind: "option"; index: number }
    | { kind: "other" }
    | { kind: "submit" }
    | { kind: "chat" };

  export function itemsOf(q: QuestionItem): Item[] {
    const items: Item[] = q.options.map((_, index) => ({ kind: "option", index }));
    items.push({ kind: "other" });
    if (q.multi_select) items.push({ kind: "submit" });
    items.push({ kind: "chat" });
    return items;
  }

  /** Choix en cours pour une question. */
  export type Pick = {
    /** Options cochées (index), une seule hors multi-sélection. */
    selected: number[];
    /** Texte de « Type something. ». */
    other: string;
    /** Multi-sélection : la réponse libre est cochée. */
    otherOn: boolean;
    /** La question a reçu une réponse (Entrée sur une option, un texte ou « Submit »). */
    done: boolean;
  };

  export const emptyPick = (): Pick => ({ selected: [], other: "", otherOn: false, done: false });

  /**
   * Réponse d'une question au format de `answers` (spec 4.5, `t4`) : le label
   * choisi, ou le texte libre ; en multi-sélection, la liste des labels cochés
   * (ordre des options), puis le texte libre s'il est coché. `null` : rien.
   */
  export function answerOf(q: QuestionItem, p: Pick): Answer | null {
    const other = p.other.trim();
    if (!q.multi_select) {
      if (p.selected.length > 0) return q.options[p.selected[0]]?.label ?? null;
      return other.length > 0 ? other : null;
    }
    const labels = [...p.selected].sort((a, b) => a - b).map((i) => q.options[i].label);
    if (p.otherOn && other.length > 0) labels.push(other);
    return labels.length > 0 ? labels : null;
  }

  /** `answers` envoyé par `answer_question` : clé = texte de la question ; questions sans réponse omises. */
  export function buildAnswers(questions: QuestionItem[], picks: Pick[]): Record<string, Answer> {
    const out: Record<string, Answer> = {};
    questions.forEach((q, i) => {
      const a = picks[i] && picks[i].done ? answerOf(q, picks[i]) : null;
      if (a !== null) out[q.question] = a;
    });
    return out;
  }

  /** Texte d'une réponse pour l'écran de relecture. */
  export function answerText(a: Answer): string {
    return Array.isArray(a) ? a.join(", ") : a;
  }
</script>

<script lang="ts">
  // Carte `AskUserQuestion` (jalon J8), rendu de la capture `07` : pas
  // d'encadré ; règle `─` grise, pastille « ☐ <header> » (noir sur bleu
  // lavande), question en blanc gras, options `❯ 1. <label>` (sélection en
  // bleu lavande) avec description grise en retrait de 5, « N. Type
  // something. », règle, « N+1. Chat about this », pied gris. La carte prend
  // la place de la saisie (`App.svelte`).
  //
  // Clavier : ↑↓ déplacent, Entrée choisit, chiffres 1 à 9 choisissent
  // directement ; Échap passe par le gestionnaire global (`keys.svelte.ts`) :
  // ignore si `canSkip`, sinon rien.
  //
  // Hors capture (inventé, à valider en revue, rendu du TUI non capturé) :
  // - multi-sélection : `[ ]` / `[✔]` devant le label, Entrée ou Espace coche,
  //   « Submit » (« Next » s'il reste des questions) valide la question ;
  // - 2 à 4 questions : barre d'onglets `←  ☐ A  ☐ B  ✔ Submit  →` (← → ou
  //   Tab changent d'onglet, ☒ = répondue), puis un écran de relecture
  //   « Submit answers » / « Cancel ».
  import type { Question } from "../state/session.svelte";

  let {
    question,
    onanswer,
    onskip,
    onchat,
  }: {
    question: Question;
    /** Réponses prêtes (`answer_question`). */
    onanswer: (answers: Record<string, Answer>) => void;
    /** « Cancel » de l'écran de relecture (seulement si `canSkip`). */
    onskip: () => void;
    /** « Chat about this » (seulement si `canSkip`, voir `App.svelte`). */
    onchat: () => void;
  } = $props();

  const RULE = "─".repeat(400);

  const qs = $derived(question.questions);
  /** Plusieurs questions : onglets et écran de relecture (onglet `qs.length`). */
  const multi = $derived(qs.length > 1);

  let tab = $state(0);
  let cursors: number[] = $state([]);
  let picks: Pick[] = $state([]);
  /** Curseur de l'écran de relecture (0 : Submit answers, 1 : Cancel). */
  let reviewCursor = $state(0);
  /** Réponse partie : plus aucune touche jusqu'à `question_closed`. */
  let sent = $state(false);

  let root: HTMLDivElement | undefined = $state();
  let otherInput: HTMLInputElement | undefined = $state();

  // Nouvelle question : tout repart de zéro.
  $effect.pre(() => {
    void question.requestId;
    tab = 0;
    cursors = question.questions.map(() => 0);
    picks = question.questions.map(() => emptyPick());
    reviewCursor = 0;
    sent = false;
  });

  const onReview = $derived(multi && tab === qs.length);
  const q = $derived(onReview ? null : qs[tab]);
  const items = $derived(q ? itemsOf(q) : []);
  const cursor = $derived(cursors[tab] ?? 0);
  const current = $derived(items[cursor]);
  const pick = $derived(picks[tab]);

  // Le champ libre a le focus quand le curseur est dessus, la carte sinon.
  $effect(() => {
    const onOther = current?.kind === "other" && !sent;
    const r = root;
    const input = otherInput;
    queueMicrotask(() => {
      if (onOther) input?.focus();
      else r?.focus();
    });
  });

  export function focus(): void {
    if (current?.kind === "other") otherInput?.focus();
    else root?.focus();
  }

  const answers = $derived(buildAnswers(qs, picks));
  const footer = $derived(
    (multi ? "Enter to select · Tab/Arrow keys to navigate" : "Enter to select · ↑/↓ to navigate") +
      (question.canSkip ? " · Esc to cancel" : ""),
  );

  function move(d: number): void {
    if (onReview) {
      reviewCursor = (reviewCursor + d + 2) % 2;
      return;
    }
    const n = items.length;
    cursors[tab] = (cursor + d + n) % n;
  }

  function goTab(t: number): void {
    const n = multi ? qs.length + 1 : 1;
    tab = (t + n) % n;
  }

  /** Question répondue : onglet suivant non répondu, sinon relecture, sinon envoi. */
  function next(): void {
    if (!multi) {
      submit();
      return;
    }
    for (let k = 1; k <= qs.length; k++) {
      const t = (tab + k) % qs.length;
      if (!picks[t].done) {
        tab = t;
        return;
      }
    }
    tab = qs.length;
    reviewCursor = 0;
  }

  function submit(): void {
    if (sent) return;
    const a = buildAnswers(qs, picks);
    if (Object.keys(a).length === 0) return;
    sent = true;
    onanswer(a);
  }

  function activate(item: Item | undefined): void {
    if (!item || !q || sent) return;
    const p = picks[tab];
    switch (item.kind) {
      case "option":
        if (q.multi_select) {
          p.selected = p.selected.includes(item.index)
            ? p.selected.filter((i) => i !== item.index)
            : [...p.selected, item.index];
          p.done = false;
        } else {
          p.selected = [item.index];
          p.other = "";
          p.done = true;
          next();
        }
        break;
      case "other":
        if (p.other.trim().length === 0) {
          otherInput?.focus();
          return;
        }
        if (q.multi_select) {
          p.otherOn = !p.otherOn;
          p.done = false;
        } else {
          p.selected = [];
          p.done = true;
          next();
        }
        break;
      case "submit":
        if (answerOf(q, p) === null) return;
        p.done = true;
        next();
        break;
      case "chat":
        if (!question.canSkip) return;
        sent = true;
        onchat();
        break;
    }
  }

  function activateReview(): void {
    if (sent) return;
    if (reviewCursor === 0) submit();
    else if (question.canSkip) {
      sent = true;
      onskip();
    } else tab = 0;
  }

  function onkeydown(e: KeyboardEvent): void {
    if (e.isComposing || e.keyCode === 229) return;
    const plain = !e.ctrlKey && !e.altKey && !e.metaKey;
    if (!plain || sent) return;
    const inOther = e.target === otherInput;
    switch (e.key) {
      case "ArrowUp":
      case "ArrowDown":
        if (e.shiftKey) return;
        e.preventDefault();
        move(e.key === "ArrowUp" ? -1 : 1);
        return;
      case "Tab":
        if (!multi) return;
        e.preventDefault();
        goTab(tab + (e.shiftKey ? -1 : 1));
        return;
      case "ArrowLeft":
      case "ArrowRight":
        // Dans le champ libre non vide, ← → déplacent le curseur du texte.
        if (!multi || e.shiftKey || (inOther && (otherInput?.value.length ?? 0) > 0)) return;
        e.preventDefault();
        goTab(tab + (e.key === "ArrowLeft" ? -1 : 1));
        return;
      case "Enter":
        if (e.shiftKey) return;
        e.preventDefault();
        if (onReview) activateReview();
        else activate(current);
        return;
      case " ":
        if (inOther || !q?.multi_select || current?.kind !== "option") return;
        e.preventDefault();
        activate(current);
        return;
    }
    // Chiffre : choisit l'entrée de ce numéro (pas dans le champ libre).
    if (!inOther && !e.shiftKey && /^[1-9]$/.test(e.key)) {
      e.preventDefault();
      const i = Number(e.key) - 1;
      if (onReview) {
        if (i < 2) {
          reviewCursor = i;
          activateReview();
        }
        return;
      }
      if (i < items.length) {
        cursors[tab] = i;
        activate(items[i]);
      }
    }
  }

  function clickItem(i: number): void {
    cursors[tab] = i;
    activate(items[i]);
  }

  const mark = (on: boolean) => (on ? "[✔]" : "[ ]");
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<div
  class="card term"
  data-testid="question-card"
  data-request={question.requestId}
  role="dialog"
  aria-label="Question de Claude"
  tabindex="-1"
  bind:this={root}
  {onkeydown}
>
  <div class="rule" aria-hidden="true">{RULE}</div>
  <div class="tabs" data-testid="question-tabs">
    <!-- Espaces dans des expressions : le texte littéral serait replié. -->
    {#if multi}<span class="dim">{"←  "}</span>{/if}{#each qs as item, i (i)}<span
        class="pill"
        class:on={i === tab}
        data-testid="question-pill">{` ${picks[i]?.done ? "☒" : "☐"} ${item.header} `}</span
      >{#if multi}{"  "}{/if}{/each}{#if multi}<span class="pill" class:on={onReview}>{" ✔ Submit "}</span><span
        class="dim">{"  →"}</span
      >{/if}
  </div>
  <div class="blank"></div>
  {#if q}
    <div class="qtext" data-testid="question-text">{q.question}</div>
    <div class="blank"></div>
    {#each items as item, i (i)}
      {@const sel = i === cursor}
      {#if item.kind === "chat"}
        <div class="rule" aria-hidden="true">{RULE}</div>
      {/if}
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <div
        class="opt"
        class:sel
        class:off={item.kind === "chat" && !question.canSkip}
        role="option"
        aria-selected={sel}
        tabindex="-1"
        data-kind={item.kind}
        onclick={() => clickItem(i)}
      >
        {sel ? "❯" : " "}{" "}<span class="num" class:dim={item.kind !== "chat"}>{i + 1}.</span>{" "}{#if item.kind === "option"}{q.multi_select
            ? `${mark(pick?.selected.includes(item.index) ?? false)} `
            : ""}{q.options[item.index].label}{:else if item.kind === "other"}<span class="other"
            >{q.multi_select ? `${mark(pick?.otherOn ?? false)} ` : ""}<input
              bind:this={otherInput}
              bind:value={picks[tab].other}
              placeholder="Type something."
              spellcheck="false"
              autocomplete="off"
              aria-label="Autre réponse"
              size={Math.max(16, (picks[tab]?.other.length ?? 0) + 1)}
            /></span
          >{:else if item.kind === "submit"}{tab < qs.length - 1 ? "Next" : "Submit"}{:else}Chat about this{/if}
      </div>
      {#if item.kind === "option" && q.options[item.index].description}
        <div class="desc dim">{q.options[item.index].description}</div>
      {/if}
    {/each}
  {:else if onReview}
    <div class="b">Review your answers</div>
    <div class="blank"></div>
    {#each qs as item, i (i)}
      {@const a = answers[item.question]}
      <div class="review"><span class="dim">● </span>{item.question}</div>
      <div class="review-a" class:dim={a === undefined}>  → {a === undefined ? "(no answer)" : answerText(a)}</div>
    {/each}
    <div class="blank"></div>
    {#if Object.keys(answers).length < qs.length}
      <div class="warn">You have not answered all questions</div>
    {/if}
    <div>Ready to submit your answers?</div>
    <div class="blank"></div>
    {#each ["Submit answers", "Cancel"] as label, i (i)}
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <div
        class="opt"
        class:sel={i === reviewCursor}
        class:off={i === 1 && !question.canSkip}
        role="option"
        aria-selected={i === reviewCursor}
        tabindex="-1"
        onclick={() => {
          reviewCursor = i;
          activateReview();
        }}
      >
        {i === reviewCursor ? "❯" : " "} {i + 1}. {label}
      </div>
    {/each}
  {/if}
  <div class="blank"></div>
  <div class="dim foot" data-testid="question-footer">{footer}</div>
</div>

<style>
  .card {
    flex: none;
    outline: none;
    max-height: 70vh;
    overflow-y: auto;
    overflow-x: hidden;
  }
  /* Règles de la carte : #999999 sur `07` (celles de la saisie : #888888, `10`). */
  .rule {
    color: var(--t-dim);
    white-space: pre;
    overflow: hidden;
    height: var(--t-lh);
  }
  .blank {
    height: var(--t-lh);
  }
  .tabs {
    white-space: pre;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  /* Pastille de l'onglet actif : texte noir sur bleu lavande (`07`), avec
     une colonne de marge de chaque côté (« ☐ Langage » commence en colonne 1). */
  .pill {
    color: var(--t-dim);
  }
  .pill.on {
    color: #000000;
    background: var(--t-lavender);
  }
  /* Question : blanc pur, gras (#FFFFFF sur `07`). */
  .qtext {
    font-weight: bold;
    color: var(--t-white);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  /* « ❯ 1. » puis le label ; un label replié reprend en colonne 5. */
  .opt {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    padding-left: 5ch;
    text-indent: -5ch;
    color: var(--t-fg);
    cursor: default;
  }
  .opt.sel {
    color: var(--t-lavender);
  }
  .opt.off {
    color: var(--t-dim);
  }
  .desc {
    padding-left: 5ch;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .other input {
    background: transparent;
    color: inherit;
    caret-color: var(--t-fg);
    border: none;
    outline: none;
    padding: 0;
    margin: 0;
    font: inherit;
    line-height: var(--t-lh);
    max-width: 100%;
  }
  /* « Type something. » : gris (#999999 sur `07`), le texte tapé en clair. */
  .other input::placeholder {
    color: var(--t-dim);
    opacity: 1;
  }
  .opt.sel .other input::placeholder {
    color: var(--t-lavender);
  }
  /* Numéro des options : gris, même sur l'entrée choisie (`07`). */
  .opt .num.dim {
    color: var(--t-dim);
  }
  .review,
  .review-a {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .warn {
    color: var(--warn);
  }
</style>
