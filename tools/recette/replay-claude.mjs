// Faux claude.exe de la recette J12 : rejoue la session longue `t15` dans
// l'app compilée, à travers le vrai cœur Rust (lecteur, réducteur, lots,
// canal), sans réseau. Lancé par `replay-claude.cmd`, que l'app trouve par
// `ORCH_CLAUDE_PATH`.
//
// - `initialize` : réponse de `t11` (108 commandes, modèles), avec l'id reçu.
// - 1er message utilisateur : toutes les lignes de `t15` (les `>>> ` du
//   pilote deviennent des `user` rejoués, les `STDERR`/`EXIT` sont omises),
//   aussi vite que le tuyau les prend.
// - Messages suivants : un tour en streaming à `REPLAY_RATE` text_delta/s
//   (100 par défaut) pendant `REPLAY_MS` ms (5 000), puis `result`.
//   `REPLAY_SKIP_T15=1` : le 1er message donne aussi un tour en streaming.
// - `interrupt` : arrête le tour en cours (`result` aborted_streaming).
// - Autres `control_request` : succès vide.
// - Fin de stdin : sortie 0.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "../..");
const FIX = join(ROOT, "fixtures", "stream-json");
const RATE = Number(process.env.REPLAY_RATE ?? 100);
const MS = Number(process.env.REPLAY_MS ?? 5000);
// `REPLAY_SKIP_T15=1` : pas de t15, chaque message donne un tour en streaming.
const SKIP = process.env.REPLAY_SKIP_T15 === "1";
const SID = "00000000-0000-4000-8000-0000000012ab";

const initLine = readFileSync(join(FIX, "t11.jsonl"), "utf8")
  .split("\n")
  .find((l) => l.startsWith('{"type":"control_response"'));
const t15 = readFileSync(join(FIX, "t15.jsonl"), "utf8")
  .split("\n")
  .filter((l) => l && !/^(STDERR|EXIT)/.test(l))
  .map((l) => {
    if (!l.startsWith(">>> ")) return l;
    const o = JSON.parse(l.slice(4));
    if (o.type !== "user") return null;
    return JSON.stringify({ ...o, session_id: SID, parent_tool_use_id: null, isReplay: true });
  })
  .filter(Boolean);

const out = process.stdout;
function write(line) {
  return out.write(line + "\n") ? Promise.resolve() : new Promise((r) => out.once("drain", r));
}
const j = (o) => write(JSON.stringify({ ...o, session_id: SID }));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

let turns = 0;
let streaming = null; // { stop: boolean }
let uuidN = 0;
const uuid = () => `00000000-0000-4000-8000-${String(++uuidN).padStart(12, "0")}`;

async function dumpT15() {
  for (const l of t15) await write(l);
}

const WORDS = "le flux arrive mot après mot dans la conversation pendant que la mesure tourne".split(" ");

async function streamTurn(text) {
  const st = { stop: false };
  streaming = st;
  const msgId = `msg_replay_${turns}`;
  await j({ type: "system", subtype: "status", status: "requesting", uuid: uuid() });
  await j({ type: "user", message: { role: "user", content: text }, parent_tool_use_id: null, uuid: uuid(), isReplay: true });
  const ev = (event) => j({ type: "stream_event", event, parent_tool_use_id: null, uuid: uuid() });
  await ev({ type: "message_start", message: { model: "claude-haiku-5-5", id: msgId, type: "message", role: "assistant", content: [], usage: { input_tokens: 1, output_tokens: 1 } } });
  await ev({ type: "content_block_start", index: 0, content_block: { type: "text", text: "" } });
  let full = "";
  const end = Date.now() + MS;
  let n = 0;
  const t0 = Date.now();
  while (Date.now() < end && !st.stop) {
    const w = WORDS[n % WORDS.length] + (n % 12 === 11 ? ".\n\n" : " ");
    full += w;
    await ev({ type: "content_block_delta", index: 0, delta: { type: "text_delta", text: w } });
    n++;
    const next = t0 + (n * 1000) / RATE;
    const wait = next - Date.now();
    if (wait > 0) await sleep(wait);
  }
  streaming = null;
  if (st.stop) {
    await j({ type: "result", subtype: "error_during_execution", is_error: true, terminal_reason: "aborted_streaming", duration_ms: Date.now() - t0, duration_api_ms: 0, num_turns: 1, total_cost_usd: 0, uuid: uuid() });
    return;
  }
  await j({ type: "assistant", message: { model: "claude-haiku-5-5", id: msgId, type: "message", role: "assistant", content: [{ type: "text", text: full }], usage: { input_tokens: 1, output_tokens: n } }, parent_tool_use_id: null, uuid: uuid() });
  await ev({ type: "content_block_stop", index: 0 });
  await ev({ type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: n } });
  await ev({ type: "message_stop" });
  await j({ type: "result", subtype: "success", is_error: false, terminal_reason: "completed", result: full, duration_ms: Date.now() - t0, duration_api_ms: Date.now() - t0, num_turns: 1, total_cost_usd: 0, stop_reason: "end_turn", uuid: uuid() });
}

let queue = Promise.resolve();
const rl = createInterface({ input: process.stdin });
rl.on("line", (line) => {
  let o;
  try {
    o = JSON.parse(line);
  } catch {
    return;
  }
  if (o.type === "control_request") {
    const id = o.request_id;
    if (o.request?.subtype === "initialize") {
      const r = JSON.parse(initLine);
      r.response.request_id = id;
      write(JSON.stringify(r));
      return;
    }
    if (o.request?.subtype === "interrupt" && streaming) streaming.stop = true;
    write(JSON.stringify({ type: "control_response", response: { subtype: "success", request_id: id, response: {} } }));
    return;
  }
  if (o.type === "user") {
    turns++;
    const text = typeof o.message?.content === "string" ? o.message.content : "message";
    queue = queue.then(() => (turns === 1 && !SKIP ? dumpT15() : streamTurn(text)));
  }
});
rl.on("close", () => queue.then(() => process.exit(0)));
