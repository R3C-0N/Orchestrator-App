// Onglets de conversation (`src/lib/state/tabs.ts`) : libellé, infobulle et
// onglet affiché après une fermeture.
import { describe, expect, it } from "vitest";
import { afterClose, baseName, NEW_TAB_LABEL, tabLabel, tabTitle } from "../../src/lib/state/tabs";

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
