<script lang="ts">
  // Un appel d'outil sur sa propre ligne : Edit et Write en vue normale
  // (`04`, `09`), tout appel en vue détaillée (`03b`, `09b`).
  // « ● Nom(argument) » : puce verte si réussi, rouge en erreur, grise en cours.
  import { BULLET } from "../../render/glyphs";
  import { seconds as clock } from "../../render/clock.svelte";
  import {
    bashCommand,
    errorText,
    lineCount,
    relPath,
    runningLabel,
    seconds,
    shortResult,
    toolArg,
    toolTitle,
    truncateLines,
  } from "../../render/tools";
  import type { ToolCallRow } from "../../state/session.svelte";
  import EditDiff from "./EditDiff.svelte";
  import ToolResult from "./ToolResult.svelte";

  let {
    row,
    verbose = false,
    cwd = null,
    noResult = false,
  }: { row: ToolCallRow; verbose?: boolean; cwd?: string | null; noResult?: boolean } = $props();

  const isEdit = $derived(row.name === "Edit" || row.name === "MultiEdit");
  const isWrite = $derived(row.name === "Write");
  const arg = $derived(toolArg(row, cwd, verbose));
  const isPath = $derived(["Read", "Edit", "MultiEdit", "Write", "NotebookEdit"].includes(row.name));
  const st = $derived(
    noResult ? "none" : row.result === null ? "running" : row.result.isError ? "error" : "ok",
  );

  /** Sortie à afficher (vue normale : 3 lignes au plus). */
  function output(text: string, max: number) {
    if (!text.trim()) return { lines: ["(No content)"], more: 0 };
    return verbose ? { lines: text.replace(/\n+$/, "").split("\n"), more: 0 } : truncateLines(text, max);
  }

  const writeInfo = $derived.by(() => {
    if (!isWrite) return null;
    const inp = row.input as Record<string, unknown> | null;
    const content = inp && typeof inp.content === "string" ? inp.content : "";
    const path = inp && typeof inp.file_path === "string" ? inp.file_path : "";
    const n = lineCount(content);
    const all = content.replace(/\n$/, "").split("\n");
    const shown = verbose ? all : all.slice(0, 10);
    const w = String(all.length).length + 1;
    return {
      head: `Wrote <b>${n}</b> ${n === 1 ? "line" : "lines"} to <b>${escape(verbose ? path : relPath(path, cwd))}</b>`,
      lines: shown.map((l, i) => `${String(i + 1).padStart(w)} ${escape(l)}`),
      more: all.length - shown.length,
    };
  });

  function escape(s: string): string {
    return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  }
</script>

<div class="l">
  <span class="g {st === 'ok' ? 'ok' : st === 'error' ? 'err' : 'dim'}">{BULLET}</span>
  <span class="x"><span class="b">{toolTitle(row.name)}</span>{#if arg}(<span class:link={isPath}>{arg}</span
      >){/if}</span>
</div>
{#if st === "none"}
  {#if bashCommand(row) !== null}
    <ToolResult lines={[`$ ${bashCommand(row)}`]} tone="dim" />
  {/if}
{:else if st === "running"}
  <ToolResult lines={[`${runningLabel(row)} · ${seconds(clock.now - row.at)}`]} tone="dim" />
{:else if st === "error" && row.result}
  {#if !verbose && isEdit}
    <ToolResult lines={["Error editing file"]} tone="err" />
  {:else if !verbose && isWrite}
    <ToolResult lines={["Error writing file"]} tone="err" />
  {:else}
    {@const o = output(errorText(row.result.text), 3)}
    <ToolResult lines={o.lines} more={o.more} tone="err" />
  {/if}
{:else if row.result}
  {#if isEdit}
    <EditDiff {row} />
  {:else if writeInfo}
    <ToolResult lines={[writeInfo.head, ...writeInfo.lines]} more={writeInfo.more} html />
  {:else if row.name === "Read" || row.name === "Glob" || row.name === "Grep"}
    <ToolResult lines={[shortResult(row)]} />
  {:else}
    {@const o = output(row.result.text, 3)}
    <ToolResult lines={o.lines} more={o.more} />
  {/if}
{/if}
