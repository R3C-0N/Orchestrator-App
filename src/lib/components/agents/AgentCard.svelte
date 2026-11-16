<script lang="ts">
  // Une ligne d'agent de la colonne (J9), style du panneau du TUI (`05`) :
  //   ├ ◯ agent-bleu  Ping bleu agent
  //   │   ⎿  Read…                    6s · ↓ 38.9k tokens
  // Dans le TUI, durée et tokens sont à droite de la description ; la
  // colonne étant étroite, ils passent à droite de la seconde ligne pour
  // laisser la place à la description.
  // Nom sur fond de sa couleur (texte noir), comme les sous-lignes du fil
  // principal ; agent inconnu (`color: null`) en gris. La ligne est un
  // bouton : clic, Entrée ou Espace déroule le fil de l'agent (bonus).
  import type { AgentEvent } from "../../core/bindings/AgentEvent";
  import { agentColor, agentName } from "../../render/colors";
  import { ELBOW, TREE_END, TREE_MID } from "../../render/glyphs";
  import type { Row } from "../../state/session.svelte";
  import AgentThread from "./AgentThread.svelte";
  import { agentDetail, agentGlyph, agentLabel, agentStats } from "./agentView";

  let {
    agent,
    now,
    last = false,
    open = false,
    thread = null,
    cwd = null,
    ontoggle,
  }: {
    /** État complet de l'agent (dernier `AgentEvent` reçu). */
    agent: AgentEvent;
    /** Horloge (ms epoch) pour la durée d'un agent en cours. */
    now: number;
    /** Dernier agent de la liste : `└` au lieu de `├`, pas de `│` dessous. */
    last?: boolean;
    /** Fil déroulé. */
    open?: boolean;
    /** Lignes du fil de l'agent (`SessionStore.threads`), `null` si absent. */
    thread?: readonly Row[] | null;
    /** Dossier de travail (chemins relatifs du fil). */
    cwd?: string | null;
    /** Clic sur la ligne ; absent = ligne non dépliable. */
    ontoggle?: (toolUseId: string) => void;
  } = $props();

  const st = $derived(agentGlyph(agent.status));
  const detail = $derived(agentDetail(agent));
  const stats = $derived(agentStats(agent, now));
  const pipe = $derived(last ? " " : "│");
</script>

<div class="card" data-agent={agent.tool_use_id} data-status={agent.status}>
  <button
    type="button"
    class="head"
    disabled={!ontoggle}
    aria-expanded={ontoggle ? open : undefined}
    aria-label={agentLabel(agent, now)}
    title={`${agentName(agent.agent_type)} — ${agent.description}`}
    onclick={() => ontoggle?.(agent.tool_use_id)}
  >
    <span class="line">
      <span class="g dim">{last ? TREE_END : TREE_MID}</span>
      <span class="g {st.tone}" data-testid="agent-glyph">{st.glyph}</span>
      <span class="name" style:background={agentColor(agent.color, agent.agent_type)}>{agentName(agent.agent_type)}</span>
      <span class="desc dim">{agent.description}</span>
    </span>
    <span class="line">
      <span class="g dim">{pipe}</span>
      <span class="g"></span>
      <span class="elbow dim">{ELBOW}</span>
      <span class="detail {detail.tone}" data-testid="agent-detail">{detail.text}</span>
      <span class="stats dim" data-testid="agent-stats">{stats}</span>
    </span>
  </button>
  {#if open}
    <AgentThread rows={thread ?? []} {cwd} gutter={pipe} />
  {/if}
</div>

<style>
  .head {
    display: block;
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
    color: var(--t-fg);
  }
  .head:focus-visible {
    outline: 1px dotted var(--t-dim);
    outline-offset: -1px;
  }
  .line {
    display: flex;
    white-space: pre;
    min-width: 0;
  }
  .g {
    flex: 0 0 2ch;
  }
  .name {
    flex: 0 0 auto;
    color: #000000;
  }
  .desc {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    padding-left: 2ch;
  }
  .stats {
    flex: 0 0 auto;
    padding-left: 2ch;
  }
  .elbow {
    flex: 0 0 3ch;
  }
  .detail {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .dim {
    color: var(--t-dim);
  }
  .ok {
    color: var(--t-ok);
  }
  .err {
    color: var(--t-error);
  }
</style>
