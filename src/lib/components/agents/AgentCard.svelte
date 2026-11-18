<script lang="ts">
  // Une ligne d'agent de la colonne (J9), restylée d'après la maquette J15 :
  //   ● [agent-bleu]  Ping bleu agent
  //     └ Read…                         6s · ↓ 38.9k tokens
  // Pastille d'état (`agentGlyph`), nom sur fond de sa couleur, description ;
  // dessous l'outil en cours ou l'état final, la durée et les tokens à droite.
  // Agent inconnu (`color: null`) en gris. La ligne est un bouton : clic,
  // Entrée ou Espace déroule le fil de l'agent (bonus), ou, avec `opensDetail`,
  // ouvre sa conversation à la place de la liste (maquette J15).
  import type { AgentEvent } from "../../core/bindings/AgentEvent";
  import { agentColor, agentName } from "../../render/colors";
  import type { Row } from "../../state/session.svelte";
  import AgentThread from "./AgentThread.svelte";
  import { agentDetail, agentGlyph, agentLabel, agentStats } from "./agentView";

  let {
    agent,
    now,
    open = false,
    waiting = false,
    thread = null,
    cwd = null,
    opensDetail = false,
    ontoggle,
  }: {
    /** État complet de l'agent (dernier `AgentEvent` reçu). */
    agent: AgentEvent;
    /** Horloge (ms epoch) pour la durée d'un agent en cours. */
    now: number;
    /** Fil déroulé. */
    open?: boolean;
    /** Une question de son fil attend la réponse de l'utilisateur. */
    waiting?: boolean;
    /** Lignes du fil de l'agent (`SessionStore.threads`), `null` si absent. */
    thread?: readonly Row[] | null;
    /** Dossier de travail (chemins relatifs du fil). */
    cwd?: string | null;
    /** Le clic ouvre la conversation de l'agent (pas de fil déroulé sur place). */
    opensDetail?: boolean;
    /** Clic sur la ligne ; absent = ligne non dépliable. */
    ontoggle?: (toolUseId: string) => void;
  } = $props();

  const st = $derived(agentGlyph(agent.status, waiting));
  const detail = $derived(agentDetail(agent, waiting));
  const stats = $derived(agentStats(agent, now));
</script>

<div class="card" data-agent={agent.tool_use_id} data-status={agent.status} data-waiting={waiting || undefined}>
  <button
    type="button"
    class="head"
    disabled={!ontoggle}
    aria-expanded={ontoggle && !opensDetail ? open : undefined}
    aria-label={agentLabel(agent, now, waiting)}
    title={opensDetail && ontoggle
      ? `Ouvrir la conversation de ${agentName(agent.agent_type)} — ${agent.description}`
      : `${agentName(agent.agent_type)} — ${agent.description}`}
    onclick={() => ontoggle?.(agent.tool_use_id)}
  >
    <span class="line">
      <span class="dot {st.tone}" data-testid="agent-glyph">{st.glyph}</span>
      <span class="name" style:background={agentColor(agent.color, agent.agent_type)}>{agentName(agent.agent_type)}</span>
      <span class="desc">{agent.description}</span>
    </span>
    <span class="line sub">
      <span class="detail {detail.tone}" data-testid="agent-detail">└ {detail.text}</span>
      <span class="stats" data-testid="agent-stats">{stats}</span>
    </span>
  </button>
  {#if open}
    <AgentThread rows={thread ?? []} {cwd} gutter=" " />
  {/if}
</div>

<style>
  .card {
    padding: 1px 0;
  }
  .head {
    display: flex;
    flex-direction: column;
    width: 100%;
    padding: 0;
    margin: 0;
    border: none;
    background: transparent;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .head:disabled {
    cursor: default;
  }
  .head:hover:not(:disabled) .desc,
  .head:focus-visible .desc {
    color: var(--strong);
  }
  .head:focus-visible {
    outline: 1px dotted var(--dim);
    outline-offset: -1px;
  }
  .line {
    display: flex;
    gap: 8px;
    align-items: baseline;
    white-space: nowrap;
    overflow: hidden;
    min-width: 0;
  }
  /* La pastille déborde sur le filet du groupe (maquette) : pas de rognage. */
  .line:first-child {
    overflow: visible;
  }
  .dot {
    flex: 0 0 auto;
    margin-left: -5px;
  }
  .name {
    flex: 0 0 auto;
    padding: 0 6px;
    border-radius: 4px;
    color: var(--on-agent);
    font-weight: 500;
  }
  .desc {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--fg);
    padding-left: 4px;
  }
  .sub {
    color: var(--muted);
    padding-left: 18px;
  }
  .detail {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .stats {
    flex: 1 0 auto;
    text-align: right;
  }
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
  .run {
    background: var(--accent-grad);
    -webkit-background-clip: text;
    background-clip: text;
    color: transparent;
  }
  .detail.dim,
  .detail.ok,
  .detail.run {
    background: none;
    color: inherit;
  }
</style>
