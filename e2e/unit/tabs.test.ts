// Onglets de conversation (`src/lib/state/tabs.ts`) : libellé, infobulle et
// onglet affiché après une fermeture ; J16 : onglet d'amélioration en cours.
import { describe, expect, it } from "vitest";
import { afterClose, baseName, NEW_TAB_LABEL, runningImproveTab, snapshotTab, tabLabel, tabTitle } from "../../src/lib/state/tabs";

describe("baseName", () => {
  it("chemins Windows et POSIX, séparateur final", () => {
    expect(baseName("C:\\Users\\moi\\projets\\orchestrator")).toBe("orchestrator");
    expect(baseName("C:\\Users\\moi\\projets\\app\\")).toBe("app");
    expect(baseName("/home/moi/app")).toBe("app");
    expect(baseName("D:/a/b")).toBe("b");
  });
});

describe("tabLabel", () => {
  it("sélecteur des projets : « Nouvel onglet »", () => {
    expect(tabLabel({ phase: "projects", cwd: "C:\\x\\ancien", projectPath: "C:\\x\\y" })).toBe(NEW_TAB_LABEL);
  });

  it("sélecteur des sessions : le projet choisi", () => {
    expect(tabLabel({ phase: "sessions", cwd: "", projectPath: "C:\\x\\mon-projet" })).toBe("mon-projet");
  });

  it("conversation : le dossier de la session", () => {
    expect(tabLabel({ phase: "start", cwd: "C:\\x\\app", projectPath: null })).toBe("app");
  });

  it("conversation sans dossier (pas encore démarrée) : « Nouvel onglet »", () => {
    expect(tabLabel({ phase: "start", cwd: "", projectPath: null })).toBe(NEW_TAB_LABEL);
  });

  it("infobulle : chemin complet", () => {
    expect(tabTitle({ phase: "start", cwd: "C:\\x\\app", projectPath: null })).toBe("C:\\x\\app");
    expect(tabTitle({ phase: "projects", cwd: "C:\\x\\app", projectPath: null })).toBe(NEW_TAB_LABEL);
  });
});

describe("afterClose", () => {
  it("fermer un onglet inactif garde l'actif", () => {
    expect(afterClose([1, 2, 3], 1, 3)).toBe(1);
  });

  it("fermer l'actif passe au voisin de droite, sinon de gauche", () => {
    expect(afterClose([1, 2, 3], 2, 2)).toBe(3);
    expect(afterClose([1, 2, 3], 3, 3)).toBe(2);
  });

  it("dernier onglet fermé : aucun", () => {
    expect(afterClose([1], 1, 1)).toBeNull();
  });
});

describe("runningImproveTab (J16)", () => {
  it("l'onglet d'amélioration d'un cycle en cours (état connu ou non)", () => {
    expect(
      runningImproveTab([
        { id: 1, kind: "conversation", improve: null },
        { id: 2, kind: "improve", improve: { step: "awaiting_user" } },
      ]),
    ).toBe(2);
    expect(runningImproveTab([{ id: 3, kind: "improve", improve: null }])).toBe(3);
  });

  it("aucun : pas d'onglet d'amélioration, ou cycle fini", () => {
    expect(runningImproveTab([{ id: 1, kind: "conversation", improve: null }])).toBeNull();
    expect(runningImproveTab([{ id: 2, kind: "improve", improve: { step: "merged" } }])).toBeNull();
    expect(runningImproveTab([{ id: 2, kind: "improve", improve: { step: "cancelled" } }])).toBeNull();
  });
});

describe("snapshotTab (J16)", () => {
  const tabs = [
    { id: 0, kind: "conversation" as const },
    { id: 1, kind: "conversation" as const },
    { id: 2, kind: "improve" as const },
  ];

  it("conversation active : elle-même", () => {
    expect(snapshotTab(tabs, 1, 0)?.id).toBe(1);
  });

  it("onglet d'amélioration actif : la dernière conversation affichée, sinon la première", () => {
    expect(snapshotTab(tabs, 2, 1)?.id).toBe(1);
    expect(snapshotTab(tabs, 2, 0)?.id).toBe(0);
    expect(snapshotTab(tabs, 2, 9)?.id).toBe(0);
    expect(snapshotTab(tabs, 2, null)?.id).toBe(0);
  });

  it("aucune conversation : null", () => {
    expect(snapshotTab([{ id: 2, kind: "improve" as const }], 2, 0)).toBeNull();
  });
});
