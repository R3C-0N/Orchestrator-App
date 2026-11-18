<script lang="ts">
  // Saisie (jalon J7), restylée d'après la maquette J15 : cadre arrondi sur
  // fond `--surface`, « ❯ » en dégradé, collée en bas ; indication à droite au-dessus
  // de la saisie (« Esc again to clear », `a*`), à la place de
  // « ◐ medium · /effort ». Texte d'invite gris tant qu'aucun message n'a été
  // envoyé (`01`), rien ensuite (`n*`).
  //
  // Clavier propre à la saisie (les raccourcis globaux sont dans
  // `state/keys.svelte.ts`, appliqués par `App.svelte`) :
  // - Entrée envoie ; Maj+Entrée insère un saut de ligne ; le collage garde
  //   les sauts de ligne (zone de texte qui grandit avec son contenu) ;
  // - ↑ sur la première ligne / ↓ sur la dernière : historique des messages ;
  // - `/` tapé dans une saisie vide : palette (J10) ; palette ouverte : ↑↓
  //   choisissent, Entrée envoie ou insère la commande, Tab l'insère ; sans
  //   correspondance, Entrée et Tab ne font rien ;
  // - sélecteur de modèle ouvert : ↑↓ choisit, Entrée valide (Échap le ferme,
  //   via le gestionnaire global).
  import type { ModelInfo } from "../core/bindings/ModelInfo";
  import Palette, { type PaletteEntry } from "./Palette.svelte";

  let {
    draft = $bindable(""),
    disabled = false,
    placeholder = "",
    hint = null,
    history = () => [],
    models = [],
    currentModel = null,
    pickerOpen = $bindable(false),
    onsubmit,
    onslash,
    onpick,
    oninputchange,
    palette = null,
    onpalettemove,
    onpalettechoose,
  }: {
    draft?: string;
    disabled?: boolean;
    placeholder?: string;
    hint?: string | null;
    /** Messages déjà envoyés, du plus ancien au plus récent. */
    history?: () => string[];
    models?: ModelInfo[];
    /** Modèle résolu en cours (`init.model`), pour la coche. */
    currentModel?: string | null;
    pickerOpen?: boolean;
    onsubmit: (text: string) => void;
    /** `/` tapé en début de saisie (palette, J10). */
    onslash?: () => void;
    onpick?: (model: ModelInfo) => void;
    oninputchange?: () => void;
    /** Palette ouverte (J10) : entrées filtrées, sélection, colonne des noms. */
    palette?: { entries: PaletteEntry[]; sel: number; nameWidth: number } | null;
    onpalettemove?: (d: number) => void;
    /** `enter` : envoie ou insère comme le TUI ; `tab` et `click` : insère. */
    onpalettechoose?: (index: number, how: "enter" | "tab" | "click") => void;
  } = $props();

  let el: HTMLTextAreaElement | undefined = $state();
  /** Position dans l'historique pendant une navigation ↑↓ (`null` hors navigation). */
  let histPos: number | null = null;
  let histList: string[] = [];
  /** Brouillon gardé quand on remonte l'historique. */
  let saved = "";
  let pick = $state(0);

  export function focus(): void {
    el?.focus();
  }

  /** Focus, curseur en fin de texte (après une commande insérée par la palette). */
  export function focusEnd(): void {
    el?.focus();
    queueMicrotask(() => {
      if (el) el.selectionStart = el.selectionEnd = el.value.length;
    });
  }

  export function element(): HTMLTextAreaElement | undefined {
    return el;
  }

  /** Quitte la navigation dans l'historique (saisie effacée par un raccourci). */
  export function resetHistory(): void {
    histPos = null;
  }

  /** Texte sélectionné dans la zone de saisie. */
  export function hasSelection(): boolean {
    return !!el && el.selectionStart !== el.selectionEnd;
  }

  $effect(() => {
    if (pickerOpen) {
      const i = models.findIndex((m) => (m.resolved ?? m.value) === currentModel);
      pick = i >= 0 ? i : 0;
    }
  });

  function caretOnFirstLine(t: HTMLTextAreaElement): boolean {
    return t.selectionStart === t.selectionEnd && !t.value.slice(0, t.selectionStart).includes("\n");
  }
  function caretOnLastLine(t: HTMLTextAreaElement): boolean {
    return t.selectionStart === t.selectionEnd && !t.value.slice(t.selectionEnd).includes("\n");
  }

  function setDraft(text: string): void {
    draft = text;
    queueMicrotask(() => {
      if (el) el.selectionStart = el.selectionEnd = el.value.length;
    });
  }

  function onkeydown(e: KeyboardEvent): void {
    const plain = !e.ctrlKey && !e.altKey && !e.metaKey;
    // Composition IME : Entrée et les flèches appartiennent à l'IME.
    if (e.isComposing || e.keyCode === 229) return;
    // Palette ouverte sans correspondance (Ctrl+K) : Entrée et Tab ne font
    // rien ; le filtre tapé n'est jamais envoyé comme message.
    if (palette && plain && !e.shiftKey && palette.entries.length === 0 && (e.key === "Enter" || e.key === "Tab")) {
      e.preventDefault();
      return;
    }
    if (palette && plain && !e.shiftKey && palette.entries.length > 0) {
      if (e.key === "ArrowUp" || e.key === "ArrowDown") {
        e.preventDefault();
        onpalettemove?.(e.key === "ArrowUp" ? -1 : 1);
        return;
      }
      if (e.key === "Enter" || e.key === "Tab") {
        e.preventDefault();
        onpalettechoose?.(palette.sel, e.key === "Enter" ? "enter" : "tab");
        return;
      }
    }
    if (pickerOpen && plain && !e.shiftKey && models.length > 0) {
      if (e.key === "ArrowUp" || e.key === "ArrowDown") {
        e.preventDefault();
        const d = e.key === "ArrowUp" ? -1 : 1;
        pick = (pick + d + models.length) % models.length;
        return;
      }
      if (e.key === "Enter") {
        e.preventDefault();
        onpick?.(models[pick]);
        return;
      }
    }
    if (e.key === "Enter" && plain && !e.shiftKey) {
      e.preventDefault();
      const text = draft;
      if (text.trim().length === 0) return;
      histPos = null;
      onsubmit(text);
      return;
    }
    if (!plain || e.shiftKey || !el) return;
    if (e.key === "ArrowUp" && caretOnFirstLine(el)) {
      if (histPos === null) {
        histList = history();
        if (histList.length === 0) return;
        saved = draft;
        histPos = histList.length;
      }
      if (histPos === 0) return;
      e.preventDefault();
      histPos--;
      setDraft(histList[histPos]);
    } else if (e.key === "ArrowDown" && histPos !== null && caretOnLastLine(el)) {
      e.preventDefault();
      histPos++;
      if (histPos >= histList.length) {
        histPos = null;
        setDraft(saved);
      } else setDraft(histList[histPos]);
    }
  }

  function oninput(): void {
    histPos = null;
    oninputchange?.();
    if (draft === "/") onslash?.();
  }
</script>

<div class="prompt term" data-testid="prompt">
  {#if pickerOpen}
    <div class="picker" data-testid="model-picker" role="listbox" aria-label="Modèle">
      <div class="b">Select model</div>
      {#if models.length === 0}
        <div class="dim">  Aucun modèle annoncé par le CLI (réponse à initialize).</div>
      {/if}
      {#each models as m, i (m.value)}
        <div class="opt" class:sel={i === pick} role="option" aria-selected={i === pick} data-value={m.value}>
          <span>{i === pick ? "❯" : " "} {i + 1}. {m.display_name ?? m.value}{(m.resolved ?? m.value) ===
            currentModel
              ? " ✔"
              : ""}</span>
          <span class="dim">  {m.resolved ?? m.value}</span>
        </div>
      {/each}
      <div class="dim">Enter to select · ↑/↓ to navigate · Esc to cancel</div>
    </div>
  {/if}
  {#if palette}
    <!-- Menu `/` au-dessus de la saisie, à la place de l'indication (`10`). -->
    <Palette
      entries={palette.entries}
      sel={palette.sel}
      nameWidth={palette.nameWidth}
      onchoose={(i) => onpalettechoose?.(i, "click")}
    />
  {:else}
    <div class="hint dim" data-testid="prompt-hint">{hint ?? ""}</div>
  {/if}
  <div class="line">
    <span class="g caret grad">❯</span>
    <textarea
      bind:this={el}
      bind:value={draft}
      {onkeydown}
      {oninput}
      {placeholder}
      {disabled}
      rows="1"
      spellcheck="false"
      autocomplete="off"
      aria-label="Message"
    ></textarea>
  </div>
</div>

<style>
  .prompt {
    flex: none;
    padding: 0 14px;
  }
  .hint {
    text-align: right;
    padding-right: 1ch;
    white-space: pre;
    height: var(--t-lh);
    overflow: hidden;
  }
  .line {
    display: grid;
    grid-template-columns: 2ch minmax(0, 1fr);
    border: 1px solid var(--rule);
    border-radius: 8px;
    background: var(--surface);
    padding: 10px 12px;
  }
  textarea {
    display: block;
    width: 100%;
    min-height: var(--t-lh);
    max-height: calc(var(--t-lh) * 12);
    field-sizing: content;
    resize: none;
    overflow-y: auto;
    background: transparent;
    color: var(--strong);
    caret-color: var(--strong);
    border: none;
    outline: none;
    padding: 0;
    margin: 0;
    font: inherit;
    line-height: var(--t-lh);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  textarea::placeholder {
    color: var(--t-dim);
    opacity: 1;
  }
  textarea:disabled {
    color: var(--t-dim);
  }
  .picker {
    padding: 0 1ch 0 2ch;
  }
  .picker .opt {
    white-space: pre;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .picker .opt.sel > span:first-child {
    color: var(--t-lavender);
  }
</style>
