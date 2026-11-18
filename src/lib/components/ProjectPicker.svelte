<script lang="ts">
  // Sélecteur des projets récents (J11, décision 11) : ouvert quand l'app est
  // lancée sans dossier en argument (`LaunchInfo.cwd_from_args` faux). Liste
  // les sous-dossiers de `~/.claude/projects` avec leur vrai chemin (`cwd` du
  // transcript le plus récent), du plus récent au plus ancien ; un dossier
  // disparu est grisé et ne peut pas être choisi ; un chemin réseau n'est pas
  // testé (`exists_checked` faux) : il reste choisissable, « non vérifié ».
  // Style terminal : liste `❯`, sans cadre ni carte (décision 3).
  //
  // Clavier : saisie = filtre, ↑/↓, Entrée ouvre, Échap annule (si `oncancel`).
  import { onMount } from "svelte";
  import { ProjectList, projectLabel, relativeTime } from "../state/sessions.svelte";
  import type { ProjectSummary } from "../core/bindings/ProjectSummary";

  let {
    list = new ProjectList(),
    onpick,
    oncancel,
  }: {
    list?: ProjectList;
    onpick: (path: string, project: ProjectSummary) => void;
    oncancel?: () => void;
  } = $props();

  let input: HTMLInputElement | undefined = $state();
  let listEl: HTMLElement | undefined = $state();

  onMount(() => {
    input?.focus();
    if (list.items.length === 0 && !list.loading) void list.refresh();
  });

  function pick(p: ProjectSummary | null) {
    if (p && list.isEnabled(p) && p.path) onpick(p.path, p);
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
      list.move(e.key === "ArrowDown" ? 1 : -1);
      listEl?.querySelector(`[data-index="${list.selected}"]`)?.scrollIntoView({ block: "nearest" });
    } else if (e.key === "Enter") {
      e.preventDefault();
      pick(list.current);
    } else if (e.key === "Escape" && oncancel) {
      e.preventDefault();
      oncancel();
    }
  }
</script>

<section class="picker" data-testid="project-picker" aria-label="Projets récents">
  <div class="title">Ouvrir un projet récent</div>
  <div class="filter">
    <span class="caret">❯</span>
    <input
      bind:this={input}
      value={list.filter}
      oninput={(e) => list.setFilter(e.currentTarget.value)}
      {onkeydown}
      role="combobox"
      aria-expanded="true"
      aria-controls="project-listbox"
      aria-activedescendant={list.current ? `project-opt-${list.selected}` : undefined}
      placeholder="filtrer"
      aria-label="Filtrer les projets"
      spellcheck="false"
      autocomplete="off"
    />
  </div>
  <div class="rule"></div>
  <ul bind:this={listEl} id="project-listbox" role="listbox" aria-label="Projets">
    {#if list.loading}
      <li class="dim">Lecture de ~/.claude/projects…</li>
    {:else if list.error}
      <li class="err" data-testid="picker-error">{list.error}</li>
    {:else if list.visible.length === 0}
      <li class="dim">{list.items.length === 0 ? "Aucun projet dans ~/.claude/projects." : "Aucun projet ne correspond."}</li>
    {/if}
    {#each list.visible as p, i (p.encoded)}
      {@const enabled = list.isEnabled(p)}
      {@const sel = i === list.selected && enabled}
      <li
        role="option"
        aria-selected={sel}
        aria-disabled={!enabled}
        data-index={i}
        data-testid="project-row"
        class:sel
        class:gone={!enabled}
        id="project-opt-{i}"
        tabindex="-1"
        onmousemove={() => enabled && (list.selected = i)}
        onclick={() => pick(p)}
        onkeydown={(e) => activate(e, () => pick(p))}
      >
        <span class="mark">{sel ? "❯" : " "}</span>
        <span class="path">{projectLabel(p)}</span>
        <span class="info"
          >{#if !enabled}{p.path ? "introuvable · " : "chemin inconnu · "}{:else if !p.exists_checked}non vérifié (réseau) · {/if}{relativeTime(p.last_used)} · {p.session_count}
          session{p.session_count > 1 ? "s" : ""}</span
        >
      </li>
    {/each}
  </ul>
  <div class="rule"></div>
  <div class="foot">
    ↑/↓ naviguer · Entrée ouvrir{oncancel ? " · Échap annuler" : ""}
    {#if list.loadMs !== null}· {list.items.length} projets · {list.loadMs} ms{/if}
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
    border-top: 1px solid var(--rule);
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
  .path {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .info {
    color: var(--t-dim);
  }
  li.sel .mark,
  li.sel .path {
    color: var(--t-lavender);
  }
  li.gone .path,
  li.gone .info {
    /* Plus éteint que `--t-dim`, dans les deux thèmes. */
    color: var(--t-rule-detail);
  }
  li.gone .path {
    text-decoration: line-through;
  }
  .dim,
  .foot {
    color: var(--t-dim);
  }
  .err {
    color: var(--t-error);
  }
</style>
