// Génère fixtures/transcripts/projects : transcripts SYNTHÉTIQUES (aucun contenu réel).
// Usage : node fixtures/transcripts/generate.mjs fixtures/transcripts
import fs from "node:fs";
import path from "node:path";
const OUT = process.argv[2];
const CWD = "C:\\Users\\<user>\\Code\\demo";
const ENC = CWD.replace(/[^A-Za-z0-9]/g, "-");
const S1 = "0b8f3c2e-1111-4a6b-9c1d-000000000001"; // complète
const S2 = "0b8f3c2e-1111-4a6b-9c1d-000000000002"; // tronquée
const S3 = "0b8f3c2e-1111-4a6b-9c1d-000000000003"; // illisible
const S4 = "0b8f3c2e-1111-4a6b-9c1d-000000000004"; // lancée en -p (sdk-cli), sans ai-title
let clock = Date.UTC(2026, 10, 16, 9, 0, 0);
const ts = () => new Date((clock += 1000)).toISOString();
let n = 0;
const uid = () => `00000000-0000-4000-8000-${String(++n).padStart(12, "0")}`;
const common = (sid, entry = "cli") => ({ userType: "external", entrypoint: entry, cwd: CWD, sessionId: sid, version: "2.1.325", gitBranch: "main" });
const user = (sid, content, extra = {}) => ({ parentUuid: null, isSidechain: false, type: "user", message: { role: "user", content }, uuid: uid(), timestamp: ts(), ...extra, ...common(sid, extra.entrypoint) });
const asst = (sid, id, block, extra = {}) => ({ parentUuid: null, isSidechain: false, message: { id, type: "message", role: "assistant", model: "claude-haiku-5-5", content: [block], stop_reason: null, usage: { input_tokens: 10, output_tokens: 5 } }, requestId: "req_synthetic", type: "assistant", uuid: uid(), timestamp: ts(), ...extra, ...common(sid) });
const sys = (sid, subtype, extra) => ({ parentUuid: null, isSidechain: false, type: "system", subtype, ...extra, timestamp: ts(), uuid: uid(), ...common(sid) });
const tr = (id, content, extra = {}) => ({ type: "tool_result", tool_use_id: id, content, ...extra });
const README = `${CWD}\\README.md`;
const BROKEN = "@@BROKEN@@";

const full = [
  { type: "mode", mode: "normal", sessionId: S1 },
  { type: "permission-mode", permissionMode: "bypassPermissions", sessionId: S1 },
  { type: "file-history-snapshot", messageId: "m0", snapshot: { trackedFileBackups: {} }, isSnapshotUpdate: false },
  { parentUuid: null, isSidechain: false, attachment: { type: "date", date: "2026-11-16" }, type: "attachment", uuid: uid(), timestamp: ts(), ...common(S1) },
  user(S1, "Lis le fichier README et résume-le.", { origin: { kind: "human" } }),
  user(S1, "<system-reminder>\nRappel interne synthétique.\n</system-reminder>", { isMeta: true }),
  asst(S1, "msg_01", { type: "thinking", thinking: "", signature: "c2lnbmF0dXJl" }),
  asst(S1, "msg_01", { type: "text", text: "Je lis le fichier." }),
  asst(S1, "msg_01", { type: "tool_use", id: "toolu_r1", name: "Read", input: { file_path: README } }),
  user(S1, [tr("toolu_r1", "1\t# Démo\n2\tUn projet de démonstration.")], { toolUseResult: { type: "text", file: { filePath: README, content: "# Démo\nUn projet de démonstration.", numLines: 2, startLine: 1, totalLines: 2 } } }),
  asst(S1, "msg_02", { type: "text", text: "Le README décrit **un projet de démonstration**." }),
  sys(S1, "turn_duration", { durationMs: 6000, messageCount: 4, isMeta: true }),
  { type: "brand-new-type", sessionId: S1, payload: { x: 1 } },
  BROKEN,
  sys(S1, "local_command", { content: "<command-name>/model</command-name>\n            <command-message>model</command-message>\n            <command-args>haiku</command-args>", level: "info", isMeta: false }),
  sys(S1, "local_command", { content: "<local-command-stdout>Set model to haiku</local-command-stdout>", level: "info", isMeta: false }),
  user(S1, "Modifie le titre puis lance deux agents en arrière-plan et un agent normal.", { origin: { kind: "human" } }),
  asst(S1, "msg_03", { type: "tool_use", id: "toolu_e1", name: "Edit", input: { file_path: README, old_string: "# Démo", new_string: "# Démo 2", replace_all: false } }),
  user(S1, [tr("toolu_e1", `The file ${README} has been updated successfully.`)], { toolUseResult: { filePath: README, oldString: "# Démo", newString: "# Démo 2", originalFile: "# Démo\nUn projet de démonstration.\n", structuredPatch: [{ oldStart: 1, oldLines: 1, newStart: 1, newLines: 1, lines: ["-# Démo", "+# Démo 2"] }], userModified: false, replaceAll: false } }),
  asst(S1, "msg_03", { type: "tool_use", id: "toolu_a1", name: "Agent", input: { description: "Compter les fichiers", subagent_type: "\u00a0agent-bleu\u00a0", prompt: "Compte les fichiers." } }),
  asst(S1, "msg_03", { type: "tool_use", id: "toolu_a3", name: "Agent", input: { description: "Lister les tests", subagent_type: "agent-vert", prompt: "Liste les tests." } }),
  asst(S1, "msg_03", { type: "tool_use", id: "toolu_a2", name: "Agent", input: { description: "Relire le diff", subagent_type: "general-purpose", prompt: "Relis le diff." } }),
  user(S1, [tr("toolu_a1", [{ type: "text", text: "Async agent launched successfully." }])], { toolUseResult: { isAsync: true, status: "async_launched", agentId: "a1", description: "Compter les fichiers" } }),
  user(S1, [tr("toolu_a3", [{ type: "text", text: "Async agent launched successfully." }])], { toolUseResult: { isAsync: true, status: "async_launched", agentId: "a3", description: "Lister les tests" } }),
  { ...asst(S1, "msg_side", { type: "text", text: "Ligne de fil latéral : ne doit pas apparaître." }), isSidechain: true },
  user(S1, [tr("toolu_a2", [{ type: "text", text: "Le diff est correct." }])], { toolUseResult: { status: "completed", content: [{ type: "text", text: "Le diff est correct." }], totalTokens: 1200, totalToolUseCount: 2 } }),
  asst(S1, "msg_04", { type: "text", text: "Les agents tournent en arrière-plan." }),
  asst(S1, "msg_04", { type: "fancy_block", data: 1 }),
  sys(S1, "turn_duration", { durationMs: 4000, messageCount: 6, isMeta: true }),
  user(S1, "<task-notification>\n<task-id>a1</task-id>\n<tool-use-id>toolu_a1</tool-use-id>\n<output-file>C:\\Users\\<user>\\AppData\\Local\\Temp\\a1.output</output-file>\n<status>completed</status>\n<summary>Agent \"Compter les fichiers\" completed</summary>\n<result>3 fichiers</result>\n</task-notification>", { origin: { kind: "task-notification" } }),
  asst(S1, "msg_05", { type: "text", text: "L'agent a trouvé 3 fichiers." }),
  sys(S1, "turn_duration", { durationMs: 2000, messageCount: 2, isMeta: true }),
  user(S1, "Encore une chose", { origin: { kind: "human" } }),
  asst(S1, "msg_06", { type: "text", text: "Je commence" }),
  user(S1, [{ type: "text", text: "[Request interrupted by user]" }]),
  { type: "queue-operation", operation: "enqueue", timestamp: ts(), sessionId: S1, content: "x" },
  { type: "ai-title", aiTitle: "Résumé du README de démo", sessionId: S1 },
  { type: "last-prompt", lastPrompt: "Encore une chose", leafUuid: uid(), sessionId: S1 },
];

const sub = [
  { parentUuid: null, isSidechain: true, agentId: "a1", type: "user", message: { role: "user", content: "Compte les fichiers." }, uuid: uid(), timestamp: ts(), ...common(S1) },
  { parentUuid: null, isSidechain: true, agentId: "a1", type: "assistant", message: { id: "msg_s1", role: "assistant", content: [{ type: "text", text: "Sous-agent : 3 fichiers" }] }, uuid: uid(), timestamp: ts(), ...common(S1) },
];

const line = (o) => (o === BROKEN ? '{"type":"user","message":{"role":"user","content":"ligne abîmée' : JSON.stringify(o));
const dir = path.join(OUT, "projects", ENC);
fs.mkdirSync(path.join(dir, S1, "subagents"), { recursive: true });
fs.writeFileSync(path.join(dir, `${S1}.jsonl`), full.map(line).join("\n") + "\n");
fs.writeFileSync(path.join(dir, S1, "subagents", "agent-a1.jsonl"), sub.map(line).join("\n") + "\n");
fs.writeFileSync(path.join(dir, S1, "subagents", "agent-a1.meta.json"), JSON.stringify({ agentType: " agent-bleu ", isBuiltIn: false, description: "Compter les fichiers", toolUseId: "toolu_a1", spawnDepth: 1, requestShape: "background" }));
fs.writeFileSync(path.join(dir, S1, "subagents", "agent-a9.meta.json"), JSON.stringify({ agentType: "agent-rouge", isBuiltIn: false, description: "Agent connu par sa méta seulement", toolUseId: "toolu_a9", spawnDepth: 1, requestShape: "background" }));
fs.writeFileSync(path.join(dir, S1, "subagents", "agent-abad.meta.json"), "{ méta abîmée");

// Tronquée : le début de la session complète (sans la ligne abîmée), puis une ligne coupée au milieu.
const trunc = full.slice(0, 12).filter((o) => o !== BROKEN).map((o) => JSON.stringify({ ...o, sessionId: S2 }));
const cut = JSON.stringify(asst(S2, "msg_99", { type: "text", text: "Réponse coupée par la fin du fichier" }));
fs.writeFileSync(path.join(dir, `${S2}.jsonl`), trunc.join("\n") + "\n" + cut.slice(0, Math.floor(cut.length / 2)));

fs.writeFileSync(path.join(dir, `${S3}.jsonl`), "ceci n'est pas du JSON\n<<<>>>\n{\"type\":\n");

const sdk = [
  user(S4, "Réponds juste OK.", { entrypoint: "sdk-cli" }),
  { ...asst(S4, "msg_p1", { type: "text", text: "OK" }), entrypoint: "sdk-cli" },
  { type: "last-prompt", lastPrompt: "Réponds juste OK.", leafUuid: uid(), sessionId: S4 },
];
fs.writeFileSync(path.join(dir, `${S4}.jsonl`), sdk.map((o) => JSON.stringify(o)).join("\n") + "\n");

// Renommée par `/rename` : le CLI écrit `custom-title` juste avant `ai-title`,
// puis la conversation continue (plus de 16 ko) avant le dernier `last-prompt`.
const S6 = "0b8f3c2e-1111-4a6b-9c1d-000000000006";
const filler = (i) => ({ parentUuid: null, isSidechain: false, attachment: { type: "synthetic_padding", text: "x".repeat(4000) }, type: "attachment", uuid: uid(), timestamp: ts(), ...common(S6), n: i });
const renamed = [
  user(S6, "Première demande.", { origin: { kind: "human" } }),
  { type: "custom-title", customTitle: "Titre choisi par /rename", sessionId: S6 },
  { type: "ai-title", aiTitle: "Titre proposé par le modèle", sessionId: S6 },
  ...Array.from({ length: 6 }, (_, i) => filler(i)),
  user(S6, "Dernière demande.", { origin: { kind: "human" } }),
  { type: "last-prompt", lastPrompt: "Dernière demande.", leafUuid: uid(), sessionId: S6 },
];
fs.writeFileSync(path.join(dir, `${S6}.jsonl`), renamed.map((o) => JSON.stringify(o)).join("\n") + "\n");

// Projet disparu, nom ambigu (`a-b` et `a\b` s'encodent pareil) : le vrai chemin vient de `cwd`.
const GONE = "D:\\disparu\\a-b";
const gdir = path.join(OUT, "projects", GONE.replace(/[^A-Za-z0-9]/g, "-"));
fs.mkdirSync(gdir, { recursive: true });
const G = "0b8f3c2e-1111-4a6b-9c1d-000000000005";
fs.writeFileSync(path.join(gdir, `${G}.jsonl`), [
  { type: "mode", mode: "normal", sessionId: G },
  { parentUuid: null, isSidechain: false, type: "user", message: { role: "user", content: "Bonjour" }, uuid: uid(), timestamp: ts(), userType: "external", entrypoint: "cli", cwd: GONE, sessionId: G, version: "2.1.325" },
  { type: "ai-title", aiTitle: "Salutations", sessionId: G },
].map((o) => JSON.stringify(o)).join("\n") + "\n");
// Dossier sans transcript : ignoré par list_projects.
fs.mkdirSync(path.join(OUT, "projects", "C--vide", "memory"), { recursive: true });
fs.writeFileSync(path.join(OUT, "projects", "C--vide", "memory", "README.txt"), "dossier sans transcript\n");
console.log("ok", dir);
