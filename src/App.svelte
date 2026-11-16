<script lang="ts">
  // Fenêtre principale. J3 : conversation virtualisée alimentée par le
  // magasin de session, soit par le cœur Rust (Tauri), soit par le mode
  // simulé (`?mock=…`, ou d'office hors de Tauri : `npm run dev:mock`).
  import { onMount } from "svelte";
  import Conversation from "./lib/components/Conversation.svelte";
  import StatusLine from "./lib/components/StatusLine.svelte";
  import {
    describeError,
    getLaunchInfo,
    sendMessage,
    startSession,
    type SessionKey,
  } from "./lib/core/client";
  import type { MockCore } from "./lib/core/mock";
  import { SessionStore } from "./lib/state/session.svelte";

  // Mode simulé : `?mock=…`, ou d'office hors de Tauri. `mock.ts` n'est
  // chargé qu'à ce moment (import dynamique, hors du bundle principal).
  const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
  const mockMode = new URLSearchParams(location.search).has("mock") || !inTauri;

  const store = new SessionStore();
  let mock: MockCore | null = null;
  let conversation: Conversation | undefined = $state();
  /** Vue détaillée (Ctrl+O, jalon J5) : un appel par ligne, sorties complètes. */
  let verbose = $state(false);

  function onWindowKey(e: KeyboardEvent) {
    if (e.ctrlKey && !e.altKey && !e.shiftKey && !e.metaKey && (e.key === "o" || e.key === "O")) {
      e.preventDefault();
      verbose = !verbose;
    }
  }

  let session: SessionKey | null = $state(null);
  let cwd = $state("");
  let startError: string | null = $state(null);
  let input = $state("");
  let inputEl: HTMLInputElement | undefined = $state();

  // Latence d'un token : de `t` (lecture de la ligne) à la peinture, pour
  // chaque `text_delta`. Le message posté depuis l'image où le lot est
  // appliqué s'exécute après la peinture de cette image.
  const latencies: number[] = [];
  let p50 = $state("–");
  let p95 = $state("–");
  const afterPaint = new MessageChannel();
  afterPaint.port1.onmessage = () => {
    const now = performance.timeOrigin + performance.now();
    const ts = paintQueue.shift();
    if (ts === undefined) return;
    for (const t of ts) latencies.push(now - t);
    if (latencies.length > 5000) latencies.splice(0, latencies.length - 5000);
  };
  const paintQueue: number[][] = [];
  store.onApplied = ({ deltaTs }) => {
    if (deltaTs.length === 0) return;
    paintQueue.push(deltaTs);
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

  let mockReady = $state(false);
  const ready = $derived(mockReady || session !== null);

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
      store.addBanner("Error", "send", describeError(err));
    }
  }

  async function startMock() {
    const { MockCore, mockOptionsFromUrl } = await import("./lib/core/mock");
    const opts = mockOptionsFromUrl(location.search) ?? { name: "demo", rows: 60 };
    mock = new MockCore(opts);
    cwd = `simulé : ${opts.name}`;
    mock.start(store.push);
    mockReady = true;
    // Points d'accès des mesures e2e (mode simulé seulement).
    (window as unknown as Record<string, unknown>).__orch = {
      store,
      mock,
      conversation: () => conversation,
      latencies,
    };
  }

  onMount(() => {
    if (mockMode) {
      startMock().catch((err) => (startError = String(err)));
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

<svelte:window onkeydown={onWindowKey} />

<main>
  {#each store.banners as b (b.id)}
    <div class="banner {b.level.toLowerCase()}">{b.message}</div>
  {/each}
  {#if startError}
    <div class="banner error">{startError}</div>
  {/if}
  <Conversation {store} {verbose} bind:this={conversation} />
  <!-- Vue détaillée (Ctrl+O) : la barre « Showing detailed transcript » de la
       conversation remplace la saisie et la ligne d'état, comme le TUI (03b). -->
  {#if !verbose}
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
  <StatusLine {store} />
  {/if}
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
