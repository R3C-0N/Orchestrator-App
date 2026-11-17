// Scénarios de fidélité du jalon J5 : pour chaque capture de référence du
// TUI 2.1.325 (J1, `.claude-work/j1/captures/`, non versionnées), la même
// situation en `CoreEvent` conformes au contrat, rejouée dans le magasin en
// mode simulé. Les textes reprennent ceux des captures (`.uia.txt`) ; les
// chemins personnels du bac à sable sont remplacés par un dossier neutre.

import type { EventBody } from "../src/lib/core/bindings/EventBody";

export type Step = { body: EventBody; t: number };
export type Scenario = {
  /** Identifiant de la capture (`02`, `03b`…). */
  id: string;
  /** Fichier PNG de référence (sans extension). */
  ref: string;
  /** Vue détaillée (Ctrl+O). */
  verbose: boolean;
  steps: Step[];
  /** Horodatages relatifs à l'instant du rejeu (états en cours, durées vivantes). */
  relative?: boolean;
  /** Attente avant la capture (durées vivantes). */
  settleMs?: number;
};

const SANDBOX = "C:\\projets\\demo";

/** Heure locale du jour de capture (le TUI affiche l'heure locale). */
function at(h: number, m: number, s = 0, ms = 0): number {
  return new Date(2026, 10, 16, h, m, s, ms).getTime();
}

class Builder {
  steps: Step[] = [];
  #t: number;
  #msg = 0;
  #tool = 0;

  constructor(start: number) {
    this.#t = start;
  }

  #push(body: EventBody, dt = 200): void {
    this.#t += dt;
    this.steps.push({ body, t: this.#t });
  }

  setTime(t: number): this {
    this.#t = t;
    return this;
  }

  boot(cwd: string, model = "claude-haiku-5-5"): this {
    this.#push({ kind: "starting", pid: 1, cwd, args: [] });
    this.#push({
      kind: "init",
      session_id: "11111111-2222-4333-8444-555555555555",
      model,
      permission_mode: "bypassPermissions",
      capabilities: [],
      tools: [],
      mcp_servers: [],
      skills: [],
      slash_commands: [],
      version: "2.1.325",
    });
    return this;
  }

  user(text: string): this {
    this.#push({ kind: "user_message", thread: "main", uuid: null, text, origin: "Typed" });
    return this;
  }

  status(state: "Idle" | "Requesting" | "Streaming"): this {
    this.#push({ kind: "status", state });
    return this;
  }

  text(text: string, dt = 500): this {
    const msg = `msg_${this.#msg++}`;
    this.#push({ kind: "block_start", thread: "main", msg, index: 0, block: { type: "Text" } }, dt);
    this.#push({ kind: "block_done", thread: "main", msg, index: 0, block: { type: "Text", text } }, 10);
    return this;
  }

  /** Appel d'outil ; `result` absent = appel resté sans résultat. */
  tool(
    name: string,
    input: Record<string, unknown>,
    result?: { text: string; isError?: boolean; meta?: unknown },
    startAt?: number,
  ): this {
    const id = `toolu_${this.#tool++}`;
    const msg = `msg_${this.#msg++}`;
    if (startAt !== undefined) this.#t = startAt;
    this.#push({ kind: "block_start", thread: "main", msg, index: 0, block: { type: "ToolUse", tool_use_id: id, name } });
    this.#push(
      { kind: "block_done", thread: "main", msg, index: 0, block: { type: "ToolUse", tool_use_id: id, name, input } as never },
      10,
    );
    if (result)
      this.#push({
        kind: "tool_result",
        thread: "main",
        tool_use_id: id,
        content: [{ type: "Text", text: result.text }],
        is_error: result.isError ?? false,
        meta: (result.meta ?? null) as never,
      });
    return this;
  }

  agent(a: {
    id: string;
    type: string;
    color: string;
    description: string;
    status: "Running" | "Completed";
    started: number;
    ended?: number;
    summary?: string;
  }): this {
    this.#push({
      kind: "agent",
      tool_use_id: a.id,
      task_id: `task_${a.id}`,
      agent_type: a.type,
      color: a.color,
      description: a.description,
      status: a.status,
      background: true,
      started_at: a.started,
      ended_at: a.ended ?? null,
      last_tool: null,
      tokens: null,
      tool_uses: null,
      summary: a.summary ?? null,
    });
    return this;
  }

  /** Bloc Thinking (vide, comme le CLI) entre `from` et `to`. */
  thinking(from: number, to: number): this {
    const msg = `msg_${this.#msg++}`;
    this.#t = from;
    this.#push({ kind: "block_start", thread: "main", msg, index: 0, block: { type: "Thinking" } }, 0);
    this.#t = to;
    this.#push({ kind: "block_done", thread: "main", msg, index: 0, block: { type: "Thinking", empty: true } }, 0);
    return this;
  }

  interrupted(): this {
    this.#push({
      kind: "user_message",
      thread: "main",
      uuid: null,
      text: "[Request interrupted by user]",
      origin: "Interrupted",
    });
    return this;
  }

  turn(durationMs: number, endAt: number, subtype = "success"): this {
    this.#t = endAt;
    this.#push(
      {
        kind: "turn_result",
        subtype,
        is_error: subtype !== "success",
        terminal_reason: null,
        duration_ms: durationMs,
        cost_usd: 0,
        num_turns: 1,
        context_used: null,
        context_window: null,
        permission_denials: [],
        result: null,
      },
      0,
    );
    this.#push({ kind: "status", state: "Idle" }, 0);
    return this;
  }
}

// --- 02 : markdown et code ---------------------------------------------------

const MD_02 = [
  "## Titre",
  "",
  "### Sous-titre",
  "",
  "- Un élément en **gras**",
  "- Un élément en *italique*",
  "- Un élément avec `code en ligne`",
  "",
  "1. Premier élément",
  "2. Deuxième élément",
  "",
  "```python",
  "def saluer(nom):",
  '    message = f"Bonjour, {nom} !"',
  "    print(message)",
  "",
  "",
  'if __name__ == "__main__":',
  '    saluer("monde")',
  '    saluer("Claude")',
  '    print("Fin du programme.")',
  "```",
].join("\n");

function s02(): Scenario {
  const b = new Builder(at(12, 20, 0) - 6000)
    .boot(`${SANDBOX}\\02-markdown`)
    .user(
      "Sans utiliser d'outil, reponds en markdown : un titre ##, un sous-titre ###, une liste a puces de 3 elements avec un mot en **gras**, un mot en *italique* et du `code en ligne`, une liste numerotee de 2 elements, puis un bloc de code python de 8 lignes.",
    )
    .text(MD_02)
    .turn(6000, at(12, 20, 5));
  return { id: "02", ref: "02-markdown-code", verbose: false, steps: b.steps };
}

// --- 03 / 03b : Read + Bash regroupés, puis vue détaillée --------------------

function s03(verbose: boolean): Scenario {
  const cwd = `${SANDBOX}\\03-outils`;
  const notes = Array.from({ length: 8 }, (_, i) => `${String(i + 1).padStart(6)}\tligne ${i + 1} des notes`).join("\n");
  const b = new Builder(at(13, 12, 50))
    .boot(cwd)
    .user(
      "Lis notes.txt avec l'outil Read, puis lance la commande Bash `seq 1 5 | tee nombres.txt`. Ne commente pas, reponds juste ok.",
    )
    .tool("Read", { file_path: `${cwd}\\notes.txt` }, { text: notes })
    .tool(
      "Bash",
      { command: "seq 1 5 | tee nombres.txt", description: "Écrire les nombres 1 à 5" },
      { text: "1\n2\n3\n4\n5" },
    )
    .setTime(at(13, 13, 5))
    .text("ok")
    .turn(17000, at(13, 13, 7));
  return { id: verbose ? "03b" : "03", ref: verbose ? "03b-outils-detail-ctrl-o" : "03-outils-tronque", verbose, steps: b.steps };
}

// --- 04 : Edit avec diff -------------------------------------------------------

const CONFIG = '"""Réglages du client de démonstration."""\n\nHOST = "localhost"\nPORT = 8080\n\nTIMEOUT = 30\nRETRIES = 3\n\nLOG_LEVEL = "info"\n\n\ndef url() -> str:\n    return f"http://{HOST}:{PORT}"\n';

function s04(): Scenario {
  const cwd = `${SANDBOX}\\04-edit`;
  const path = `${cwd}\\config.py`;
  const numbered = CONFIG.replace(/\n$/, "")
    .split("\n")
    .map((l, i) => `${i + 1}\t${l}`)
    .join("\n");
  const b = new Builder(at(12, 21, 0) - 14000)
    .boot(cwd)
    .user(
      "Dans config.py, avec un seul appel de l'outil Edit, remplace les deux lignes TIMEOUT = 30 et RETRIES = 3 par les trois lignes TIMEOUT = 60, RETRIES = 5 et BACKOFF = 1.5. Puis reponds juste ok.",
    )
    .tool("Glob", { pattern: "config.py" }, { text: "config.py" })
    .tool("Read", { file_path: path }, { text: numbered })
    .tool(
      "Edit",
      { file_path: path, old_string: "TIMEOUT = 30\nRETRIES = 3", new_string: "TIMEOUT = 60\nRETRIES = 5\nBACKOFF = 1.5", replace_all: false },
      {
        text: `The file ${path} has been updated successfully.`,
        // Forme de `tool_use_result` relevée dans la fixture t13.
        meta: {
          filePath: path,
          oldString: "TIMEOUT = 30\nRETRIES = 3",
          newString: "TIMEOUT = 60\nRETRIES = 5\nBACKOFF = 1.5",
          originalFile: CONFIG,
          structuredPatch: [
            {
              oldStart: 3,
              oldLines: 9,
              newStart: 3,
              newLines: 10,
              lines: [
                ' HOST = "localhost"',
                " PORT = 8080",
                " ",
                "-TIMEOUT = 30",
                "-RETRIES = 3",
                "+TIMEOUT = 60",
                "+RETRIES = 5",
                "+BACKOFF = 1.5",
                " ",
                ' LOG_LEVEL = "info"',
                " ",
              ],
            },
          ],
          userModified: false,
          replaceAll: false,
        },
      },
    )
    .text("ok")
    .turn(14000, at(12, 21, 0));
  return { id: "04", ref: "04-edit-diff", verbose: false, steps: b.steps };
}

// --- 05 / 06 : sous-agents en arrière-plan ------------------------------------

const PROMPT_05 =
  "Lance en parallele, dans un seul message, deux sous-agents avec l'outil Agent : agent-bleu et agent-rouge. Chacun doit executer la commande Bash `ping -n 30 127.0.0.1` puis repondre fini. Quand les deux ont fini, resume en une ligne.";

function s05(): Scenario {
  const t0 = at(12, 25, 50);
  const b = new Builder(t0)
    .boot(`${SANDBOX}\\05-agents`)
    .user(PROMPT_05)
    .agent({ id: "toolu_bleu", type: " agent-bleu ", color: "blue", description: "Ping bleu agent", status: "Running", started: t0 })
    .agent({ id: "toolu_rouge", type: " agent-rouge ", color: "red", description: "Ping rouge agent", status: "Running", started: t0 })
    .text(
      "Les deux sous-agents, agent-bleu et agent-rouge, tournent en parallèle en arrière-plan, chacun sur `ping -n 30 127.0.0.1`. Je fais le résumé en une ligne quand ils auront tous les deux répondu « fini ».",
    )
    .turn(5000, at(12, 26, 0));
  return { id: "05", ref: "05-agents-en-cours", verbose: false, steps: b.steps };
}

function s06(): Scenario {
  const t0 = at(12, 30, 38);
  const b = new Builder(t0)
    .boot(`${SANDBOX}\\05-agents`)
    .user(PROMPT_05)
    .agent({ id: "toolu_bleu", type: " agent-bleu ", color: "blue", description: "Ping bleu", status: "Running", started: t0 })
    .agent({ id: "toolu_rouge", type: " agent-rouge ", color: "red", description: "Ping rouge", status: "Running", started: t0 })
    .text(
      "Les deux sous-agents, agent-bleu et agent-rouge, tournent en parallèle. Chacun exécute `ping -n 30 127.0.0.1` et répond « fini ». Je fais le résumé une fois que les deux ont terminé.",
    )
    .turn(4000, t0 + 4000)
    .setTime(t0 + 42000)
    .agent({ id: "toolu_rouge", type: " agent-rouge ", color: "red", description: "Ping rouge", status: "Completed", started: t0, ended: t0 + 42000, summary: "fini" })
    .text(
      "Un seul des deux sous-agents a fini pour l'instant : celui qui a répondu « fini » est agent-rouge. Je n'ai pas encore le résultat d'agent-bleu, donc je ne peux pas encore faire le résumé d'une ligne. Je le ferai quand il aura terminé.",
    )
    .agent({ id: "toolu_bleu", type: " agent-bleu ", color: "blue", description: "Ping bleu", status: "Completed", started: t0, ended: t0 + 43000, summary: "fini" })
    .text(
      "Les deux sous-agents ont fini : agent-bleu et agent-rouge ont chacun répondu « fini » après avoir exécuté `ping -n 30 127.0.0.1`.",
    )
    .turn(53000, at(12, 31, 31));
  return { id: "06", ref: "06-agents-termines", verbose: false, steps: b.steps };
}

// --- 08 : interruption ---------------------------------------------------------

function s08(): Scenario {
  const b = new Builder(at(12, 34, 0))
    .boot(`${SANDBOX}\\08-interruption`)
    .user("Lance la commande Bash `ping -n 90 127.0.0.1`, puis dis termine.")
    .tool("Bash", { command: "ping -n 90 127.0.0.1", description: "Ping for 90 requests to localhost" })
    .interrupted()
    .turn(9000, at(12, 34, 10), "error_during_execution");
  return { id: "08", ref: "08-interruption", verbose: false, steps: b.steps };
}

// --- 09 / 09b : erreurs ----------------------------------------------------------

const ANSWER_09 = [
  "Pas tout à fait fait : sur les trois actions, deux ont échoué.",
  "",
  "1. **Edit sur config.py** : échec, la chaîne `INEXISTANT = 0` est introuvable dans",
  "   `C:\\projets\\demo\\09-erreurs\\config.py`. Je n'ai pas relu le fichier, donc le fichier n'a pas été modifié.",
  "2. **`mkdir dossier-absent/sous-dossier`** : échec, `No such file or directory`. Le dossier parent `dossier-absent` n'existe pas, et `mkdir` sans `-p` ne le crée pas.",
  "3. **`exit 3`** : code de sortie 3, comme demandé.",
].join("\n");

function s09(verbose: boolean): Scenario {
  const cwd = `${SANDBOX}\\09-erreurs`;
  const b = new Builder(at(12, 29, 0) - 23000)
    .boot(cwd)
    .user(
      "Fais ces trois actions l'une apres l'autre, sans rien corriger ni relire : 1) avec l'outil Edit, dans config.py, remplace la ligne INEXISTANT = 0 par X = 1 ; 2) lance en Bash `mkdir dossier-absent/sous-dossier` ; 3) lance en Bash `exit 3`. Puis reponds juste fait.",
    )
    .tool(
      "Edit",
      { file_path: `${cwd}\\config.py`, old_string: "INEXISTANT = 0", new_string: "X = 1", replace_all: false },
      { text: "<tool_use_error>String to replace not found in file.\nString: INEXISTANT = 0</tool_use_error>", isError: true },
    )
    .tool(
      "Bash",
      { command: "mkdir dossier-absent/sous-dossier", description: "Créer un sous-dossier" },
      { text: "Exit code 1\nmkdir: cannot create directory 'dossier-absent/sous-dossier': No such file or directory", isError: true },
    )
    .tool("Bash", { command: "exit 3", description: "Sortir avec le code 3" }, { text: "Exit code 3", isError: true })
    // Horodatage de la réponse : 12:29 (« 12:29 PM claude-haiku-5-5 », 09b).
    .setTime(at(12, 28, 59, 600))
    .text(ANSWER_09)
    .turn(23000, at(12, 29, 2));
  return { id: verbose ? "09b" : "09", ref: verbose ? "09b-erreurs-detail-ctrl-o" : "09-erreurs", verbose, steps: b.steps };
}

// --- k1 : tour en cours ------------------------------------------------------------

function sK1(): Scenario {
  // Horodatages relatifs : 0 = instant du rejeu dans la page.
  const now = 0;
  const b = new Builder(now - 8000)
    .boot(`${SANDBOX}\\ctrlc-tour`)
    .user("Lance la commande Bash `ping -n 90 127.0.0.1`, puis dis termine.")
    .status("Requesting")
    .status("Streaming")
    // Réflexion d'une seconde : « thought for 1s » (k1).
    .thinking(now - 7000, now - 6000)
    .tool("Bash", { command: "ping -n 90 127.0.0.1", description: "Ping for 90 requests to localhost" }, undefined, now - 3000);
  return { id: "k1", ref: "k1-tour-en-cours-saisie-vide", verbose: false, steps: b.steps, relative: true, settleMs: 1200 };
}

// --- w1 / w1b : Write d'un nouveau fichier (capture J13) ------------------------

const LISTE = "alpha\nbeta\ngamma\ndelta\nepsilon\nzeta\neta\ntheta\niota\nkappa\nlambda\nmu\n";

function sW1(verbose: boolean): Scenario {
  const cwd = `${SANDBOX}\\write`;
  const path = `${cwd}\\liste.txt`;
  const b = new Builder(at(8, 8, 0) - 15000)
    .boot(cwd)
    .user(
      "Avec un seul appel de l outil Write, cree le nouveau fichier liste.txt contenant exactement ces 12 lignes : alpha, beta, gamma, delta, epsilon, zeta, eta, theta, iota, kappa, lambda, mu (un mot par ligne). Ne lis aucun fichier avant. Puis reponds juste ok.",
    )
    .tool(
      "Write",
      { file_path: path, content: LISTE },
      {
        text: `File created successfully at: ${path}`,
        // Forme de `tool_use_result` relevée dans la fixture t13 (`type: "create"`).
        meta: { type: "create", filePath: path, content: LISTE, structuredPatch: [], originalFile: null, userModified: false },
      },
    )
    .setTime(at(8, 8, 0))
    .text("ok")
    .turn(15000, at(8, 8, 1));
  return { id: verbose ? "w1b" : "w1", ref: verbose ? "w1b-write-detail-ctrl-o" : "w1-write-normal", verbose, steps: b.steps };
}

export function scenarios(): Scenario[] {
  return [s02(), s03(false), s03(true), s04(), s05(), s06(), s08(), s09(false), s09(true), sK1(), sW1(false), sW1(true)];
}
