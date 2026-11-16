// Outils communs de la recette J12 (plan 1.11) : lancement de l'app compilée
// avec le port de débogage de WebView2 (CDP), arbre de processus, mémoire
// (ensemble de travail privé), fenêtres visibles. Windows seulement.
//
// Aucune touche n'est envoyée au bureau : tout passe par CDP (événements
// injectés dans la webview) ou par des API de lecture de Windows.
import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "@playwright/test";

export const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
export const WORK = join(ROOT, ".claude-work", "j12");
export const SANDBOX = join(WORK, "sandbox");
export const EXE = join(ROOT, "src-tauri", "target", "release", "orchestrator.exe");

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** PowerShell sans profil ; renvoie stdout. */
export function ps(script) {
  return execFileSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", script], {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
}

/** Tous les processus : pid, ppid, nom, ligne de commande. */
export function processes() {
  const out = ps(
    "Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,Name,CreationDate | ConvertTo-Json -Compress",
  );
  return JSON.parse(out).map((p) => ({ pid: p.ProcessId, ppid: p.ParentProcessId, name: p.Name }));
}

/** Pids de l'arbre de `root` (lui compris). */
export function tree(root, all = processes()) {
  const seen = new Set([root]);
  let grew = true;
  while (grew) {
    grew = false;
    for (const p of all) if (seen.has(p.ppid) && !seen.has(p.pid)) (seen.add(p.pid), (grew = true));
  }
  return all.filter((p) => seen.has(p.pid));
}

/**
 * Ensemble de travail privé (octets) par pid, comme la colonne « Mémoire »
 * du Gestionnaire des tâches (plan 1.11).
 */
export function privateWs(pids) {
  if (pids.length === 0) return {};
  const filter = pids.map((p) => `IDProcess=${p}`).join(" OR ");
  const out = ps(
    `Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -Filter "${filter}" | Select-Object IDProcess,Name,WorkingSetPrivate | ConvertTo-Json -Compress`,
  );
  if (!out.trim()) return {};
  const arr = [].concat(JSON.parse(out));
  return Object.fromEntries(arr.map((r) => [r.IDProcess, { name: r.Name, bytes: Number(r.WorkingSetPrivate) }]));
}

/**
 * Mémoire de l'app : orchestrator.exe + ses msedgewebview2.exe, hors
 * claude.exe (et hors conhost des claude.exe). En Mo (10^6).
 */
export function appMemory(rootPid) {
  const t = tree(rootPid);
  const keep = t.filter((p) => /^(orchestrator|msedgewebview2)\.exe$/i.test(p.name));
  const ws = privateWs(keep.map((p) => p.pid));
  const detail = keep.map((p) => ({ pid: p.pid, name: p.name, mb: +((ws[p.pid]?.bytes ?? 0) / 1e6).toFixed(1) }));
  const total = +detail.reduce((a, b) => a + b.mb, 0).toFixed(1);
  const claude = t.filter((p) => /^claude\.exe$/i.test(p.name)).map((p) => p.pid);
  return { totalMb: total, processes: detail.length, detail, claudePids: claude };
}

/**
 * Fenêtres visibles de premier niveau appartenant aux pids donnés, avec leur
 * classe (une console serait `ConsoleWindowClass` ou
 * `CASCADIA_HOSTING_WINDOW_CLASS`).
 */
export function visibleWindows(pids) {
  const script = `
Add-Type @'
using System; using System.Text; using System.Runtime.InteropServices; using System.Collections.Generic;
public static class W {
  public delegate bool P(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(P p, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  public static List<string> All() {
    var r = new List<string>();
    EnumWindows((h, l) => { if (IsWindowVisible(h)) { uint pid; GetWindowThreadProcessId(h, out pid);
      var c = new StringBuilder(256); GetClassName(h, c, 256); var t = new StringBuilder(256); GetWindowText(h, t, 256);
      r.Add(pid + "\\t" + c + "\\t" + t); } return true; }, IntPtr.Zero);
    return r;
  }
}
'@
[W]::All()`;
  const set = new Set(pids);
  return ps(script)
    .split(/\r?\n/)
    .filter(Boolean)
    .map((l) => {
      const [pid, cls, title] = l.split("\t");
      return { pid: Number(pid), cls, title };
    })
    .filter((w) => set.has(w.pid));
}

/** Attend que le port CDP réponde. */
export async function waitCdp(port, ms = 30_000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    try {
      const r = await fetch(`http://127.0.0.1:${port}/json/version`);
      if (r.ok) return Date.now();
    } catch {}
    await sleep(50);
  }
  throw new Error(`CDP muet sur le port ${port}`);
}

/**
 * Lance l'app compilée. `args` : arguments de l'app (dossier du projet).
 * Variables : port CDP pour WebView2, journal dans `.claude-work/j12/logs`,
 * modèle haiku pour les claude.exe lancés (ANTHROPIC_MODEL).
 */
export async function launchApp({ args = [], port = 9333, env = {}, exe = EXE } = {}) {
  const logDir = join(WORK, "logs");
  mkdirSync(logDir, { recursive: true });
  const tSpawn = Date.now();
  const child = spawn(exe, args, {
    env: {
      ...process.env,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}`,
      ORCH_LOG_DIR: logDir,
      ANTHROPIC_MODEL: "haiku",
      ...env,
    },
    stdio: "ignore",
    detached: false,
  });
  const tCdp = await waitCdp(port);
  const browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  let page = null;
  for (let i = 0; i < 200 && !page; i++) {
    page = browser.contexts().flatMap((c) => c.pages())[0] ?? null;
    if (!page) await sleep(50);
  }
  if (!page) throw new Error("aucune page dans la webview");
  return { child, pid: child.pid, tSpawn, tCdp, browser, page };
}

/** Fermeture normale : WM_CLOSE (taskkill sans /F), ou forcée (/F). */
export function closeApp(pid, { force = false } = {}) {
  try {
    execFileSync("taskkill.exe", force ? ["/PID", String(pid), "/T", "/F"] : ["/PID", String(pid)], {
      stdio: "pipe",
    });
  } catch (e) {
    return String(e.stderr ?? e);
  }
  return "ok";
}

export function alive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

export async function waitGone(pids, ms) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (pids.every((p) => !alive(p))) return Date.now();
    await sleep(100);
  }
  return null;
}

export function save(name, data) {
  mkdirSync(WORK, { recursive: true });
  const f = join(WORK, name);
  writeFileSync(f, JSON.stringify(data, null, 2));
  return f;
}

export function log(...a) {
  console.log(new Date().toISOString().slice(11, 23), ...a);
}
