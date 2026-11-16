// Palette (J10) : modèle pur de `Palette.svelte` (entrées, filtre flou,
// colonne des noms, fenêtre visible), sur les 108 commandes de `t11`.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { CommandInfo } from "../../src/lib/core/bindings/CommandInfo";
import {
  entriesFrom,
  filterEntries,
  nameColumn,
  VISIBLE,
  windowOf,
} from "../../src/lib/components/Palette.svelte";

const t11 = JSON.parse(readFileSync("fixtures/ui-events/t11.json", "utf8")) as { kind: string; commands?: CommandInfo[] }[];
const COMMANDS = t11.find((e) => e.kind === "ready")!.commands!;
const ALL = entriesFrom(COMMANDS);

describe("entrées (ready.commands, t11)", () => {
  it("108 commandes, dont /orchestrateur et /formulaire-dynamique", () => {
    expect(COMMANDS).toHaveLength(108);
    expect(ALL).toHaveLength(108);
    const names = ALL.map((e) => e.name);
    expect(names).toContain("orchestrateur");
    expect(names).toContain("formulaire-dynamique");
  });
  it("init.skills et slash_commands complètent sans doublon ni vider la liste", () => {
    const more = entriesFrom(COMMANDS, ["loop", "/nouveau-skill", "nouveau-skill", ""]);
    expect(more).toHaveLength(109);
    expect(more.at(-1)).toEqual({ name: "nouveau-skill", description: "", argumentHint: null });
    expect(entriesFrom([], ["a"])).toEqual([{ name: "a", description: "", argumentHint: null }]);
  });
});

describe("filtre flou (nom puis description)", () => {
  const names = (q: string) => filterEntries(ALL, q).map((e) => e.name);
  it("filtre vide : tout, dans l'ordre de ready", () => {
    expect(names("")).toEqual(ALL.map((e) => e.name));
  });
  it("début du nom d'abord", () => {
    expect(names("orch")[0]).toBe("orchestrateur");
    expect(names("form")[0]).toBe("formulaire-dynamique");
    expect(names("loop")[0]).toBe("loop");
  });
  it("segment après « : » ou « - » : commit trouve les commandes du plugin", () => {
    expect(names("commit")).toEqual(
      expect.arrayContaining(["commit-commands:commit", "commit-commands:commit-push-pr"]),
    );
  });
  it("lettres dans l'ordre : « frmdyn » trouve formulaire-dynamique", () => {
    expect(names("frmdyn")).toContain("formulaire-dynamique");
  });
  it("description : « artefact » trouve formulaire-dynamique, sans casse", () => {
    expect(names("ARTEFACT")).toContain("formulaire-dynamique");
  });
  it("rien ne correspond : liste vide", () => {
    expect(names("zzzzqqq")).toEqual([]);
  });
});

describe("rendu (10)", () => {
  it("colonne des noms : /nom le plus long + 3 (descriptions en colonne 50 sur 10)", () => {
    const longest = Math.max(...ALL.map((e) => e.name.length));
    expect(nameColumn(ALL)).toBe(longest + 1 + 3);
    expect(4 + nameColumn(ALL)).toBe(50);
  });
  it("3 entrées visibles ; la fenêtre suit la sélection", () => {
    expect(VISIBLE).toBe(3);
    expect(windowOf(2, 1)).toEqual([0, 2]);
    expect(windowOf(108, 0)).toEqual([0, 3]);
    expect(windowOf(108, 2)).toEqual([0, 3]);
    expect(windowOf(108, 3)).toEqual([1, 4]);
    expect(windowOf(108, 107)).toEqual([105, 108]);
  });
});
