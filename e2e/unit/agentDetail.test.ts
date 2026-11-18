// En-tête de la conversation d'un agent dans la colonne (maquette
// « Orchestrator » J15, vue détaillée) : l'état en clair. L'interaction
// (clic, retour, focus) est vérifiée dans `e2e/agentDetail.spec.ts`.
import { describe, expect, it } from "vitest";
import { agentStateLabel } from "../../src/lib/components/agents/agentView";

describe("agentStateLabel", () => {
  it("donne l'état en clair de l'en-tête", () => {
    expect(agentStateLabel("Running", true)).toEqual({ text: "En attente d'action", tone: "ask" });
    expect(agentStateLabel("Running")).toEqual({ text: "En cours", tone: "run" });
    expect(agentStateLabel("Starting")).toEqual({ text: "En cours", tone: "run" });
    // Un agent fini n'attend plus rien.
    expect(agentStateLabel("Completed", true)).toEqual({ text: "Terminé", tone: "ok" });
    expect(agentStateLabel("Failed")).toEqual({ text: "Échec", tone: "err" });
    expect(agentStateLabel("Stopped")).toEqual({ text: "Arrêté", tone: "dim" });
  });
});
