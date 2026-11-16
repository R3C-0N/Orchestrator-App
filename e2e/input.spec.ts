/// <reference types="node" />
// Saisie et raccourcis (jalon J7), en mode simulé : une vérification par
// règle de la décision 13 (Ctrl+C, Échap, palette, sélecteur), l'envoi,
// le saut de ligne, l'historique, le changement de modèle, et les seuils
// de la section 1.11 (message visible < 50 ms après Entrée ; état `Idle`
// < 2 s après Échap).
import { expect, test, type Page } from "@playwright/test";

type Call = { cmd: string; args: Record<string, unknown> };
type Orch = {
  store: {
    rows: { kind: string; text?: string }[];
    status: string;
    meta: { model: string | null; models: unknown[] };
    question: unknown;
  };
  mock: { send(text: string, replyChars?: number): void; stopStream(): void; streaming: boolean };
  calls: Call[];
};
/** Fenêtre de l'app en mode simulé (types locaux : `scroll.spec.ts` a les siens). */
type W = Window & { __orch: Orch };

const PROMPT = '[data-testid="prompt"] textarea[aria-label="Message"]';

async function open(page: Page, rows = 6) {
  await page.goto(`/?mock=demo&rows=${rows}&rate=200`);
  await page.waitForFunction(() => (window as unknown as W).__orch?.store.status === "Idle");
  await expect(page.locator(PROMPT)).toBeFocused();
}

const draft = (page: Page) => page.locator(PROMPT).inputValue();
const calls = (page: Page, cmd?: string) =>
  page.evaluate((cmd) => (window as unknown as W).__orch.calls.filter((c) => !cmd || c.cmd === cmd), cmd);
const status = (page: Page) => page.evaluate(() => (window as unknown as W).__orch.store.status);

/** Lance un tour qui ne finit pas seul (flux continu). */
async function startTurn(page: Page) {
  await page.evaluate(() => (window as unknown as W).__orch.mock.send("tour long", Infinity));
  await page.waitForFunction(() => (window as unknown as W).__orch.store.status === "Streaming");
}

function report(name: string, data: unknown) {
  console.log(`MESURE ${name} ${JSON.stringify(data)}`);
}


/** Nombre de fins de tour dans le fil. */
const turnCount = (page: Page) =>
  page.evaluate(() => (window as unknown as W).__orch.store.rows.filter((r: { kind: string }) => r.kind === "turn").length);

/**
 * Fin du tour lancé après `before` fins de tour : une fin de tour de plus,
 * puis `Idle`. Attendre seulement `Idle` juste après Entrée pouvait réussir
 * avant même le début du tour (statut encore au repos) ; Ctrl+C tombait
 * alors pendant le tour et interrompait au lieu d'effacer.
 */
const turnDone = (page: Page, before: number) =>
  page.waitForFunction(
    (before) => {
      const st = (window as unknown as W).__orch.store;
      return st.rows.filter((r: { kind: string }) => r.kind === "turn").length > before && st.status === "Idle";
    },
    before,
    { timeout: 10_000 },
  );

test.describe("rendu de la saisie (01)", () => {
  test("❯ entre deux règles ─, texte d'invite avant le premier message", async ({ page }) => {
    await open(page, 0);
    const p = page.getByTestId("prompt");
    await expect(p.locator(".rule")).toHaveCount(2);
    expect((await p.locator(".rule").first().innerText()).startsWith("────")).toBe(true);
    await expect(p.locator(".caret")).toHaveText("❯");
    await expect(page.locator(PROMPT)).toHaveAttribute("placeholder", 'Try "how do I log an error?"');
    // La ligne d'état suit la saisie.
    const promptBox = await p.boundingBox();
    const statusBox = await page.getByTestId("status-line").boundingBox();
    expect(statusBox!.y).toBeGreaterThanOrEqual(promptBox!.y + promptBox!.height - 1);
  });
});

test.describe("envoi, saut de ligne, historique", () => {
  test("Entrée envoie, Maj+Entrée fait un saut de ligne, collage multiligne gardé", async ({ page, context }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    await open(page);
    await page.keyboard.type("ligne 1");
    await page.keyboard.press("Shift+Enter");
    await page.keyboard.type("ligne 2");
    expect(await draft(page)).toBe("ligne 1\nligne 2");
    expect(await calls(page, "send_message")).toHaveLength(0);
    // Collage (Ctrl+V) : les sauts de ligne restent.
    await page.evaluate(() => navigator.clipboard.writeText("\ncollé A\ncollé B"));
    await page.keyboard.press("Control+v");
    expect(await draft(page)).toBe("ligne 1\nligne 2\ncollé A\ncollé B");
    await page.keyboard.press("Enter");
    expect(await draft(page)).toBe("");
    expect(await calls(page, "send_message")).toEqual([
      { cmd: "send_message", args: { text: "ligne 1\nligne 2\ncollé A\ncollé B" } },
    ]);
    await expect(page.locator('[data-row-kind="user"]').last()).toContainText("collé B");
  });

  test("↑ / ↓ parcourent l'historique et rendent le brouillon", async ({ page }) => {
    await open(page);
    for (const m of ["premier", "second"]) {
      const turns = await turnCount(page);
      await page.keyboard.type(m);
      await page.keyboard.press("Enter");
      await turnDone(page, turns);
    }
    await page.keyboard.type("brouillon");
    await page.keyboard.press("ArrowUp");
    expect(await draft(page)).toBe("second");
    await page.keyboard.press("ArrowUp");
    expect(await draft(page)).toBe("premier");
    await page.keyboard.press("ArrowDown");
    expect(await draft(page)).toBe("second");
    await page.keyboard.press("ArrowDown");
    expect(await draft(page)).toBe("brouillon");
  });
});

test("frappe depuis la conversation : le texte va dans la saisie", async ({ page }) => {
  await open(page);
  await page.locator(".vlist").focus();
  await page.keyboard.type("abc");
  expect(await draft(page)).toBe("abc");
});

test.describe("Ctrl+C (décision 13)", () => {
  test("sélection dans la saisie : copie, rien d'effacé ni d'interrompu", async ({ page, context }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    await open(page);
    await page.keyboard.type("à copier");
    await page.keyboard.press("Control+a");
    await page.keyboard.press("Control+c");
    expect(await draft(page)).toBe("à copier");
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe("à copier");
    expect(await calls(page, "interrupt")).toHaveLength(0);
  });

  test("sélection dans la conversation pendant un tour : copie, pas d'interruption", async ({ page, context }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    await open(page);
    await startTurn(page);
    await page.locator(PROMPT).fill("brouillon");
    const selected = await page.evaluate(() => {
      const row = document.querySelector('[data-row-kind="user"]')!;
      const r = document.createRange();
      r.selectNodeContents(row);
      const s = window.getSelection()!;
      s.removeAllRanges();
      s.addRange(r);
      return s.toString();
    });
    await page.keyboard.press("Control+c");
    // Le presse-papiers garde la mise en page du DOM (espaces de la grille) :
    // on compare le texte du message.
    const words = selected.replace(/^❯\s*/, "").trim().slice(0, 40);
    expect(await page.evaluate(() => navigator.clipboard.readText())).toContain(words);
    expect(await calls(page, "interrupt")).toHaveLength(0);
    expect(await draft(page)).toBe("brouillon");
    await page.evaluate(() => (window as unknown as W).__orch.mock.stopStream());
  });

  test("au repos, saisie non vide : efface (c2)", async ({ page }) => {
    await open(page);
    await page.keyboard.type("texte");
    await page.keyboard.press("Control+c");
    expect(await draft(page)).toBe("");
    expect(await calls(page)).toHaveLength(0);
  });

  test("au repos, saisie vide : rien (c3)", async ({ page }) => {
    await open(page);
    const rows = await page.evaluate(() => (window as unknown as W).__orch.store.rows.length);
    await page.keyboard.press("Control+c");
    expect(await calls(page)).toHaveLength(0);
    expect(await status(page)).toBe("Idle");
    expect(await page.evaluate(() => (window as unknown as W).__orch.store.rows.length)).toBe(rows);
    await expect(page.getByText("Press Ctrl-C again to exit")).toHaveCount(0);
  });

  test("pendant un tour : interrompt et garde la saisie (c4)", async ({ page }) => {
    await open(page);
    await startTurn(page);
    await page.keyboard.type("brouillon");
    const t0 = Date.now();
    await page.keyboard.press("Control+c");
    await page.waitForFunction(() => (window as unknown as W).__orch.store.status === "Idle", null, { timeout: 2000 });
    report("ctrl-c-idle-ms", Date.now() - t0);
    expect(await calls(page, "interrupt")).toEqual([{ cmd: "interrupt", args: { cancel_queued: false } }]);
    expect(await draft(page)).toBe("brouillon");
    await expect(page.locator('[data-row-kind="interrupted"]').last()).toContainText(
      "Interrupted · What should Claude do instead?",
    );
  });
});

test.describe("Échap (décision 13)", () => {
  test("pendant un tour : interrompt, état Idle < 2 s, rendu de 08", async ({ page }) => {
    await open(page);
    const res: number[] = [];
    for (let i = 0; i < 3; i++) {
      await startTurn(page);
      const ms = await page.evaluate(
        () =>
          new Promise<number>((resolve) => {
            const t0 = performance.now();
            window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
            const poll = () =>
              (window as unknown as W).__orch.store.status === "Idle" ? resolve(performance.now() - t0) : requestAnimationFrame(poll);
            poll();
          }),
      );
      res.push(+ms.toFixed(1));
    }
    report("echap-idle-ms", res);
    expect(Math.max(...res)).toBeLessThan(2000);
    expect(await calls(page, "interrupt")).toHaveLength(3);
    await expect(page.locator('[data-row-kind="interrupted"]').last()).toContainText(
      "Interrupted · What should Claude do instead?",
    );
    // Le bloc interrompu ne finit pas en succès : pas de « ✻ … for … » après lui.
    const last = await page.evaluate(() => (window as unknown as W).__orch.store.rows.at(-1));
    expect(last).toMatchObject({ kind: "turn", subtype: "error_during_execution", isError: true });
  });

  test("pendant un tour avec un brouillon : interrompt et garde la saisie", async ({ page }) => {
    await open(page);
    await startTurn(page);
    await page.keyboard.type("garde-moi");
    await page.keyboard.press("Escape");
    await page.waitForFunction(() => (window as unknown as W).__orch.store.status === "Idle", null, { timeout: 2000 });
    expect(await draft(page)).toBe("garde-moi");
  });

  test("saisie non vide, un appui : « Esc again to clear », rien d'effacé (a*)", async ({ page }) => {
    await open(page);
    await page.keyboard.type("essai");
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("prompt-hint")).toHaveText("Esc again to clear");
    expect(await draft(page)).toBe("essai");
    // L'indication s'efface quand le second appui n'effacerait plus rien.
    await expect(page.getByTestId("prompt-hint")).toHaveText("", { timeout: 2000 });
    expect(await draft(page)).toBe("essai");
  });

  test("double Échap rapproché (300 ms) : efface (n300)", async ({ page }) => {
    await open(page);
    await page.keyboard.type("essai 300 ms");
    await page.keyboard.press("Escape");
    await page.waitForTimeout(300);
    await page.keyboard.press("Escape");
    expect(await draft(page)).toBe("");
    await expect(page.getByTestId("prompt-hint")).toHaveText("");
  });

  test("double Échap espacé (950 ms) : garde (n800)", async ({ page }) => {
    await open(page);
    await page.keyboard.type("essai 950 ms");
    await page.keyboard.press("Escape");
    await page.waitForTimeout(950);
    await page.keyboard.press("Escape");
    expect(await draft(page)).toBe("essai 950 ms");
    await expect(page.getByTestId("prompt-hint")).toHaveText("Esc again to clear");
  });

  test("double Échap sur saisie vide : rien (Rewind hors prototype, v1)", async ({ page }) => {
    await open(page);
    const rows = await page.evaluate(() => (window as unknown as W).__orch.store.rows.length);
    await page.keyboard.press("Escape");
    await page.waitForTimeout(200);
    await page.keyboard.press("Escape");
    expect(await calls(page)).toHaveLength(0);
    await expect(page.getByTestId("prompt-hint")).toHaveText("");
    expect(await page.evaluate(() => (window as unknown as W).__orch.store.rows.length)).toBe(rows);
  });

  test("Échap maintenu (répétition) : la saisie reste", async ({ page }) => {
    await open(page);
    await page.keyboard.type("maintenu");
    await page.evaluate(() => {
      const ta = document.querySelector("textarea")!;
      for (let i = 0; i < 10; i++)
        ta.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", repeat: i > 0, bubbles: true }));
    });
    expect(await draft(page)).toBe("maintenu");
    await expect(page.getByTestId("prompt-hint")).toHaveText("Esc again to clear");
  });

  test("effacement par Ctrl+C pendant l'historique : ↓ ne rend pas l'ancien brouillon", async ({ page }) => {
    await open(page);
    const turns = await turnCount(page);
    await page.keyboard.type("envoyé");
    await page.keyboard.press("Enter");
    await turnDone(page, turns);
    await page.keyboard.type("brouillon");
    await page.keyboard.press("ArrowUp");
    expect(await draft(page)).toBe("envoyé");
    await page.keyboard.press("Control+c");
    expect(await draft(page)).toBe("");
    await page.keyboard.press("ArrowDown");
    expect(await draft(page)).toBe("");
  });

  test("sélecteur ouvert pendant un tour : Échap le ferme sans interrompre", async ({ page }) => {
    await open(page);
    await startTurn(page);
    await page.keyboard.type("x");
    await page.keyboard.press("Alt+p");
    await expect(page.getByTestId("model-picker")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("model-picker")).toHaveCount(0);
    expect(await calls(page, "interrupt")).toHaveLength(0);
    expect(await draft(page)).toBe("x");
    expect(await status(page)).toBe("Streaming");
    await page.evaluate(() => (window as unknown as W).__orch.mock.stopStream());
  });

  test("carte de question : Échap n'interrompt pas (ignorable ou non, J8)", async ({ page }) => {
    await open(page);
    await startTurn(page);
    for (const canSkip of [false, true]) {
      await page.evaluate(
        (canSkip) =>
          ((window as unknown as W).__orch.store.question = { requestId: "r1", toolUseId: null, questions: [], canSkip }),
        canSkip,
      );
      await page.keyboard.press("Escape");
      expect(await calls(page, "interrupt")).toHaveLength(0);
      expect(await page.evaluate(() => (window as unknown as W).__orch.store.question !== null)).toBe(true);
    }
    await page.evaluate(() => ((window as unknown as W).__orch.store.question = null));
    await page.evaluate(() => (window as unknown as W).__orch.mock.stopStream());
  });
});

test.describe("palette (point d'accroche J10)", () => {
  test("Ctrl+K et `/` en début de saisie : rien d'envoyé ni d'effacé", async ({ page }) => {
    await open(page);
    await page.keyboard.press("Control+k");
    await page.keyboard.type("/");
    expect(await draft(page)).toBe("/");
    expect(await calls(page)).toHaveLength(0);
  });
});

test.describe("changement de modèle", () => {
  test("Alt+P, ↓, Entrée : set_model, puis le tour suivant l'utilise (init.model)", async ({ page }) => {
    await open(page);
    await page.keyboard.press("Alt+p");
    const picker = page.getByTestId("model-picker");
    await expect(picker).toBeVisible();
    await expect(picker.locator('[aria-selected="true"]')).toContainText("Default (recommended) ✔");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    await expect(picker).toHaveCount(0);
    expect(await calls(page, "set_model")).toEqual([{ cmd: "set_model", args: { model: "sonnet" } }]);
    await expect(page.getByTestId("prompt-hint")).toHaveText("Set model to Sonnet 5.5");
    await page.keyboard.type("bonjour");
    await page.keyboard.press("Enter");
    await page.waitForFunction(() => (window as unknown as W).__orch.store.status !== "Idle");
    expect(await page.evaluate(() => (window as unknown as W).__orch.store.meta.model)).toBe("claude-sonnet-5-5");
    await page.evaluate(() => (window as unknown as W).__orch.mock.stopStream());
  });

  test("`/model haiku` change le modèle ; `/model` seul ouvre le sélecteur", async ({ page }) => {
    await open(page);
    await page.keyboard.type("/model haiku");
    await page.keyboard.press("Enter");
    await expect.poll(() => calls(page, "set_model")).toEqual([{ cmd: "set_model", args: { model: "haiku" } }]);
    expect(await calls(page, "send_message")).toHaveLength(0);
    expect(await page.evaluate(() => (window as unknown as W).__orch.store.meta.model)).toBe("claude-haiku-5-5");
    await page.keyboard.type("/model");
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("model-picker")).toBeVisible();
    await expect(page.getByTestId("model-picker").locator('[aria-selected="true"]')).toContainText("Haiku 5.5 ✔");
  });
});

test("`/model` avec une valeur invalide : indication, pas de set_model", async ({ page }) => {
  await open(page);
  for (const bad of ["--dangerous", "-x"]) {
    await page.locator(PROMPT).fill(`/model ${bad}`);
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("prompt-hint")).toHaveText(`Invalid model: ${bad}`);
  }
  expect(await calls(page)).toHaveLength(0);
  // Un identifiant complet non annoncé reste accepté.
  await page.locator(PROMPT).fill("/model claude-opus-4-8");
  await page.keyboard.press("Enter");
  await expect.poll(() => calls(page, "set_model")).toEqual([{ cmd: "set_model", args: { model: "claude-opus-4-8" } }]);
});

test("latence : message visible < 50 ms après Entrée (1.11)", async ({ page }) => {
  await open(page, 60);
  const res: number[] = [];
  for (let i = 0; i < 5; i++) {
    const text = `message de mesure ${i}`;
    await page.locator(PROMPT).fill(text);
    await page.evaluate((text) => {
      const w = window as unknown as { __t0: number; __vis: Promise<number> };
      w.__t0 = 0;
      document.querySelector("textarea")!.addEventListener(
        "keydown",
        (e) => {
          if ((e as KeyboardEvent).key === "Enter") w.__t0 = performance.now();
        },
        { once: true, capture: true },
      );
      w.__vis = new Promise<number>((resolve) => {
        const found = () =>
          [...document.querySelectorAll<HTMLElement>('[data-row-kind="user"]')].some((r) =>
            r.textContent?.includes(text),
          );
        const mo = new MutationObserver(() => {
          if (!found()) return;
          mo.disconnect();
          const ch = new MessageChannel();
          ch.port1.onmessage = () => resolve(performance.now() - w.__t0);
          ch.port2.postMessage(0);
        });
        mo.observe(document.querySelector(".vlist")!, { childList: true, subtree: true, characterData: true });
      });
    }, text);
    await page.keyboard.press("Enter");
    res.push(+(await page.evaluate(() => (window as unknown as { __vis: Promise<number> }).__vis)).toFixed(1));
    await page.evaluate(() => (window as unknown as W).__orch.mock.stopStream());
    await page.waitForTimeout(200);
  }
  report("ajout-message-textarea", res);
  expect(Math.max(...res)).toBeLessThan(50);
});

test("latence : frappe peinte < 25 ms en moyenne, < 50 ms au pire, pendant un flux", async ({ page }) => {
  await open(page, 60);
  await startTurn(page);
  const res = await page.evaluate(async () => {
    const ta = document.querySelector("textarea")!;
    const out: number[] = [];
    for (let i = 0; i < 30; i++) {
      const ms = await new Promise<number>((resolve) => {
        const t0 = performance.now();
        ta.addEventListener(
          "input",
          () => {
            requestAnimationFrame(() => {
              const ch = new MessageChannel();
              ch.port1.onmessage = () => resolve(performance.now() - t0);
              ch.port2.postMessage(0);
            });
          },
          { once: true },
        );
        ta.setRangeText("a", ta.value.length, ta.value.length, "end");
        ta.dispatchEvent(new InputEvent("input", { bubbles: true, data: "a", inputType: "insertText" }));
      });
      out.push(+ms.toFixed(1));
      await new Promise((r) => setTimeout(r, 30));
    }
    return out;
  });
  await page.evaluate(() => (window as unknown as W).__orch.mock.stopStream());
  const avg = res.reduce((a, b) => a + b, 0) / res.length;
  report("frappe-peinte-ms", { avg: +avg.toFixed(1), max: Math.max(...res) });
  expect(avg).toBeLessThan(25);
  expect(Math.max(...res)).toBeLessThan(50);
});
