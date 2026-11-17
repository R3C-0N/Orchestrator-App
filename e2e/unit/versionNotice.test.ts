// Avertissement « version non testée » : masqué pour la version fermée,
// affiché pour toute autre, et affiché sans erreur si le stockage manque.
import { describe, expect, it } from "vitest";
import { EventFactory } from "../../src/lib/core/mock";
import { SessionStore } from "../../src/lib/state/session.svelte";
import {
  DISMISSED_KEY,
  dismissVersionNotice,
  isVersionNoticeDismissed,
  noticeVersion,
} from "../../src/lib/state/versionNotice";

function memory(): Pick<Storage, "getItem" | "setItem"> & { data: Map<string, string> } {
  const data = new Map<string, string>();
  return { data, getItem: (k) => data.get(k) ?? null, setItem: (k, v) => void data.set(k, v) };
}

const broken = {
  getItem: () => {
    throw new Error("SecurityError");
  },
  setItem: () => {
    throw new Error("QuotaExceededError");
  },
};

describe("versionNotice", () => {
  it("rien de mémorisé : affiché", () => {
    expect(isVersionNoticeDismissed("2.1.326", memory())).toBe(false);
  });

  it("version masquée = version courante : caché", () => {
    const s = memory();
    expect(dismissVersionNotice("2.1.326", s)).toBe(true);
    expect(s.data.get(DISMISSED_KEY)).toBe("2.1.326");
    expect(isVersionNoticeDismissed("2.1.326", s)).toBe(true);
  });

  it("version différente de la version masquée : affiché", () => {
    const s = memory();
    dismissVersionNotice("2.1.326", s);
    expect(isVersionNoticeDismissed("2.1.327", s)).toBe(false);
    // Fermer la nouvelle remplace l'ancienne : 2.1.326 revient si on y retourne.
    dismissVersionNotice("2.1.327", s);
    expect(isVersionNoticeDismissed("2.1.327", s)).toBe(true);
    expect(isVersionNoticeDismissed("2.1.326", s)).toBe(false);
  });

  it("version inconnue : affiché, rien n'est mémorisé", () => {
    const s = memory();
    expect(dismissVersionNotice(null, s)).toBe(false);
    expect(s.data.size).toBe(0);
    expect(isVersionNoticeDismissed(null, s)).toBe(false);
  });

  it("stockage indisponible ou en erreur : affiché, sans exception", () => {
    expect(isVersionNoticeDismissed("2.1.326", null)).toBe(false);
    expect(dismissVersionNotice("2.1.326", null)).toBe(false);
    expect(() => isVersionNoticeDismissed("2.1.326", broken)).not.toThrow();
    expect(isVersionNoticeDismissed("2.1.326", broken)).toBe(false);
    expect(dismissVersionNotice("2.1.326", broken)).toBe(false);
  });

  it("accès à localStorage qui lève (stockage par défaut) : affiché, sans exception", () => {
    const before = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      get() {
        throw new Error("SecurityError");
      },
    });
    try {
      expect(isVersionNoticeDismissed("2.1.326")).toBe(false);
      expect(dismissVersionNotice("2.1.326")).toBe(false);
    } finally {
      if (before) Object.defineProperty(globalThis, "localStorage", before);
      else delete (globalThis as { localStorage?: unknown }).localStorage;
    }
  });

  it("version lue dans detail.found de la notice", () => {
    expect(noticeVersion({ found: "2.1.326", expected: "2.1.325" })).toBe("2.1.326");
    expect(noticeVersion(null)).toBeNull();
    expect(noticeVersion({ found: 3 })).toBeNull();
    expect(noticeVersion({ found: "" })).toBeNull();
    expect(noticeVersion("2.1.326")).toBeNull();
  });

  it("le bandeau garde detail : la version est connue avant ready/init (ordre du cœur)", () => {
    const f = new EventFactory();
    const s = new SessionStore(() => {
      throw new Error("planificateur non attendu");
    });
    s.apply([
      f.make({
        kind: "notice",
        level: "Warn",
        code: "unexpected_version",
        message: "v",
        detail: { found: "2.1.326", expected: "2.1.325" },
      }),
    ]);
    expect(s.meta.version).toBeNull();
    expect(s.banners).toHaveLength(1);
    expect(noticeVersion(s.banners[0].detail)).toBe("2.1.326");
  });
});
