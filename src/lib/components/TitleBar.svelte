<script lang="ts">
  // Barre de titre de la maquette « Orchestrator » (spec, décision 20) : la
  // fenêtre est sans décorations natives (`decorations: false` dans
  // `tauri.conf.json`), cette barre les remplace. À gauche la pastille, le
  // nom de l'app et la version de claude.exe ; à droite Réduire, Agrandir et
  // Fermer. Toute la barre déplace la fenêtre (`data-tauri-drag-region="deep"`,
  // les boutons restent cliquables) et le double-clic l'agrandit, par le
  // script de Tauri. Permissions : `capabilities/default.json`.
  // Hors de Tauri (mode simulé, e2e) ou si l'API de fenêtre échoue, les
  // boutons ne font rien.
  import { getCurrentWindow } from "@tauri-apps/api/window";

  let { version }: { version: string | null } = $props();

  const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

  /** Fenêtre agrandie : l'icône du bouton du milieu devient « Restaurer ». */
  let maximized = $state(false);

  /**
   * Fenêtre courante, ou `null` si l'API ne la donne pas. `getCurrentWindow`
   * lit `__TAURI_INTERNALS__.metadata`, absent quand l'IPC est simulé par
   * `mockIPC` (banc `e2e/startup.harness.ts`) : il lève alors. Une erreur
   * de l'API ne doit jamais empêcher l'app de démarrer.
   */
  function currentWindow(): ReturnType<typeof getCurrentWindow> | null {
    if (!inTauri) return null;
    try {
      return getCurrentWindow();
    } catch {
      return null;
    }
  }

  $effect(() => {
    const win = currentWindow();
    if (!win) return;
    let stop: (() => void) | null = null;
    let gone = false;
    const sync = () => {
      try {
        win.isMaximized().then(
          (m) => (maximized = m),
          () => {},
        );
      } catch {
        // État inconnu : l'icône reste « Agrandir ».
      }
    };
    sync();
    try {
      win.onResized(sync).then(
        (un) => (gone ? un() : (stop = un)),
        () => {},
      );
    } catch {
      // Sans suivi du redimensionnement, l'icône n'est juste plus mise à jour.
    }
    return () => {
      gone = true;
      stop?.();
    };
  });

  /** Commande de fenêtre ; une erreur (permission, fenêtre fermée, API absente) est ignorée. */
  function act(cmd: "minimize" | "toggleMaximize" | "close"): void {
    const win = currentWindow();
    if (!win) return;
    try {
      win[cmd]().catch(() => {});
    } catch {
      // Rien : le bouton est sans effet.
    }
  }

  function onkeydown(e: KeyboardEvent, cmd: "minimize" | "toggleMaximize" | "close"): void {
    // Entrée et Espace agissent ici : le gestionnaire global renverrait la
    // frappe vers la saisie avant que le bouton ne la reçoive (comme
    // `VersionNotice`). Avec un modificateur, la touche continue vers les
    // raccourcis globaux.
    if (e.key !== "Enter" && e.key !== " ") return;
    if (e.ctrlKey || e.altKey || e.metaKey || e.shiftKey) {
      e.preventDefault();
      return;
    }
    e.preventDefault();
    e.stopPropagation();
    act(cmd);
  }
</script>

<header class="title-bar" data-testid="title-bar" data-tauri-drag-region="deep">
  <div class="name">
    <span class="dot" aria-hidden="true"></span>
    <span class="app">orchestrator</span>
    {#if version}<span class="version" data-testid="title-bar-version">— claude {version}</span>{/if}
  </div>
  <div class="buttons">
    <button
      type="button"
      aria-label="Réduire"
      title="Réduire"
      onclick={() => act("minimize")}
      onkeydown={(e) => onkeydown(e, "minimize")}
    >
      <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M0 5h10" /></svg>
    </button>
    <button
      type="button"
      aria-label={maximized ? "Restaurer" : "Agrandir"}
      title={maximized ? "Restaurer" : "Agrandir"}
      onclick={() => act("toggleMaximize")}
      onkeydown={(e) => onkeydown(e, "toggleMaximize")}
    >
      {#if maximized}
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <rect x="0.5" y="2.5" width="7" height="7" rx="1.5" fill="none" />
          <path d="M2.5 2.5V2a1.5 1.5 0 0 1 1.5-1.5h4A1.5 1.5 0 0 1 9.5 2v4A1.5 1.5 0 0 1 8 7.5h-.5" fill="none" />
        </svg>
      {:else}
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <rect x="0.5" y="0.5" width="9" height="9" rx="1.5" fill="none" />
        </svg>
      {/if}
    </button>
    <button
      type="button"
      class="close"
      aria-label="Fermer"
      title="Fermer"
      onclick={() => act("close")}
      onkeydown={(e) => onkeydown(e, "close")}
    >
      <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M0.5 0.5l9 9M9.5 0.5l-9 9" /></svg>
    </button>
  </div>
</header>

<style>
  .title-bar {
    height: 36px;
    flex: 0 0 auto;
    display: flex;
    align-items: center;
    justify-content: space-between;
    background: var(--bar);
    font-size: 12px;
    user-select: none;
  }
  .name {
    display: flex;
    align-items: center;
    gap: 10px;
    padding-left: 14px;
    min-width: 0;
    white-space: nowrap;
  }
  .dot {
    width: 10px;
    height: 10px;
    flex: 0 0 auto;
    border-radius: 50%;
    background: var(--accent-grad);
  }
  .app {
    color: var(--fg);
    font-weight: 500;
  }
  .version {
    color: var(--dim);
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .buttons {
    display: flex;
    height: 100%;
  }
  button {
    width: 46px;
    height: 100%;
    border: 0;
    padding: 0;
    background: transparent;
    color: var(--fg);
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
  }
  button:hover {
    background: var(--surface);
  }
  /* Fermer : rouge façon Windows au survol. */
  button.close:hover {
    background: #c42b1c;
    color: #ffffff;
  }
  button:focus-visible {
    outline: 1px solid var(--accent);
    outline-offset: -2px;
  }
  svg {
    stroke: currentColor;
    stroke-width: 1;
  }
</style>
