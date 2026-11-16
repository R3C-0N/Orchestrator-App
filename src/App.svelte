<script lang="ts">
  // J0 : tuyau de bout en bout. Texte brut, sans rendu Markdown ni liste
  // virtualisée (jalons suivants). Affiche aussi la latence des tokens.
  import { onMount, tick } from "svelte";
  import {
    describeError,
    getLaunchInfo,
    sendMessage,
    startSession,
    type CoreEvent,
    type SessionKey,
  } from "./lib/core/client";

  type Row =
    | { id: string; kind: "user"; text: string }
    | { id: string; kind: "assistant"; text: string; done: boolean }
    | { id: string; kind: "tool"; text: string }
    | { id: string; kind: "info"; text: string; level: "info" | "warn" | "error" };

  let rows: Row[] = $state([]);
  let session: SessionKey | null = $state(null);
  let cwd = $state("");
  let status = $state("démarrage…");
  let banner: string | null = $state(null);
  let input = $state("");
  let latencies: number[] = $state([]);
  let scroller: HTMLElement | undefined = $state();
  let inputEl: HTMLInputElement | undefined = $state();

  // Focus sur la saisie dès que la session est prête.
  $effect(() => {
    if (session) inputEl?.focus();
  });

  const byBlock = new Map<string, number>();

  function push(row: Row) {
    rows.push(row);
  }

  function blockKey(thread: string, msg: string, index: number) {
    return `${thread}/${msg}/${index}`;
  }

  function onEvents(batch: CoreEvent[]) {
    let sawDelta = false;
    let lastT = 0;
    for (const ev of batch) {
      switch (ev.kind) {
        case "starting":
          status = `claude.exe lancé (pid ${ev.pid})`;
          break;
        case "ready":
          status = `prêt : claude ${ev.version}, ${ev.commands.length} commandes`;
          break;
        case "status":
          status = ev.state;
          break;
        case "user_message":
          if (ev.origin !== "Replay" || ev.thread !== "main") {
            push({ id: `u${ev.seq}`, kind: "user", text: ev.text });
          }
          break;
        case "block_start":
          if (ev.block.type === "Text") {
            byBlock.set(blockKey(ev.thread, ev.msg, ev.index), rows.length);
            push({ id: `b${ev.seq}`, kind: "assistant", text: "", done: false });
          } else if (ev.block.type === "ToolUse") {
            push({ id: `b${ev.seq}`, kind: "tool", text: `● ${ev.block.name}` });
          }
          break;
        case "text_delta": {
          const i = byBlock.get(blockKey(ev.thread, ev.msg, ev.index));
          const r = i === undefined ? undefined : rows[i];
          if (r && r.kind === "assistant") r.text += ev.text;
          sawDelta = true;
          lastT = ev.t;
          break;
        }
        case "block_done": {
          if (ev.block.type !== "Text") break;
          const k = blockKey(ev.thread, ev.msg, ev.index);
          const i = byBlock.get(k);
          const r = i === undefined ? undefined : rows[i];
          // Le message complet fait foi et remplace le texte accumulé.
          if (r && r.kind === "assistant") {
            r.text = ev.block.text;
            r.done = true;
          } else {
            push({ id: `d${ev.seq}`, kind: "assistant", text: ev.block.text, done: true });
          }
          break;
        }
        case "tool_result":
          push({
            id: `r${ev.seq}`,
            kind: "tool",
            text: `  ⎿ ${ev.is_error ? "erreur" : "ok"} ${ev.content
              .map((p) => (p.type === "Text" ? p.text : `[${p.type}]`))
              .join(" ")
              .slice(0, 300)}`,
          });
          break;
        case "question":
          // J0 : simple relais ; la carte de question est un jalon ultérieur.
          push({
            id: `q${ev.seq}`,
            kind: "info",
            level: "warn",
            text: `Question (non gérée en J0) : ${ev.questions.map((q) => q.question).join(" / ")}`,
          });
          break;
        case "turn_result":
          push({
            id: `t${ev.seq}`,
            kind: "info",
            level: ev.is_error ? "error" : "info",
            text: `fin du tour : ${ev.subtype}, ${(ev.duration_ms / 1000).toFixed(1)} s, ${ev.cost_usd.toFixed(4)} $`,
          });
          break;
        case "notice":
          if (ev.code === "unexpected_version") banner = ev.message;
          push({
            id: `n${ev.seq}`,
            kind: "info",
            level: ev.level === "Warn" ? "warn" : ev.level === "Error" ? "error" : "info",
            text: `${ev.code} : ${ev.message}`,
          });
          break;
        case "exited":
          push({
            id: `x${ev.seq}`,
            kind: "info",
            level: "error",
            text: `claude.exe s'est arrêté (code ${ev.code ?? "?"})`,
          });
          session = null;
          break;
        default:
          break;
      }
    }
    if (sawDelta) {
      requestAnimationFrame(() => {
        const ms = performance.timeOrigin + performance.now() - lastT;
        latencies.push(ms);
        if (latencies.length > 500) latencies.shift();
      });
    }
    tick().then(() => scroller?.scrollTo({ top: scroller.scrollHeight }));
  }

  function pct(p: number): string {
    if (latencies.length === 0) return "–";
    const s = [...latencies].sort((a, b) => a - b);
    return s[Math.min(s.length - 1, Math.floor((p / 100) * s.length))].toFixed(0);
  }

  async function send(e: SubmitEvent) {
    e.preventDefault();
    const text = input.trim();
    if (!text || !session) return;
    input = "";
    try {
      await sendMessage(session, text);
    } catch (err) {
      push({ id: `e${Date.now()}`, kind: "info", level: "error", text: describeError(err) });
    }
  }

  onMount(async () => {
    try {
      const info = await getLaunchInfo();
      cwd = info.cwd;
      const started = await startSession(info.cwd, onEvents);
      session = started.session;
    } catch (err) {
      status = "échec";
      push({ id: "start", kind: "info", level: "error", text: describeError(err) });
    }
  });
</script>

<main>
  {#if banner}
    <div class="banner">{banner}</div>
  {/if}
  <div class="scroll" bind:this={scroller}>
    {#each rows as row (row.id)}
      {#if row.kind === "user"}
        <pre class="user">&gt; {row.text}</pre>
      {:else if row.kind === "assistant"}
        <pre class="assistant" class:streaming={!row.done}>{row.text}</pre>
      {:else if row.kind === "tool"}
        <pre class="tool">{row.text}</pre>
      {:else}
        <pre class="info {row.level}">{row.text}</pre>
      {/if}
    {/each}
  </div>
  <form onsubmit={send}>
    <input
      bind:value={input}
      placeholder={session ? "Message…" : "session non démarrée"}
      disabled={!session}
      bind:this={inputEl}
    />
  </form>
  <footer>
    <span>{cwd}</span>
    <span>{status}</span>
    <span>latence token p50 {pct(50)} ms · p95 {pct(95)} ms</span>
  </footer>
</main>

<style>
  main {
    display: flex;
    flex-direction: column;
    height: 100%;
  }
  .banner {
    background: var(--warn);
    color: #000;
    padding: 4px 12px;
  }
  .scroll {
    flex: 1;
    overflow-y: auto;
    padding: 12px;
  }
  pre {
    margin: 0 0 8px;
    white-space: pre-wrap;
    word-break: break-word;
    font: inherit;
  }
  .user {
    color: var(--dim);
  }
  .assistant.streaming::after {
    content: "▍";
    color: var(--accent);
  }
  .tool {
    color: var(--accent);
  }
  .info {
    color: var(--dim);
  }
  .info.warn {
    color: var(--warn);
  }
  .info.error {
    color: var(--error);
  }
  form {
    border-top: 1px solid var(--line);
    padding: 8px 12px;
  }
  input {
    width: 100%;
    background: transparent;
    color: var(--fg);
    border: none;
    outline: none;
    font: inherit;
  }
  footer {
    display: flex;
    justify-content: space-between;
    gap: 16px;
    padding: 4px 12px;
    color: var(--dim);
    border-top: 1px solid var(--line);
    font-size: 12px;
  }
</style>
