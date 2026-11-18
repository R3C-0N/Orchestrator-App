// Thème de l'interface (maquette « Orchestrator », J15) : sombre par défaut,
// clair au choix (bouton Clair/Sombre de la barre d'état, `ThemeToggle`).
// Le thème est posé en `data-theme` sur `<html>` ; `app.css` redéfinit la
// palette sous `:root[data-theme="light"]`. Le choix est gardé dans le
// `localStorage` du webview ; stockage absent ou en erreur : thème sombre,
// et la bascule marche quand même (sans être retenue).
// Ne suit pas le thème du système (`prefers-color-scheme`) : non demandé.

export type Theme = "dark" | "light";

/** Clé du `localStorage` : le dernier thème choisi. */
export const THEME_KEY = "orchestrator.theme";

type Store = Pick<Storage, "getItem" | "setItem">;

/** `localStorage` s'il est accessible (son simple accès peut lever). */
function defaultStorage(): Store | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

/** Thème mémorisé ; rien de valable ou stockage en erreur : sombre. */
export function readTheme(storage: Store | null = defaultStorage()): Theme {
  if (!storage) return "dark";
  try {
    return storage.getItem(THEME_KEY) === "light" ? "light" : "dark";
  } catch {
    return "dark";
  }
}

/** Pose le thème sur `<html>` (sans effet hors navigateur). */
function apply(theme: Theme): void {
  if (typeof document !== "undefined") document.documentElement.dataset.theme = theme;
}

const initial = readTheme();
let current = $state<Theme>(initial);
apply(initial);

/** Thème courant (réactif : lisible dans un `$derived` ou un gabarit). */
export function currentTheme(): Theme {
  return current;
}

/** Change de thème et le mémorise ; rend faux si le stockage a échoué. */
export function setTheme(theme: Theme, storage: Store | null = defaultStorage()): boolean {
  current = theme;
  apply(theme);
  if (!storage) return false;
  try {
    storage.setItem(THEME_KEY, theme);
    return true;
  } catch {
    return false;
  }
}

/** Passe du sombre au clair, ou l'inverse ; rend le nouveau thème. */
export function toggleTheme(storage: Store | null = defaultStorage()): Theme {
  const next: Theme = current === "dark" ? "light" : "dark";
  setTheme(next, storage);
  return next;
}
