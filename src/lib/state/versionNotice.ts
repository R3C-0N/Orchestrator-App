// Avertissement « version non testée » (notice `unexpected_version`, spec 1) :
// la croix du bandeau le masque pour CETTE version de claude.exe. Le choix est
// gardé dans le `localStorage` du webview (valeur = version masquée) ; il ne
// vaut plus dès que la version change. Stockage indisponible ou en erreur :
// l'avertissement reste affiché, sans exception.

/** Code de la notice émise par le réducteur Rust (`check_version`). */
export const VERSION_NOTICE_CODE = "unexpected_version";
/** Clé du `localStorage` : la version dont l'avertissement a été fermé. */
export const DISMISSED_KEY = "orchestrator.versionNotice.dismissed";

type Store = Pick<Storage, "getItem" | "setItem">;

/**
 * Version trouvée, lue dans `detail.found` de la notice : le cœur l'émet
 * avant `ready`/`init`, donc avant que `meta.version` ne soit connu.
 */
export function noticeVersion(detail: unknown): string | null {
  if (detail && typeof detail === "object" && "found" in detail) {
    const found = (detail as { found: unknown }).found;
    if (typeof found === "string" && found) return found;
  }
  return null;
}

/** `localStorage` s'il est accessible (son simple accès peut lever). */
function defaultStorage(): Store | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

/** Vrai si l'avertissement a été fermé pour `version` (version inconnue : affiché). */
export function isVersionNoticeDismissed(version: string | null, storage: Store | null = defaultStorage()): boolean {
  if (!version || !storage) return false;
  try {
    return storage.getItem(DISMISSED_KEY) === version;
  } catch {
    return false;
  }
}

/** Mémorise la fermeture pour `version` ; rend faux si le stockage a échoué. */
export function dismissVersionNotice(version: string | null, storage: Store | null = defaultStorage()): boolean {
  if (!version || !storage) return false;
  try {
    storage.setItem(DISMISSED_KEY, version);
    return true;
  } catch {
    return false;
  }
}
