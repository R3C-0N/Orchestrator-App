// Carte AskUserQuestion (J8) : modèle pur de `QuestionCard.svelte` (entrées,
// réponses au format `answers` de la spec 4.5), comparé aux réponses
// réellement envoyées dans les fixtures `t4` (réponse) et `t10` (ignorer).
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { QuestionItem } from "../../src/lib/core/bindings/QuestionItem";
import {
  answerOf,
  answerText,
  buildAnswers,
  emptyPick,
  itemsOf,
  type Pick,
} from "../../src/lib/components/QuestionCard.svelte";

const LANG: QuestionItem = {
  question: "Quel langage préfères-tu ?",
  header: "Langage",
  multi_select: false,
  options: [
    { label: "Rust", description: "Performances élevées" },
    { label: "TypeScript", description: "JavaScript typé" },
    { label: "Python", description: "Syntaxe simple" },
  ],
};
const MULTI: QuestionItem = { ...LANG, question: "Lesquels ?", header: "Langages", multi_select: true };
const pick = (over: Partial<Pick>): Pick => ({ ...emptyPick(), ...over });

/** Ligne `>>> ` d'une fixture stream-json contenant `needle`, décodée. */
function sent(fixture: string, needle: string): Record<string, unknown> {
  const line = readFileSync(`fixtures/stream-json/${fixture}.jsonl`, "utf8")
    .split("\n")
    .find((l) => l.startsWith(">>> ") && l.includes(needle));
  if (!line) throw new Error(`${needle} absent de ${fixture}`);
  return JSON.parse(line.slice(4));
}

describe("entrées de la carte (07)", () => {
  it("question simple : options, « Type something. », « Chat about this » (numéros 1 à N+1)", () => {
    expect(itemsOf(LANG).map((i) => i.kind)).toEqual(["option", "option", "option", "other", "chat"]);
  });
  it("multi-sélection : « Submit » avant la règle et « Chat about this »", () => {
    expect(itemsOf(MULTI).map((i) => i.kind)).toEqual(["option", "option", "option", "other", "submit", "chat"]);
  });
});

describe("réponse d'une question", () => {
  it("simple : le label choisi", () => {
    expect(answerOf(LANG, pick({ selected: [1] }))).toBe("TypeScript");
  });
  it("simple : le texte libre, sans espaces autour", () => {
    expect(answerOf(LANG, pick({ other: "  Zig " }))).toBe("Zig");
  });
  it("simple : rien choisi → null", () => {
    expect(answerOf(LANG, pick({ other: "   " }))).toBeNull();
  });
  it("multi : labels dans l'ordre des options, puis le texte libre coché", () => {
    expect(answerOf(MULTI, pick({ selected: [2, 0], other: "Zig", otherOn: true }))).toEqual(["Rust", "Python", "Zig"]);
  });
  it("multi : texte libre non coché ignoré ; rien coché → null", () => {
    expect(answerOf(MULTI, pick({ selected: [1], other: "Zig" }))).toEqual(["TypeScript"]);
    expect(answerOf(MULTI, pick({ other: "Zig" }))).toBeNull();
  });
  it("texte de relecture", () => {
    expect(answerText(["Rust", "Zig"])).toBe("Rust, Zig");
    expect(answerText("Rust")).toBe("Rust");
  });
});

describe("answers (spec 4.5)", () => {
  it("clé = texte de la question ; questions sans réponse ou non validées omises", () => {
    const Q2: QuestionItem = { ...LANG, question: "Et la couleur ?", header: "Couleur" };
    const answers = buildAnswers(
      [LANG, MULTI, Q2],
      [pick({ selected: [0], done: true }), pick({ selected: [0, 1], done: true }), pick({ selected: [1] })],
    );
    expect(answers).toEqual({ "Quel langage préfères-tu ?": "Rust", "Lesquels ?": ["Rust", "TypeScript"] });
  });

  it("t4 : Bleu choisi donne exactement les answers envoyés au CLI", () => {
    const line = sent("t4", '"answers"');
    const input = (line.response as { response: { updatedInput: { questions: unknown[]; answers: unknown } } }).response
      .updatedInput;
    const q = input.questions[0] as { question: string; header: string; options: QuestionItem["options"]; multiSelect: boolean };
    const item: QuestionItem = { question: q.question, header: q.header, options: q.options, multi_select: q.multiSelect };
    const bleu = item.options.findIndex((o) => o.label === "Bleu");
    expect(buildAnswers([item], [pick({ selected: [bleu], done: true })])).toEqual(input.answers);
  });

  it("t10 : « ignorer » = answers vide côté CLI (la carte n'envoie aucune réponse)", () => {
    const line = sent("t10", '"unanswered"');
    const input = (line.response as { response: { updatedInput: { answers: unknown; annotations: unknown } } }).response
      .updatedInput;
    expect(input.answers).toEqual({});
    expect(input.annotations).toEqual({ "Quelle est ta couleur préférée ?": { unanswered: "skipped" } });
  });
});
