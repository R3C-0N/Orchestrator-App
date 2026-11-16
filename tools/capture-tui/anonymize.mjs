#!/usr/bin/env node
// Anonymise une capture stream-json brute (sortie de fixture-driver.mjs).
//
// Usage :
//   node anonymize.mjs <entree.jsonl> <sortie.jsonl> [nom-utilisateur]
//
// Regles (les memes que pour t1 a t12) :
//   - nom d'utilisateur Windows (defaut : %USERNAME%) -> `<user>`, sous toutes ses formes
//     (chemins `\\`, `/`, `\\\\`, chemins encodes `C--Users-<user>-...`), sans tenir compte de la casse ;
//   - dossier temporaire de session `...\Temp\claude\<id>\` -> `<session>` ;
//   - champs `signature` non vides (blocs thinking, signature_delta) -> `REDACTED` ;
//   - `email`, `organization`, `subscriptionType` -> `REDACTED` ;
//   - fragments `input_json_delta` d'un meme bloc : si une donnee a masquer est coupee entre
//     deux fragments, le JSON recolle et anonymise est mis dans le premier fragment et les
//     suivants sont vides (le JSON reconstitue reste identique).
// Verifie a la fin que chaque ligne (hors prefixes `>>> `, `STDERR `, `EXIT `) passe JSON.parse
// et qu'aucune trace du nom d'utilisateur ne reste.

import { readFileSync, writeFileSync } from 'node:fs';

const [inp, out, userArg] = process.argv.slice(2);
if (!inp || !out) {
  console.error('usage: node anonymize.mjs <entree.jsonl> <sortie.jsonl> [nom-utilisateur]');
  process.exit(2);
}
const user = userArg || process.env.USERNAME || process.env.USER;
if (!user) throw new Error("nom d'utilisateur inconnu");
const esc = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
const userRe = new RegExp(esc(user), 'gi');

function scrub(text) {
  return text
    .replace(/(Temp[\\/]+claude[\\/]+)[0-9a-f]{8,}(?=[\\/])/gi, '$1<session>')
    .replace(/(Temp-claude-)[0-9a-f]{8,}(?=-)/gi, '$1<session>')
    .replace(userRe, '<user>');
}

function scrubLine(line) {
  return scrub(line)
    .replace(/"signature":"[^"]+"/g, '"signature":"REDACTED"')
    .replace(/"(email|organization|subscriptionType)":"[^"]*"/g, '"$1":"REDACTED"');
}

const lines = readFileSync(inp, 'utf8').split(/\r?\n/);
if (lines.at(-1) === '') lines.pop();

// Prefixes du pilote.
const split = (l) => {
  for (const p of ['>>> ', 'STDERR ', 'EXIT ']) if (l.startsWith(p)) return [p, l.slice(p.length)];
  return ['', l];
};

// 1) Regroupe les fragments input_json_delta par (message, index de bloc).
const parsed = lines.map((l) => {
  const [p, body] = split(l);
  if (p) return null;
  try { return JSON.parse(body); } catch { return null; }
});
const groups = new Map();
let msgKey = 0;
parsed.forEach((o, i) => {
  if (!o || o.type !== 'stream_event') return;
  const ev = o.event;
  if (ev.type === 'message_start') msgKey++;
  if (ev.type === 'content_block_delta' && ev.delta?.type === 'input_json_delta') {
    const k = `${o.parent_tool_use_id || ''}|${msgKey}|${ev.index}`;
    if (!groups.has(k)) groups.set(k, []);
    groups.get(k).push(i);
  }
});
const rewritten = new Map();
let glued = 0;
for (const idx of groups.values()) {
  const frags = idx.map((i) => parsed[i].event.delta.partial_json);
  const whole = frags.join('');
  const a = scrub(whole);
  if (a === frags.map(scrub).join('')) continue;
  glued++;
  idx.forEach((i, n) => {
    const o = structuredClone(parsed[i]);
    o.event.delta.partial_json = n === 0 ? a : '';
    rewritten.set(i, JSON.stringify(o));
  });
}

// 2) Remplacements ligne a ligne.
const outLines = lines.map((l, i) => scrubLine(rewritten.has(i) ? rewritten.get(i) : l));

// 3) Verifications.
let bad = 0;
outLines.forEach((l, i) => {
  const [p, body] = split(l);
  if (p === 'EXIT ' || p === 'STDERR ') return;
  try { JSON.parse(body); } catch { bad++; console.error(`ligne ${i + 1} : JSON invalide`); }
});
const userTest = new RegExp(esc(user), 'i');
const left = outLines.filter((l) => userTest.test(l)).length;
writeFileSync(out, outLines.join('\n') + '\n', 'utf8');
console.error(`${out} : ${outLines.length} lignes, ${glued} groupe(s) input_json_delta recolle(s), ${bad} ligne(s) JSON invalide(s), ${left} trace(s) du nom d'utilisateur`);
if (bad || left) process.exit(1);
