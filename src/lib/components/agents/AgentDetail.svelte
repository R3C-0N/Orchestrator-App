<script lang="ts">
  // Conversation d'un sous-agent dans la colonne (maquette « Orchestrator »
  // J15, vue détaillée) : un clic sur un agent remplace la liste par cette
  // vue, le bouton « Agents » y revient.
  //   [< Agents]
  //   ● [agent-bleu]  Ping bleu agent
  //     En cours · 6s · ↓ 38.9k tokens
  //   ──────────────────────────────
  //   ❯ consigne de l'agent                    (bandeau)
  //     ⎿ Read(a.txt)  résultat
  //   ● texte de l'agent
  //   ◆ Question en attente …  /  ✻ Read…  /  ✓ Terminé en 42s
  //
  // Écarts assumés avec la maquette : l'app ne demande jamais de permission
  // (décision 7), l'encadré d'attente montre donc la question posée par
  // l'agent, à laquelle on répond dans la carte de question ; la saisie
  // « Message à l'agent » n'est pas reprise, le CLI n'offre aucun moyen
  // d'écrire à un sous-agent en cours.
  import type { AgentEvent } from "../../core/bindings/AgentEvent";
  import { agentColor, agentName } from "../../render/colors";
  import { BULLET, DOT, ELBOW, PROMPT, TURN_DONE } from "../../render/glyphs";
  import { seconds } from "../../render/tools";
  import type { Question, Row } from "../../state/session.svelte";
  import {
    WAITING_GLYPH,
    agentDetail,
    agentElapsed,
    agentGlyph,
    agentStateLabel,
    agentStats,
    isLive,
    threadLines,
  } from "./agentView";

  let {
    agent,
    now,
    waiting = false,
    question = null,
    rows = [],
    cwd = null,
    onback,
  }: {
    /** État complet de l'agent (dernier `AgentEvent` reçu). */
    agent: AgentEvent;
    /** Horloge (ms epoch) pour la durée d'un agent en cours. */
    now: number;
    /** Une question de son fil attend la réponse de l'utilisateur. */
    waiting?: boolean;
    /** La plus ancienne question de son fil, s'il y en a une. */
    question?: Question | null;
    /** Lignes du fil de l'agent (`SessionStore.threads`). */
    rows?: readonly Row[];
    /** Dossier de travail (chemins relatifs du fil). */
    cwd?: string | null;
    /** Retour à la liste des agents. */
    onback: () => void;
  } = $props();

  const st = $derived(agentGlyph(agent.status, waiting));
  const stateLabel = $derived(agentStateLabel(agent.status, waiting));
  const stats = $derived(agentStats(agent, now));
  const detail = $derived(agentDetail(agent, waiting));
  const lines = $derived(threadLines(rows, cwd));
  // La consigne va dans le bandeau du haut ; à défaut, la description.
  const prompt = $derived(lines.find((l) => l.kind === "prompt"));
  // L'appel `AskUserQuestion` en suspens est montré par l'encadré d'attente.
  const body = $derived(
    lines.filter((l) => l !== prompt && !(l.kind === "tool" && l.title === "AskUserQuestion" && l.result === null)),
  );
  const statParts = $derived(stats ? stats.split(` ${DOT} `) : []);
  const elapsed = $derived(agentElapsed(agent, now));
  const live = $derived(isLive(agent));

  /** Entrée et Espace : la fenêtre renverrait l'Espace vers la saisie (cf. `AgentColumn`). */
  function backKey(e: KeyboardEvent): void {
    if (e.key !== "Enter" && e.key !== " ") return;
    e.preventDefault();
    if (e.ctrlKey || e.altKey || e.metaKey || e.shiftKey) return;
    e.stopPropagation();
    onback();
  }

  // Suit la fin du fil tant que l'utilisateur n'est pas remonté.
  let scroller: HTMLElement | undefined = $state();
  let stick = true;
  function onscroll(): void {
    if (!scroller) return;
    stick = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 24;
  }
  $effect(() => {
    void [lines, waiting, agent.status];
    if (stick && scroller) scroller.scrollTop = scroller.scrollHeight;
  });
</script>

<div class="detail" data-testid="agent-detail-view" data-agent-detail={agent.tool_use_id}>
  <div class="top">
    <button type="button" class="back" data-testid="agent-detail-back" aria-label="Retour à la liste des agents"
      onclick={onback} onkeydown={backKey}>
      <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><path d="M7.5 2.5L4 6l3.5 3.5" /></svg>
      Agents
    </button>
    <div class="line">
      <span class="dot {st.tone}">{st.glyph}</span>
      <span class="name" style:background={agentColor(agent.color, agent.agent_type)}>{agentName(agent.agent_type)}</span>
      <span class="title" title={agent.description}>{agent.description}</span>
    </div>
    <div class="meta">
      <span class={stateLabel.tone === "ask" ? "ask" : stateLabel.tone === "err" ? "err" : ""} data-testid="agent-detail-state"
        >{stateLabel.text}</span
      >
      {#each statParts as p, i (i)}<span>{DOT}</span><span>{p}</span>{/each}
    </div>
  </div>
  <div class="scroll" bind:this={scroller} {onscroll} data-testid="agent-detail-thread">
    <div class="prompt"><span class="grad">{PROMPT}</span><span>{prompt?.text || agent.description}</span></div>
    {#if body.length === 0 && live}
      <div class="muted">(fil vide)</div>
    {/if}
    {#each body as l (l.key)}
      {#if l.kind === "text"}
        <div class="msg"><span class="grad">{BULLET}</span><span class="x">{l.text}{l.done ? "" : "…"}</span></div>
      {:else if l.kind === "tool"}
        <div class="tool">
          <span class={l.isError ? "err" : "grad"}>{ELBOW} </span><span class="x"
            ><span class="t">{l.title}</span>{l.arg ? `(${l.arg})` : ""}{#if l.result !== null}<span
                class="res"
                class:err={l.isError}>  {l.result}</span
              >{/if}</span
          >
        </div>
      {/if}
    {/each}
    {#if waiting && live}
      <div class="ask-box" data-testid="agent-detail-question">
        <div class="ask-head">{WAITING_GLYPH} Question en attente</div>
        {#each question?.questions ?? [] as q, i (i)}
          <div class="ask-q">{q.question}</div>
        {/each}
        <div class="muted">Répondez dans la carte de question de la conversation.</div>
      </div>
    {:else if live}
      <div class="muted"><span class="grad">{TURN_DONE}</span> {detail.text}</div>
    {:else if agent.status === "Completed"}
      <div class="ok">✓ Terminé{elapsed !== null ? ` en ${seconds(elapsed)}` : ""}</div>
    {:else if agent.status === "Failed"}
      <div class="err">✗ Échec{elapsed !== null ? ` après ${seconds(elapsed)}` : ""}</div>
    {:else}
      <div class="muted">■ Arrêté{elapsed !== null ? ` après ${seconds(elapsed)}` : ""}</div>
    {/if}
  </div>
</div>

<style>
  .detail {
    flex: 1 1 auto;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .top {
    flex: 0 0 auto;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 8px 12px 10px 10px;
    border-bottom: 1px solid var(--rule);
  }
  .back {
    align-self: flex-start;
    display: flex;
    align-items: center;
    gap: 6px;
    min-height: 28px;
    padding: 0 10px 0 6px;
    border: 1px solid var(--rule);
    border-radius: 4px;
    background: var(--surface);
    color: var(--fg);
    font: inherit;
    font-size: 12px;
    cursor: pointer;
  }
  .back:focus-visible {
    outline: 1px dotted var(--dim);
    outline-offset: 1px;
  }
  .back svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.6;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .line {
    display: flex;
    gap: 8px;
    align-items: baseline;
    min-width: 0;
    white-space: nowrap;
  }
  .dot {
    flex: 0 0 auto;
  }
  .name {
    flex: 0 0 auto;
    padding: 0 6px;
    border-radius: 4px;
    color: var(--on-agent);
    font-weight: 500;
  }
  .title {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--strong);
    font-weight: 700;
  }
  .meta {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    font-size: 12px;
    color: var(--muted);
    padding-left: 18px;
  }
  .scroll {
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
    padding: 12px 12px 12px 10px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .prompt {
    display: flex;
    gap: 10px;
    padding: 6px 10px;
    border-radius: 6px;
    background: var(--surface);
    color: var(--strong);
    font-weight: 700;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .msg {
    display: flex;
    gap: 10px;
    color: var(--fg);
  }
  .tool {
    display: flex;
    padding-left: 18px;
    color: var(--dim);
  }
  .x {
    flex: 1 1 auto;
    min-width: 0;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .t {
    color: var(--fg);
    font-weight: 700;
  }
  .ask-box {
    border: 1px solid var(--warn);
    border-radius: 6px;
    background: var(--warn-bg);
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .ask-head {
    color: var(--warn);
    font-weight: 700;
  }
  .ask-q {
    color: var(--strong);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .grad,
  .run {
    flex: 0 0 auto;
    background: var(--accent-grad);
    -webkit-background-clip: text;
    background-clip: text;
    color: transparent;
  }
  .muted,
  .dim {
    color: var(--dim);
  }
  .ok {
    color: var(--ok);
  }
  .err {
    color: var(--danger);
  }
  .ask {
    color: var(--warn);
  }
</style>
