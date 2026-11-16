// Raccourcis de la saisie (J7, spec décision 13) : un test par cas de la
// liste du plan (sélection, tour en cours, saisie vide ou non, double Échap
// rapproché ou espacé, saisie vide, palette ou sélecteur ouvert, carte
// ignorable ou non). Règles mesurées sur le TUI 2.1.325 (captures c*, a*, n*, v*, k*).
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { decide, ESC_CLEAR_MS, Keys, type KeyContext, type KeyInput } from "../../src/lib/state/keys.svelte";

const key = (k: string, mods: Partial<KeyInput> = {}): KeyInput => ({
  key: k,
  ctrl: false,
  alt: false,
  shift: false,
  meta: false,
  ...mods,
});
const CTRL_C = key("c", { ctrl: true });
const ESC = key("Escape");

const ctx = (over: Partial<KeyContext> = {}): KeyContext => ({
  hasSelection: false,
  overlay: null,
  question: null,
  busy: false,
  interrupting: false,
  draft: "",
  now: 10_000,
  ...over,
});

describe("Ctrl+C", () => {
  it("avec une sélection : copie (même pendant un tour, même saisie non vide)", () => {
    expect(decide(CTRL_C, ctx({ hasSelection: true }), null)).toEqual({ type: "copy" });
    expect(decide(CTRL_C, ctx({ hasSelection: true, busy: true, draft: "x" }), null)).toEqual({ type: "copy" });
  });
  it("pendant un tour, saisie non vide : interrompt et garde la saisie (c4)", () => {
    expect(decide(CTRL_C, ctx({ busy: true, draft: "brouillon" }), null)).toEqual({ type: "interrupt" });
  });
  it("pendant un tour, saisie vide : interrompt (c5, k2)", () => {
    expect(decide(CTRL_C, ctx({ busy: true }), null)).toEqual({ type: "interrupt" });
  });
  it("pendant une interruption déjà demandée : rien de plus", () => {
    expect(decide(CTRL_C, ctx({ busy: true, interrupting: true }), null)).toEqual({ type: "none" });
  });
  it("au repos, saisie non vide : efface (c2)", () => {
    expect(decide(CTRL_C, ctx({ draft: "texte" }), null)).toEqual({ type: "clear" });
  });
  it("au repos, saisie vide : rien (c3, choix du prototype)", () => {
    expect(decide(CTRL_C, ctx(), null)).toEqual({ type: "none" });
  });
  it("Ctrl+Maj+C n'est pas Ctrl+C", () => {
    expect(decide(key("C", { ctrl: true, shift: true }), ctx({ draft: "x" }), null)).toEqual({ type: "pass" });
  });
});

describe("Échap", () => {
  it("palette ouverte : la ferme d'abord, même pendant un tour", () => {
    expect(decide(ESC, ctx({ overlay: "palette", busy: true, draft: "x" }), null)).toEqual({
      type: "closeOverlay",
      overlay: "palette",
    });
  });
  it("sélecteur ouvert : le ferme d'abord", () => {
    expect(decide(ESC, ctx({ overlay: "picker" }), null)).toEqual({ type: "closeOverlay", overlay: "picker" });
  });
  it("carte de question ignorable : l'ignore (07, J8)", () => {
    expect(decide(ESC, ctx({ question: { canSkip: true }, busy: true }), null)).toEqual({ type: "skipQuestion" });
  });
  it("carte de question non ignorable : rien, la carte reste", () => {
    expect(decide(ESC, ctx({ question: { canSkip: false }, busy: true }), null)).toEqual({ type: "none" });
  });
  it("pendant un tour : interrompt (08), saisie gardée", () => {
    expect(decide(ESC, ctx({ busy: true }), null)).toEqual({ type: "interrupt" });
    expect(decide(ESC, ctx({ busy: true, draft: "x" }), null)).toEqual({ type: "interrupt" });
  });
  it("saisie non vide, premier appui : indication sans rien effacer (a*)", () => {
    expect(decide(ESC, ctx({ draft: "essai" }), null)).toEqual({ type: "armClear" });
  });
  it("double Échap rapproché (< 0,8 s) : efface (n000 à n700)", () => {
    expect(decide(ESC, ctx({ draft: "essai", now: 10_700 }), 10_000)).toEqual({ type: "clear" });
    expect(decide(ESC, ctx({ draft: "essai", now: 10_736 }), 10_000)).toEqual({ type: "clear" });
  });
  it("double Échap espacé (> 0,8 s) : garde, réarme (n800)", () => {
    expect(decide(ESC, ctx({ draft: "essai", now: 10_843 }), 10_000)).toEqual({ type: "armClear" });
    expect(decide(ESC, ctx({ draft: "essai", now: 10_000 + ESC_CLEAR_MS }), 10_000)).toEqual({ type: "armClear" });
  });
  it("double Échap sur saisie vide : rien (Rewind hors prototype, v1, v4)", () => {
    expect(decide(ESC, ctx(), null)).toEqual({ type: "none" });
    expect(decide(ESC, ctx({ now: 10_200 }), 10_000)).toEqual({ type: "none" });
  });
});

describe("autres raccourcis", () => {
  it("Ctrl+K : palette (J10)", () => {
    expect(decide(key("k", { ctrl: true }), ctx(), null)).toEqual({ type: "openPalette" });
  });
  it("Ctrl+O : vue détaillée (J5)", () => {
    expect(decide(key("o", { ctrl: true }), ctx(), null)).toEqual({ type: "toggleVerbose" });
  });
  it("Alt+P : sélecteur de modèle", () => {
    expect(decide(key("p", { alt: true }), ctx(), null)).toEqual({ type: "openModelPicker" });
  });
  it("frappe ordinaire : laissée au navigateur", () => {
    expect(decide(key("a"), ctx({ draft: "x" }), null)).toEqual({ type: "pass" });
    expect(decide(key("Enter"), ctx({ draft: "x" }), null)).toEqual({ type: "pass" });
  });
});

describe("IME, répétition et conflits", () => {
  it("composition IME : tout est laissé à l'IME (Échap, Entrée, Ctrl+C)", () => {
    expect(decide(key("Escape", { composing: true }), ctx({ busy: true, draft: "x" }), null)).toEqual({ type: "pass" });
    expect(decide(key("Enter", { composing: true }), ctx({ draft: "x" }), null)).toEqual({ type: "pass" });
    expect(decide(key("c", { ctrl: true, composing: true }), ctx({ draft: "x" }), null)).toEqual({ type: "pass" });
  });
  it("Échap répété (touche maintenue) : ignoré, même armé ou pendant un tour", () => {
    expect(decide(key("Escape", { repeat: true }), ctx({ draft: "x", now: 10_100 }), 10_000)).toEqual({ type: "pass" });
    expect(decide(key("Escape", { repeat: true }), ctx({ busy: true }), null)).toEqual({ type: "pass" });
  });
  it("sélection avec palette ouverte : Ctrl+C copie, Échap ferme la palette", () => {
    const c = ctx({ hasSelection: true, overlay: "palette", draft: "x" });
    expect(decide(CTRL_C, c, null)).toEqual({ type: "copy" });
    expect(decide(ESC, c, null)).toEqual({ type: "closeOverlay", overlay: "palette" });
  });
  it("carte de question et sélecteur ouverts : Échap ferme d'abord le sélecteur", () => {
    expect(decide(ESC, ctx({ overlay: "picker", question: { canSkip: true }, busy: true }), null)).toEqual({
      type: "closeOverlay",
      overlay: "picker",
    });
  });
});

describe("sélecteur de projet ou de session ouvert (J11)", () => {
  const sel = (over: Partial<KeyContext> = {}) => ctx({ selector: true, ...over });
  it("↑↓, Entrée, Échap, Espace : laissés au sélecteur", () => {
    for (const k of ["ArrowUp", "ArrowDown", "Enter", "Escape", " "]) {
      expect(decide(key(k), sel({ draft: "x", busy: true }), null)).toEqual({ type: "pass" });
    }
  });
  it("Ctrl+O : sans effet (pas de vue détaillée derrière un sélecteur)", () => {
    expect(decide(key("o", { ctrl: true }), sel(), null)).toEqual({ type: "none" });
  });
  it("Ctrl+K et Alt+P : sans effet", () => {
    expect(decide(key("k", { ctrl: true }), sel(), null)).toEqual({ type: "none" });
    expect(decide(key("p", { alt: true }), sel(), null)).toEqual({ type: "none" });
  });
  it("Ctrl+C : copie une sélection, sinon rien (ni interruption ni effacement)", () => {
    expect(decide(CTRL_C, sel({ hasSelection: true }), null)).toEqual({ type: "copy" });
    expect(decide(CTRL_C, sel({ busy: true, draft: "x" }), null)).toEqual({ type: "none" });
  });
  it("frappe ordinaire : laissée au filtre du sélecteur", () => {
    expect(decide(key("a"), sel(), null)).toEqual({ type: "pass" });
  });
});

describe("Ctrl+C avec la carte de question ouverte (J8)", () => {
  const card = (over: Partial<KeyContext> = {}) => ctx({ question: { canSkip: true }, ...over });
  it("sélection : copie", () => {
    expect(decide(CTRL_C, card({ hasSelection: true, busy: true }), null)).toEqual({ type: "copy" });
  });
  it("pendant le tour : interrompt", () => {
    expect(decide(CTRL_C, card({ busy: true, draft: "x" }), null)).toEqual({ type: "interrupt" });
  });
  it("interruption déjà demandée : rien de plus", () => {
    expect(decide(CTRL_C, card({ busy: true, interrupting: true }), null)).toEqual({ type: "none" });
  });
  it("au repos : n'efface pas la saisie cachée derrière la carte", () => {
    expect(decide(CTRL_C, card({ draft: "brouillon caché" }), null)).toEqual({ type: "none" });
    expect(decide(CTRL_C, card({ question: { canSkip: false }, draft: "x" }), null)).toEqual({ type: "none" });
  });
});

describe("Ctrl+K sans correspondance (J10)", () => {
  const pal = (over: Partial<KeyContext> = {}) => ctx({ overlay: "palette", draft: "texte zzz", ...over });
  it("Entrée : laissée à la saisie (Prompt.svelte la consomme sans rien envoyer)", () => {
    expect(decide(key("Enter"), pal(), null)).toEqual({ type: "pass" });
  });
  it("Échap : ferme la palette, sans armer l'effacement", () => {
    expect(decide(ESC, pal(), null)).toEqual({ type: "closeOverlay", overlay: "palette" });
  });
  it("Ctrl+K de nouveau : reste une demande d'ouverture (sans effet si déjà ouverte)", () => {
    expect(decide(key("k", { ctrl: true }), pal(), null)).toEqual({ type: "openPalette" });
  });
});

describe("Keys (armement du double Échap)", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("premier Échap : indication « Esc again to clear », puis effacement au second", () => {
    const k = new Keys();
    expect(k.handle(ESC, ctx({ draft: "essai", now: 1000 })).type).toBe("armClear");
    expect(k.hint).toBe("Esc again to clear");
    expect(k.handle(ESC, ctx({ draft: "essai", now: 1500 })).type).toBe("clear");
    expect(k.hint).toBeNull();
  });
  it("l'indication disparaît après 0,8 s", () => {
    const k = new Keys();
    k.handle(ESC, ctx({ draft: "essai", now: 1000 }));
    vi.advanceTimersByTime(ESC_CLEAR_MS);
    expect(k.hint).toBeNull();
    expect(k.armedAt).toBeNull();
  });
  it("Maj entre les deux Échap ne désarme pas", () => {
    const k = new Keys();
    k.handle(ESC, ctx({ draft: "essai", now: 1000 }));
    k.handle(key("Shift", { shift: true }), ctx({ draft: "essai", now: 1100 }));
    expect(k.handle(ESC, ctx({ draft: "essai", now: 1200 })).type).toBe("clear");
  });
  it("Échap maintenu : le premier appui arme, la répétition n'efface pas", () => {
    const k = new Keys();
    expect(k.handle(ESC, ctx({ draft: "essai", now: 1000 })).type).toBe("armClear");
    for (let t = 1030; t < 1700; t += 33) {
      expect(k.handle(key("Escape", { repeat: true }), ctx({ draft: "essai", now: t })).type).toBe("pass");
    }
    expect(k.hint).toBe("Esc again to clear");
  });
  it("une frappe entre les deux Échap désarme", () => {
    const k = new Keys();
    k.handle(ESC, ctx({ draft: "essai", now: 1000 }));
    k.handle(key("a"), ctx({ draft: "essai", now: 1100 }));
    expect(k.handle(ESC, ctx({ draft: "essaia", now: 1200 })).type).toBe("armClear");
  });
});
