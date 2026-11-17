<script module lang="ts">
  // Modèle pur de la palette (testé dans `e2e/unit/palette.test.ts`).
  import type { CommandInfo } from "../core/bindings/CommandInfo";

  /**
   * `disabled` : raison pour laquelle l'entrée est grisée (entrée locale de
   * l'app indisponible, `/ameliorer` sans clone par exemple) ; elle remplace
   * la description et l'entrée ne part pas.
   */
  export type PaletteEntry = {
    name: string;
    description: string;
    argumentHint: string | null;
    disabled?: string | null;
  };

  /**
   * Entrées : `ready.commands` (réponse à `initialize`, `t11` : commandes et
   * skills), puis les noms de `init.skills` et `init.slash_commands` absents
   * de `ready`, qui complètent la liste sans la vider (plan J10).
   */
  export function entriesFrom(commands: CommandInfo[], extra: Iterable<string> = []): PaletteEntry[] {
    const out: PaletteEntry[] = [];
    const seen = new Set<string>();
    for (const c of commands) {
      if (seen.has(c.name)) continue;
      seen.add(c.name);
      out.push({ name: c.name, description: c.description, argumentHint: c.argument_hint });
    }
    for (const raw of extra) {
      const name = raw.replace(/^\//, "");
      if (!name || seen.has(name)) continue;
      seen.add(name);
      out.push({ name, description: "", argumentHint: null });
    }
    return out;
  }

  /** `abc` présent dans l'ordre (pas forcément contigu) dans `text`. */
  function subsequence(q: string, text: string): boolean {
    let i = 0;
    for (const ch of text) if (ch === q[i] && ++i === q.length) return true;
    return q.length === 0;
  }

  /**
   * Filtre flou sur le nom et la description, sans casse. Rang : début du
   * nom (ou d'un segment après `:` / `-`), puis nom qui contient, puis lettres
   * du nom dans l'ordre, puis description qui contient. Ordre d'origine à
   * rang égal ; filtre vide : tout, dans l'ordre de `ready`.
   */
  export function filterEntries(entries: PaletteEntry[], query: string): PaletteEntry[] {
    const q = query.trim().toLowerCase();
    if (q === "") return entries;
    const ranked: { e: PaletteEntry; r: number; i: number }[] = [];
    entries.forEach((e, i) => {
      const n = e.name.toLowerCase();
      let r = -1;
      if (n.startsWith(q)) r = 0;
      else if (n.split(/[:\-_]/).some((s) => s.startsWith(q))) r = 1;
      else if (n.includes(q)) r = 2;
      else if (subsequence(q, n)) r = 3;
      else if (e.description.toLowerCase().includes(q)) r = 4;
      if (r >= 0) ranked.push({ e, r, i });
    });
    ranked.sort((a, b) => a.r - b.r || a.i - b.i);
    return ranked.map((x) => x.e);
  }

  /** Entrées visibles à la fois (`10` : 3, sans barre de défilement). */
  export const VISIBLE = 3;

  /** Fenêtre visible `[début, fin[` qui garde la sélection à l'écran. */
  export function windowOf(count: number, sel: number, size = VISIBLE): [number, number] {
    if (count <= size) return [0, count];
    const start = Math.min(Math.max(0, sel - size + 1), count - size);
    const s = sel < start ? sel : start;
    return [s, s + size];
  }

  /** Largeur de la colonne des noms (`ch`) : « /nom » le plus long + 3 (`10` : descriptions en colonne 50). */
  export function nameColumn(entries: PaletteEntry[]): number {
    let max = 0;
    for (const e of entries) max = Math.max(max, e.name.length + 1);
    return max + 3;
  }
</script>

<script lang="ts">
  // Palette (jalon J10), rendu de la capture `10` : au-dessus de la saisie,
  // sans cadre ; entrée choisie `❯ /nom` et sa description en bleu lavande,
  // les autres en gris ; descriptions alignées en colonne, repliées sur deux
  // lignes puis tronquées par `…`. Clavier dans `Prompt.svelte` (↑↓, Entrée,
  // Tab) ; Échap par le gestionnaire global (`keys.svelte.ts`).
  let {
    entries,
    sel,
    nameWidth,
    onchoose,
  }: {
    /** Entrées déjà filtrées. */
    entries: PaletteEntry[];
    sel: number;
    /** Colonne des noms, en `ch` (sur toutes les entrées, pour qu'elle ne bouge pas en filtrant). */
    nameWidth: number;
    onchoose: (index: number) => void;
  } = $props();

  const win = $derived(windowOf(entries.length, sel));
  const shown = $derived(entries.slice(win[0], win[1]));
</script>

<div
  class="palette term"
  data-testid="palette"
  role="listbox"
  aria-label="Commandes"
  style:--name-w={`${nameWidth}ch`}
>
  {#if entries.length === 0}
    <div class="row none dim">{"    No matching commands"}</div>
  {/if}
  {#each shown as e, k (e.name)}
    {@const i = win[0] + k}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div
      class="row"
      class:sel={i === sel}
      class:off={!!e.disabled}
      role="option"
      aria-selected={i === sel}
      aria-disabled={e.disabled ? "true" : undefined}
      tabindex="-1"
      data-name={e.name}
      onmousedown={(ev) => ev.preventDefault()}
      onclick={() => onchoose(i)}
    >
      <span class="cur">{i === sel ? "  ❯ " : "    "}</span>
      <span class="name">/{e.name}</span>
      <span class="desc">{e.disabled ? `(indisponible : ${e.disabled})` : e.description}</span>
    </div>
  {/each}
</div>

<style>
  .palette {
    flex: none;
  }
  /* « ❯ » en colonne 2, nom en colonne 4, description alignée ensuite. */
  .row {
    display: grid;
    grid-template-columns: 4ch min(var(--name-w), 45%) minmax(0, 1fr);
    color: var(--t-dim);
    cursor: default;
  }
  .row.none {
    display: block;
    white-space: pre;
  }
  .cur {
    white-space: pre;
  }
  .name {
    white-space: pre;
    overflow: hidden;
    text-overflow: ellipsis;
    padding-right: 1ch;
  }
  .desc {
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    overflow: hidden;
    overflow-wrap: anywhere;
    padding-right: 2ch;
  }
  .row.sel {
    color: var(--t-lavender);
  }
  .row.off .name,
  .row.off .desc {
    opacity: 0.55;
  }
</style>
