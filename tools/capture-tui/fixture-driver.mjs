#!/usr/bin/env node
// Pilote de capture stream-json (fixtures t13, t14).
//
// Usage :
//   node fixture-driver.mjs <scenario> <sortie-brute.jsonl>
//
// Lance claude.exe en `-p --input-format stream-json --output-format stream-json --verbose`
// + les options du scenario, dans le dossier du bac a sable `.claude-work/j1/sandbox/<scenario>`
// (recree a chaque fois, avec les fichiers de `files/`). Envoie les messages du scenario un
// par un (le suivant apres chaque `result`), ferme stdin apres le dernier `result`.
// Format de sortie (identique aux fixtures t1 a t12) : lignes `>>> ` = envois du pilote,
// `STDERR ` = stderr du CLI, `EXIT n` = code de sortie ; le reste = stdout du CLI tel quel.
// La sortie brute n'est PAS anonymisee : passer ensuite par anonymize.mjs.

import { spawn } from 'node:child_process';
import { createWriteStream, mkdirSync, rmSync, copyFileSync } from 'node:fs';
import { createInterface } from 'node:readline';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { homedir } from 'node:os';

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, '..', '..');
const claude = process.env.CLAUDE_EXE || join(homedir(), '.local', 'bin', 'claude.exe');

const BASE = ['-p', '--input-format', 'stream-json', '--output-format', 'stream-json', '--verbose'];

export const SCENARIOS = {
  t13: {
    files: ['config.py'],
    args: ['--include-partial-messages', '--model', 'haiku', '--permission-mode', 'acceptEdits'],
    messages: [
      "Dans config.py, avec un seul appel de l'outil Edit, remplace les deux lignes TIMEOUT = 30 et RETRIES = 3 par les trois lignes TIMEOUT = 60, RETRIES = 5 et BACKOFF = 1.5. Ensuite, avec l'outil Write, cree le fichier notes2.txt contenant deux lignes : alpha et beta. Puis reponds juste ok.",
    ],
  },
  t14: {
    files: [],
    args: ['--include-partial-messages', '--model', 'haiku', '--permission-mode', 'bypassPermissions', '--max-turns', '2'],
    messages: [
      "Fais ces actions l'une apres l'autre, en attendant chaque resultat avant la suivante, sans rien corriger : 1) lis le fichier absent.txt avec l'outil Read ; 2) lance en Bash `ls dossier-absent` ; 3) lance en Bash `exit 3`. Puis reponds fait.",
    ],
  },
};

async function main() {
  const [name, out] = process.argv.slice(2);
  const sc = SCENARIOS[name];
  if (!sc || !out) {
    console.error(`usage: node fixture-driver.mjs <${Object.keys(SCENARIOS).join('|')}> <sortie.jsonl>`);
    process.exit(2);
  }
  const cwd = join(repo, '.claude-work', 'j1', 'sandbox', name);
  rmSync(cwd, { recursive: true, force: true });
  mkdirSync(cwd, { recursive: true });
  for (const f of sc.files) copyFileSync(join(here, 'files', f), join(cwd, f));

  // Environnement propre : si le pilote tourne sous Claude Code, ne pas transmettre ses variables.
  const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC_)/.test(k)));
  const args = [...BASE, ...sc.args];
  const w = createWriteStream(out, { encoding: 'utf8' });
  const child = spawn(claude, args, { cwd, env, stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true });

  let i = 0;
  const send = () => {
    const msg = { type: 'user', message: { role: 'user', content: sc.messages[i++] } };
    const line = JSON.stringify(msg);
    w.write('>>> ' + line + '\n');
    child.stdin.write(line + '\n');
  };

  createInterface({ input: child.stdout }).on('line', (line) => {
    w.write(line + '\n');
    let o;
    try { o = JSON.parse(line); } catch { return; }
    if (o.type === 'result') {
      if (i < sc.messages.length) send();
      else child.stdin.end();
    }
  });
  createInterface({ input: child.stderr }).on('line', (line) => w.write('STDERR ' + line + '\n'));
  child.on('close', (code) => {
    w.write(`EXIT ${code}\n`);
    w.end();
    console.error(`${name}: ${claude} ${args.join(' ')} -> EXIT ${code} (cwd ${cwd})`);
  });
  send();
}

main();
