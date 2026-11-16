#!/usr/bin/env node
// Genere fixtures/stream-json/t15.jsonl : session longue synthetique (plan, section 1.11),
// en concatenant et renumerotant des tours pris dans les fixtures deja anonymisees.
//
// Usage : node gen-t15.mjs [sortie.jsonl]     (defaut : fixtures/stream-json/t15.jsonl)
//
// - En tete : les lignes de t13 jusqu'a son `system/init` inclus (hooks de demarrage + init).
// - Puis N tours. Un tour = de la ligne `>>> {"type":"user"...}` jusqu'au `result` inclus, pris
//   dans un modele (tableau MIX). Sont retires des tours : `system/init`, `hook_*`, requetes et
//   reponses de controle (`control_request`, `>>> {"type":"control_response"...}`), `EXIT`, `STDERR`.
//   Quand la fixture n'a pas journalise le message envoye (t3), une ligne `>>> ` est reconstituee (SYNTH).
// - Renumerotation par copie : chaque UUID, `toolu_`, `msg_`, `req_` et identifiant d'agent ou
//   de tache recoit un suffixe propre a la copie (deterministe) ; tous les `session_id` prennent
//   celui de l'en-tete. Les `stream_event` (deltas) ne sont gardes que dans 1 copie sur 10. Les textes, horodatages et sorties d'outils sont gardes tels quels.
// Le script est deterministe : relancer donne le meme fichier.

import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const fx = resolve(here, '..', '..', 'fixtures', 'stream-json');
const out = process.argv[2] || join(fx, 't15.jsonl');

// [fixture, numero du tour dans la fixture (0 = premier), nombre de copies]
const MIX = [
  ['t13', 0, 80],  // Glob + Read + Edit (diff) + Write : 80 diffs, 320 appels d'outils
  ['t3', 0, 20],   // 2 sous-agents en parallele : 40 sous-agents
  ['t14', 0, 40],  // Read en erreur + Bash code non nul + result en erreur
  ['t1', 0, 60],   // Read
  ['t1', 1, 40],   // Read
  ['t9', 0, 20],   // Bash refuse
  ['t10', 0, 20],  // AskUserQuestion ignoree
  ['t8', 0, 10],   // Bash + AskUserQuestion
  ['t6', 0, 160],  // tour interrompu en plein texte (nombres 1 a 600)
  ['t6', 1, 200],  // texte seul
  ['t5', 2, 150],  // texte seul
  ['t7', 0, 200],  // reprise, texte seul
];

// Message utilisateur reconstitue pour les fixtures dont l'envoi n'a pas ete journalise.
const SYNTH = { t3: 'Lance en parallele deux sous-agents : agent-bleu et agent-rouge.' };

const STREAM_EVERY = 10;

const read = (n) => readFileSync(join(fx, n + '.jsonl'), 'utf8').split(/\r?\n/).filter(Boolean);
const body = (l) => (l.startsWith('>>> ') ? l.slice(4) : l);
const parse = (l) => { try { return JSON.parse(body(l)); } catch { return null; } };

function keep(l) {
  if (l.startsWith('EXIT ') || l.startsWith('STDERR ')) return false;
  const o = parse(l);
  if (!o) return false;
  if (o.type === 'control_request' || o.type === 'control_response' || o.type === 'control_cancel_request') return false;
  if (o.type === 'system' && (o.subtype === 'init' || String(o.subtype).startsWith('hook_'))) return false;
  return true;
}

function turns(name) {
  const res = [];
  let cur = null;
  for (const l of read(name)) {
    const o = parse(l);
    if (l.startsWith('>>> ') && o?.type === 'user') cur = [l];
    else if (cur && keep(l)) cur.push(l);
    // Pas de ligne `>>> ` : t1 a l'echo du CLI (--replay-user-messages), garde tel quel ;
    // t3 n'a rien (envoi non journalise) : message utilisateur reconstitue (SYNTH).
    else if (!cur && keep(l)) cur = o.type === 'user' ? [l] : [`>>> ${JSON.stringify({ type: 'user', message: { role: 'user', content: SYNTH[name] || 'Continue.' } })}`, l];
    if (cur && o?.type === 'result') { res.push(cur); cur = null; }
  }
  return res;
}

// En-tete : t13 jusqu'au system/init.
const head = [];
for (const l of read('t13')) {
  if (l.startsWith('>>> ')) continue;
  head.push(l);
  if (parse(l)?.subtype === 'init') break;
}
const session = parse(head.at(-1)).session_id;

// Renumerotation.
const UUID = /\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/g;
const PREF = /\b(toolu|msg|req|srvtoolu)_[0-9A-Za-z]{10,}\b/g;
const AGENT = /("(?:agent_id|agentId|task_id|taskId)":")([0-9a-z]{8,})"/g;
function renumber(line, k, sessions) {
  const tag = k.toString(16).padStart(6, '0');
  return line
    .replace(UUID, (u) => (sessions.has(u) ? session : u.slice(0, 30) + tag))
    .replace(PREF, (m) => m.slice(0, -6) + tag)
    .replace(AGENT, (_, p, id) => `${p}${id.slice(0, -6)}${tag}"`)
    .replace(/(a[0-9a-f]{10})[0-9a-f]{6}(?=[^0-9a-f])/g, (m, p) => p + tag); // id d'agent dans les chemins de sortie
}

// Copies entrelacees : on tire dans les modeles au prorata de ce qui reste.
const pools = MIX.map(([f, t, n]) => {
  const tt = turns(f);
  if (!tt[t]) throw new Error(`${f} n'a pas de tour ${t}`);
  const sessions = new Set(tt[t].map((l) => parse(l)?.session_id).filter(Boolean));
  return { f, t, left: n, total: n, lines: tt[t], sessions };
});
const outLines = [...head];
let k = 0;
for (;;) {
  const live = pools.filter((p) => p.left > 0);
  if (!live.length) break;
  // le modele le plus « en retard » sur sa proportion
  live.sort((a, b) => a.left / a.total - b.left / b.total).reverse();
  const p = live[0];
  p.left--;
  k++;
  // Deltas de streaming (`stream_event`) gardes dans 1 copie sur STREAM_EVERY de chaque modele,
  // pour garder le fichier raisonnable (les `assistant` complets sont toujours la).
  const withStream = (p.total - p.left - 1) % STREAM_EVERY === 0;
  for (const l of p.lines) {
    if (!withStream && parse(l)?.type === 'stream_event') continue;
    outLines.push(renumber(l, k, p.sessions));
  }
}

writeFileSync(out, outLines.join('\n') + '\n', 'utf8');

// Statistiques.
let textBytes = 0;
let toolMain = 0, toolSub = 0, edits = 0, agents = 0, results = 0, users = 0, texts = 0, bad = 0;
const seen = new Set();
for (const l of outLines) {
  const o = parse(l);
  if (!o) { bad++; continue; }
  if (l.startsWith('>>> ')) { users++; continue; }
  if (o.type === 'user' && !o.parent_tool_use_id && !(Array.isArray(o.message?.content) && o.message.content.some((c) => c.type === 'tool_result'))) users++;
  if (o.type === 'result') results++;
  if (o.type === 'user' && Array.isArray(o.message?.content)) {
    for (const c of o.message.content) if (c.type === 'tool_result') textBytes += Buffer.byteLength(typeof c.content === 'string' ? c.content : JSON.stringify(c.content));
  }
  if (o.type === 'assistant') {
    for (const c of o.message.content) {
      if (c.type === 'tool_use') {
        if (seen.has(c.id)) continue; seen.add(c.id);
        if (o.parent_tool_use_id) toolSub++; else toolMain++;
        if (c.name === 'Edit' && !o.parent_tool_use_id) edits++;
        if ((c.name === 'Agent' || c.name === 'Task') && !o.parent_tool_use_id) agents++;
      }
      if (c.type === 'text' && !o.parent_tool_use_id) { texts++; textBytes += Buffer.byteLength(c.text); }
    }
  }
}
const size = Buffer.byteLength(outLines.join('\n'));
console.error(`${out} : ${outLines.length} lignes, ${(size / 1e6).toFixed(2)} Mo ; ${users} messages utilisateur, ${results} result, ${texts} blocs de texte, ${toolMain} appels d'outils (fil principal) + ${toolSub} (sous-agents), ${edits} Edit, ${agents} sous-agents (appels Agent) ; texte affichable (texte + resultats d'outils) ${(textBytes / 1e6).toFixed(2)} Mo ; lignes de rendu estimees (utilisateur + texte + 2 par outil, appel et resultat) : ${users + texts + 2 * toolMain} ; ${bad} ligne(s) non JSON`);
if (bad) process.exit(1);
