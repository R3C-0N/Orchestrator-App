// @vitest-environment jsdom
// Thème de l'interface (J15) : sombre par défaut, bascule clair/sombre posée
// sur `<html data-theme>`, choix retenu dans le `localStorage`, et rien ne
// casse si le stockage est absent ou en erreur. Le module applique le thème
// à son chargement : chaque test le recharge (`vi.resetModules`).
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const KEY = "orchestrator.theme";

async function load() {
  vi.resetModules();
  return import("../../src/lib/state/theme.svelte");
}

const broken = {
  getItem: () => {
    throw new Error("SecurityError");
  },
  setItem: () => {
    throw new Error("QuotaExceededError");
  },
};

beforeEach(() => {
  localStorage.clear();
  delete document.documentElement.dataset.theme;
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("theme", () => {
  it("rien de mémorisé : sombre, posé sur <html>", async () => {
    const t = await load();
    expect(t.currentTheme()).toBe("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
  });

  it("bascule : clair puis sombre, <html> suit", async () => {
    const t = await load();
    expect(t.toggleTheme()).toBe("light");
    expect(t.currentTheme()).toBe("light");
    expect(document.documentElement.dataset.theme).toBe("light");
    expect(t.toggleTheme()).toBe("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
  });

  it("persistance : le choix est écrit puis relu au chargement suivant", async () => {
    const t = await load();
    t.toggleTheme();
    expect(localStorage.getItem(KEY)).toBe("light");
    delete document.documentElement.dataset.theme;
    const again = await load();
    expect(again.currentTheme()).toBe("light");
    expect(document.documentElement.dataset.theme).toBe("light");
  });

  it("valeur inconnue en mémoire : sombre", async () => {
    localStorage.setItem(KEY, "violet");
    const t = await load();
    expect(t.currentTheme()).toBe("dark");
  });

  it("stockage en erreur : sombre au chargement, bascule sans exception", async () => {
    vi.stubGlobal("localStorage", broken);
    const t = await load();
    expect(t.currentTheme()).toBe("dark");
    expect(() => t.toggleTheme()).not.toThrow();
    expect(t.currentTheme()).toBe("light");
    expect(document.documentElement.dataset.theme).toBe("light");
    expect(t.setTheme("dark")).toBe(false);
  });

  it("stockage dont l'accès même lève : sombre, bascule sans exception", async () => {
    const saved = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      get() {
        throw new Error("SecurityError");
      },
    });
    try {
      const t = await load();
      expect(t.currentTheme()).toBe("dark");
      expect(t.toggleTheme()).toBe("light");
    } finally {
      // Rend le localStorage de jsdom.
      if (saved) Object.defineProperty(globalThis, "localStorage", saved);
      else delete (globalThis as { localStorage?: unknown }).localStorage;
    }
  });

  it("stockage absent : sombre, et setTheme le signale", async () => {
    vi.stubGlobal("localStorage", undefined);
    const t = await load();
    expect(t.currentTheme()).toBe("dark");
    expect(t.setTheme("light")).toBe(false);
    expect(t.currentTheme()).toBe("light");
  });
});
