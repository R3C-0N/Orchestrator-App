// Banc du démarrage de l'app entière (J11 branché, finitions) : `App.svelte`
// monté par `src/main.ts`, IPC Tauri simulé par `mockIPC`. Chargé par
// `startup.spec.ts` dans une page servie par Vite (`page.route`) ; la
// configuration du scénario est lue dans `window.__cfg` :
// - `fromArgs`, `launchError` : réponse de `get_launch_info` ;
// - `failStarts` : nombre de `start_session` qui échouent d'abord (`SpawnFailed`) ;
// - `historyError` : `load_history` illisible ; `omitted` : lignes omises ;
// - `version` : version de claude.exe annoncée par `init` (2.1.325 par défaut) ;
//   autre que 2.1.325, la notice `unexpected_version` est émise AVANT `ready`
//   et `init`, dans l'ordre du réducteur Rust (`check_version`).
// Les commandes reçues sont gardées dans `window.__calls`.
import { mockIPC } from "@tauri-apps/api/mocks";

type Cfg = {
  fromArgs: boolean;
  launchError: string | null;
  failStarts?: number;
  historyError?: boolean;
  omitted?: number;
  version?: string;
};
type Call = { cmd: string; args: unknown };
type Events = { onmessage: (batch: unknown[]) => void };

const w = window as unknown as { __cfg: Cfg; __calls: Call[] };
const cfg = w.__cfg;
w.__calls = [];
let starts = 0;

mockIPC((cmd, payload) => {
  const args = (payload ?? {}) as Record<string, unknown>;
  w.__calls.push({ cmd, args: cmd === "start_session" ? { cwd: args.cwd, resume: args.resume } : args });
  switch (cmd) {
    case "get_launch_info":
      return {
        cwd: "C:\\cwd",
        cwd_from_args: cfg.fromArgs,
        launch_error: cfg.launchError,
        claude_path: null,
        expected_version: "2.1.325",
      };
    case "list_projects":
      return [
        { encoded: "C--proj", path: "C:\\proj", exists: true, exists_checked: true, last_used: Date.now(), session_count: 2 },
      ];
    case "list_sessions":
      return [
        { id: "00000000-0000-4000-8000-000000000001", title: "Ma session", last_prompt: null, modified: Date.now(), size_bytes: 10, entrypoint: "cli" },
        { id: "00000000-0000-4000-8000-000000000002", title: "Autre session", last_prompt: null, modified: Date.now() - 60_000, size_bytes: 10, entrypoint: "cli" },
      ];
    case "start_session": {
      if (starts++ < (cfg.failStarts ?? 0)) throw { kind: "spawn_failed", message: "claude.exe absent (simulé)" };
      const events = args.events as Events;
      const key = `k${starts}`;
      const version = cfg.version ?? "2.1.325";
      const notice =
        version === "2.1.325"
          ? []
          : [
              {
                seq: 1, t: Date.now(), session: key, kind: "notice", level: "Warn", code: "unexpected_version",
                message: `claude.exe ${version} n'est pas la version testée (2.1.325) ; l'app continue.`,
                detail: { found: version, expected: "2.1.325" },
              },
            ];
      setTimeout(
        () =>
          events.onmessage([
            { seq: 0, t: Date.now(), session: key, kind: "status", state: "Idle" },
            ...notice,
            {
              seq: 2, t: Date.now(), session: key, kind: "ready", version, capabilities: [], commands: [],
              agents: [], models: [], permission_mode: "bypassPermissions",
            },
            {
              seq: 3, t: Date.now(), session: key, kind: "init", session_id: "s", version, model: "claude-haiku-5-5",
              permission_mode: "bypassPermissions", capabilities: [], tools: [], mcp_servers: [],
              skills: [`skill-de-${key}`], slash_commands: [],
            },
          ]),
        10,
      );
      return { session: key, pid: 1, claude_path: "x" };
    }
    case "stop_session":
      return { code: 0, killed: false };
    case "send_message":
      return null;
    case "load_history":
      if (cfg.historyError) throw { kind: "transcript_format", message: "illisible" };
      return {
        session_id: args.session_id,
        events: [
          { seq: 0, t: Date.now(), session: "k", kind: "user_message", thread: "main", uuid: null, text: `bonjour ${String(args.session_id).slice(-1)}`, origin: "History" },
        ],
        lines: 1, used: 1, ignored: 0, unknown: 0, unknown_elements: 0, bad: 0, sidechain: 0,
        truncated: (cfg.omitted ?? 0) > 0, subagent_threads: 0,
        omitted_lines: cfg.omitted ?? 0, omitted_bytes: 0, clipped: 0, dropped_events: 0,
      };
  }
  return null;
});

await import("../src/main");
