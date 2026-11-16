<script lang="ts">
  // Fenêtre principale. J3 : conversation virtualisée alimentée par le
  // magasin de session, soit par le cœur Rust (Tauri), soit par le mode
  // simulé (`?mock=…`, ou d'office hors de Tauri : `npm run dev:mock`).
  import { onMount } from "svelte";
  import Conversation from "./lib/components/Conversation.svelte";
  import {
    describeError,
    getLaunchInfo,
    sendMessage,
    startSession,
    type SessionKey,
  } from "./lib/core/client";
  import { MockCore, mockOptionsFromUrl } from "./lib/core/mock";
  import { SessionStore } from "./lib/state/session.svelte";

  const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
  const mockOpts = mockOptionsFromUrl(location.search) ?? (inTauri ? null : { name: "demo", rows: 60 });

  const store = new SessionStore();
  let mock: MockCore | null = null;
  let conversation: Conversation | undefined = $state();

  let session: SessionKey | null = $state(null);
  let cwd = $state("");
  let startError: string | null = $state(null);
  let input = $state("");
  let inputEl: HTMLInputElement | undefined = $state();

  // Latence d'un token : de `t` (lecture de la ligne) à la peinture. Le
  // message posté depuis l'image où le lot est appliqué s'exécute après
  // la peinture de cette image.
  const latencies: number[] = [];
  let p50 = $state("–");
  let p95 = $state("–");
  const afterPaint = new MessageChannel();
  const paintQueue: number[] = [];
  afterPaint.port1.onmessage = () => {
    const now = performance.timeOrigin + performance.now();
    const t = paintQueue.shift();
    if (t === undefined) return;
    latencies.push(now - t);
    if (latencies.length > 2000) latencies.splice(0, latencies.length - 2000);
  };
  store.onApplied = ({ lastDeltaT }) => {
    if (lastDeltaT === null) return;
    paintQueue.push(lastDeltaT);
    afterPaint.port2.postMessage(0);
  };

  function pct(xs: number[], p: number): number {
    const s = [...xs].sort((a, b) => a - b);
    return s[Math.min(s.length - 1, Math.floor((p / 100) * s.length))];
  }

  // Mise à jour du pied une fois par seconde, pas à chaque token.
  onMount(() => {
    const id = setInterval(() => {
      if (latencies.length === 0) return;
      p50 = pct(latencies, 50).toFixed(0);
      p95 = pct(latencies, 95).toFixed(0);
    }, 1000);
    return () => clearInterval(id);
  });

  const ready = $derived(mockOpts !== null || session !== null);

  $effect(() => {
    if (ready) inputEl?.focus();
  });

  async function send(e: SubmitEvent) {
    e.preventDefault();
    const text = input.trim();
    if (!text) return;
    input = "";
    // Envoyer un message ramène en bas, comme le terminal.
    conversation?.scrollToBottom();
    if (mock) {
      mock.send(text);
      return;
    }
    if (!session) return;
    try {
      await sendMessage(session, text);
    } catch (err) {
      store.banners.push({ level: "Error", code: "send", message: describeError(err), seq: -1 });
    }
  }

  onMount(() => {
    if (mockOpts) {
      mock = new MockCore(mockOpts);
      cwd = `simulé : ${mockOpts.name}`;
      mock.start(store.push);
      // Points d'accès des mesures e2e (mode simulé seulement).
      (window as unknown as Record<string, unknown>).__orch = {
        store,
        mock,
        conversation: () => conversation,
        latencies,
      };
      return;
    }
    (async () => {
      try {
        const info = await getLaunchInfo();
        cwd = info.cwd;
        const started = await startSession(info.cwd, store.push);
        // Un `exited` arrivé pendant le démarrage ne doit pas être écrasé.
        if (store.status !== "Exited") session = started.session;
      } catch (err) {
        startError = describeError(err);
      }
    })();
  });
</script>

<main>
  {#each store.banners as b (b.seq + b.code)}
    <div class="banner {b.level.toLowerCase()}">{b.message}</div>
  {/each}
  {#if startError}
    <div class="banner error">{startError}</div>
  {/if}
  <Conversation {store} bind:this={conversation} />
  <form onsubmit={send}>
    <span class="prompt">❯</span>
    <input
      bind:value={input}
      placeholder={ready ? "" : "session non démarrée"}
      disabled={!ready || store.status === "Exited"}
      bind:this={inputEl}
      aria-label="Message"
    />
  </form>
  <footer>
    <span>{cwd}</span>
    <span>{store.status} · {store.rows.length} lignes</span>
    <span>latence token p50 {p50} ms · p95 {p95} ms</span>
  </footer>
</main>

<style>
  main {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: #0c0c0c;
  }
  .banner {
    padding: 0 1ch;
    color: #0c0c0c;
    background: var(--warn);
  }
  .banner.error {
    background: var(--error);
  }
  form {
    display: grid;
    grid-template-columns: 2ch 1fr;
    border-top: 1px solid #555;
    border-bottom: 1px solid #555;
    padding: 0 1ch;
    line-height: 18px;
  }
  .prompt {
    color: #8b8b8b;
  }
  input {
    width: 100%;
    background: transparent;
    color: #cccccc;
    border: none;
    outline: none;
    font: inherit;
    padding: 0;
  }
  footer {
    display: flex;
    justify-content: space-between;
    gap: 2ch;
    padding: 0 1ch;
    color: #8b8b8b;
    font-size: 12px;
  }
</style>
