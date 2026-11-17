// Empreinte du contrat interface <-> cœur (J14, spec décision 18).
// Même calcul que `src-tauri/build.rs` (`contract_fingerprint`) : FNV-1a
// 64 bits, fichiers triés par chemin relatif, pour chacun « chemin, 0,
// contenu sans `\r`, 0 ». Une interface dont l'empreinte diffère de celle de
// l'exe n'est jamais activée.
//
// Usage : `node tools/contract-fingerprint.mjs` affiche l'empreinte ;
// `import { contractFingerprint } from "./tools/contract-fingerprint.mjs"`.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

/** Garder identique à `CONTRACT_FILES` de `src-tauri/build.rs`. */
export const CONTRACT_FILES = ["src-tauri/build.rs", "src-tauri/capabilities/default.json", "src/lib/core/bindings"];

const OFFSET = 0xcbf29ce484222325n;
const PRIME = 0x100000001b3n;
const MASK = 0xffffffffffffffffn;

function fnv1a(h, bytes) {
  for (const b of bytes) {
    h ^= BigInt(b);
    h = (h * PRIME) & MASK;
  }
  return h;
}

/** Empreinte (16 chiffres hexadécimaux) du contrat du dépôt `root`. */
export function contractFingerprint(root = resolve(dirname(fileURLToPath(import.meta.url)), "..")) {
  const files = [];
  for (const rel of CONTRACT_FILES) {
    const p = join(root, rel);
    let dir = false;
    try {
      dir = statSync(p).isDirectory();
    } catch {
      dir = false;
    }
    if (dir) {
      for (const name of readdirSync(p)) if (name.endsWith(".ts")) files.push([`${rel}/${name}`, join(p, name)]);
    } else files.push([rel, p]);
  }
  files.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
  const enc = new TextEncoder();
  let h = OFFSET;
  for (const [rel, path] of files) {
    let content;
    try {
      content = readFileSync(path).filter((b) => b !== 13);
    } catch {
      content = new Uint8Array();
    }
    h = fnv1a(h, enc.encode(rel));
    h = fnv1a(h, [0]);
    h = fnv1a(h, content);
    h = fnv1a(h, [0]);
  }
  return h.toString(16).padStart(16, "0");
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  console.log(contractFingerprint());
}
