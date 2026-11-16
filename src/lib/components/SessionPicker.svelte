<script lang="ts">
  // Sélecteur des sessions d'un projet (J11, décision 9) : « nouvelle
  // session » en tête, puis les sessions du dossier, de la plus récente à la
  // plus ancienne (titre `ai-title` ou `/rename`, sinon dernier prompt pour
  // une session `-p`). Choisir une session lance la reprise avec tout son
  // historique (`sessions.svelte.ts`, `resumeSession`). Style terminal.
  //
  // Clavier : saisie = filtre, ↑/↓, Entrée choisit, Échap annule (si `oncancel`).
  import { onMount } from "svelte";
  import { SessionList, formatSize, relativeTime, sessionLabel } from "../state/sessions.svelte";
  import type { SessionSummary } from "../core/bindings/SessionSummary";

  let {
    cwd,
    list = new SessionList(cwd),
    onpick,
    onnew,
    oncancel,
  }: {
    cwd: string;
    list?: SessionList;
    /** Reprendre la session `id` (UUID, seule forme acceptée par `resume`, spec 4.6). */
    onpick: (id: string, session: SessionSummary) => void;
    /** Nouvelle session dans `cwd` (entrée « nouvelle session »). */
    onnew?: () => void;
    oncancel?: () => void;
  } = $props();

  let input: HTMLInputElement | undefined = $state();
  let listEl: HTMLElement | undefined = $state();
  /** -1 = « nouvelle session » (si `onnew`), sinon index dans `list.visible`. */
  let cursor = $state(0);
  const hasNew = $derived(onnew !== undefined && list.filter.trim() === "");
  const first = $derived(hasNew ? -1 : 0);

  onMount(() => {
    input?.focus();
    if (list.items.length === 0 && !list.loading) void list.refresh();
  });

  $effect(() => {
    // Nouvelle liste ou nouveau filtre : on revient en tête.
    void list.visible;
    cursor = first;
  });

  function choose() {
    if (cursor === -1) onnew?.();
    else {
      const s = list.visible[cursor];
      if (s) onpick(s.id, s);
    }
  }

  /** Entrée ou Espace sur une option qui a le focus (clic au clavier). */
  function activate(e: KeyboardEvent, run: () => void) {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      run();
    } else onkeydown(e);
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const d = e.key === "ArrowDown" ? 1 : -1;
      cursor = Math.min(list.visible.length - 1, Math.max(first, cursor + d));
      list.selected = Math.max(0, cursor);
      listEl?.querySelector(`[data-index="${cursor}"]`)?.scrollIntoView({ block: "nearest" });
    } else if (e.key === "Enter") {
      e.preventDefault();
      choose();
    } else if (e.key === "Escape" && oncancel) {
      e.preventDefault();
      oncancel();
    }
  }
</script>

<section class="picker" data-testid="session-picker" aria-label="Sessions du projet">
  <div class="title">Reprendre une session <span class="cwd">{cwd}</span></div>
  <div class="filter">
    <span class="caret">❯</span>
    <input
      bind:this={input}
      value={list.filter}
      oninput={(e) => list.setFilter(e.currentTarget.value)}
      {onkeydown}
      role="combobox"
      aria-expanded="true"
      aria-controls="session-listbox"
      aria-activedescendant={hasNew || list.visible.length > 0 ? `session-opt-${cursor}` : undefined}
      placeholder="filtrer"
      aria-label="Filtrer les sessions"
      spellcheck="false"
      autocomplete="off"
    />
  </div>
  <div class="rule"></div>
  <ul bind:this={listEl} id="session-listbox" role="listbox" aria-label="Sessions">
    {#if hasNew}
      <li
        role="option"
        aria-selected={cursor === -1}
        data-index={-1}
        data-testid="session-new"
        class:sel={cursor === -1}
        id="session-opt--1"
        tabindex="-1"
        onmousemove={() => (cursor = -1)}
        onclick={() => onnew?.()}
        onkeydown={(e) => activate(e, () => onnew?.())}
      >
        <span class="mark">{cursor === -1 ? "❯" : " "}</span>
        <span class="label">Nouvelle session</span>
        <span class="info"></span>
      </li>
    {/if}
    {#if list.loading}
      <li class="dim"><span></span><span>Lecture des transcripts…</span></li>
    {:else if list.error}
      <li class="err" data-testid="picker-error"><span></span><span>{list.error}</span></li>
    {:else if list.visible.length === 0}
      <li class="dim">
        <span></span><span>{list.items.length === 0 ? "Aucune session pour ce dossier." : "Aucune session ne correspond."}</span>
      </li>
    {/if}
    {#each list.visible as s, i (s.id)}
      <li
        role="option"
        aria-selected={i === cursor}
        data-index={i}
        data-testid="session-row"
        data-session-id={s.id}
        class:sel={i === cursor}
        id="session-opt-{i}"
        tabindex="-1"
        onmousemove={() => (cursor = i)}
        onclick={() => onpick(s.id, s)}
        onkeydown={(e) => activate(e, () => onpick(s.id, s))}
      >
        <span class="mark">{i === cursor ? "❯" : " "}</span>
        <span class="label" class:untitled={!s.title}>{sessionLabel(s)}</span>
        <span class="info"
          >{relativeTime(s.modified)} · {formatSize(s.size_bytes)}{s.entrypoint && s.entrypoint !== "cli"
            ? ` · ${s.entrypoint}`
            : ""}</span
        >
      </li>
    {/each}
  </ul>
  <div class="rule"></div>
  <div class="foot">
    ↑/↓ naviguer · Entrée {cursor === -1 ? "démarrer" : "reprendre"}{oncancel ? " · Échap annuler" : ""}
    {#if list.loadMs !== null}· {list.items.length} sessions · {list.loadMs} ms{/if}
  </div>
</section>

<style>
  .picker {
    display: flex;
    flex-direction: column;
    min-height: 0;
    flex: 1;
    padding: 0 1ch;
    background: var(--t-bg);
    color: var(--t-fg);
    line-height: 18px;
  }
  .title {
    color: var(--t-bold);
    font-weight: bold;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .cwd {
    color: var(--t-dim);
    font-weight: normal;
  }
  .filter {
    display: grid;
    grid-template-columns: 2ch 1fr;
  }
  .caret {
    color: var(--t-dim);
  }
  input {
    background: transparent;
    border: none;
    outline: none;
    color: var(--t-fg);
    font: inherit;
    padding: 0;
  }
  input::placeholder {
    color: var(--t-dim);
  }
  .rule {
    border-top: 1px solid #555;
    margin: 2px 0;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    overflow-y: auto;
    min-height: 0;
    flex: 1;
  }
  li {
    display: grid;
    grid-template-columns: 2ch minmax(0, 1fr) auto;
    gap: 0 2ch;
    white-space: nowrap;
    cursor: default;
  }
  .label {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .label.untitled {
    color: var(--t-dim);
  }
  .info {
    color: var(--t-dim);
  }
  li.sel .mark,
  li.sel .label {
    color: var(--t-lavender);
  }
  .dim,
  .foot {
    color: var(--t-dim);
  }
  .err {
    color: var(--t-error);
  }
</style>
