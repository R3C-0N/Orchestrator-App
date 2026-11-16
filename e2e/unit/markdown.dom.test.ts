// @vitest-environment jsdom
// Markdown avec un vrai DOM (jsdom) : DOMPurify est actif et nettoie la
// sortie (plan 1.4). On vérifie qu'il tourne vraiment, puis les règles de
// rendu de la capture 02.
import DOMPurify from "dompurify";
import { describe, expect, it } from "vitest";
import { markdownCached, renderMarkdown } from "../../src/lib/render/markdown";

/** Le HTML produit, monté dans un élément : on inspecte le DOM, pas la chaîne. */
function mount(html: string): HTMLElement {
  const el = document.createElement("div");
  el.innerHTML = html;
  return el;
}

describe("DOMPurify actif (jsdom)", () => {
  it("DOMPurify a un DOM et retire lui-même les attributs et balises dangereux", () => {
    expect(DOMPurify.isSupported).toBe(true);
    // Preuve directe que la sanitisation tourne (pas seulement l'échappement du rendu).
    expect(DOMPurify.sanitize('<img src=x onerror="alert(1)">')).toBe('<img src="x">');
    expect(DOMPurify.sanitize("<script>alert(1)</script>ok")).toBe("ok");
    expect(DOMPurify.sanitize('<a href="javascript:alert(1)">x</a>')).toBe("<a>x</a>");
  });

  it("<img onerror> dans le texte : affiché en texte, aucun élément img", () => {
    const el = mount(renderMarkdown('avant <img src=x onerror="alert(1)"> après'));
    expect(el.querySelector("img")).toBeNull();
    expect(el.querySelector("[onerror]")).toBeNull();
    expect(el.textContent).toContain('<img src=x onerror="alert(1)">');
  });

  it("<script> en bloc et en code : jamais d'élément script", () => {
    const el = mount(renderMarkdown("<script>alert(1)</script>\n\n`<script>`\n\n```html\n<script>x()</script>\n```"));
    expect(el.querySelector("script")).toBeNull();
    expect(el.textContent).toContain("<script>alert(1)</script>");
  });

  it("lien javascript: : aucun href, texte seul", () => {
    const el = mount(renderMarkdown("[clic](javascript:alert(1)) et <a href=\"javascript:alert(1)\">x</a>"));
    expect(el.querySelector("a")).toBeNull();
    expect(el.querySelector("[href]")).toBeNull();
    expect(el.querySelector(".md-link")?.textContent).toBe("clic");
  });
});

describe("rendu markdown (02)", () => {
  const el = mount(
    renderMarkdown(
      "## Titre\n\n### Sous-titre\n\n- un **gras** et `code`\n\n1. Premier\n\n```python\ndef f():\n    m = f\"Bonjour, {nom} !\"\n    return 1\n```",
    ),
  );

  it("titres sans #, en bloc de même taille (aucun h1-h6)", () => {
    expect(el.querySelector(".md-h")?.textContent).toBe("Titre");
    expect(el.querySelector("h1,h2,h3,h4,h5,h6")).toBeNull();
    expect(el.textContent).not.toContain("#");
  });

  it("listes avec leur marqueur en texte, code en ligne, bloc de code coloré", () => {
    const marks = [...el.querySelectorAll(".md-mark")].map((m) => m.textContent);
    expect(marks).toEqual(["- ", "1. "]);
    expect(el.querySelector(".md-code")?.textContent).toBe("code");
    expect(el.querySelector("pre.md-pre.hl-ansi .hljs-keyword")?.textContent).toBe("def");
  });

  it("f-string : seul le début jusqu'à la première substitution est une chaîne (02)", () => {
    const strings = [...el.querySelectorAll(".hljs-string")].map((s) => s.textContent);
    expect(strings).toEqual(['f"Bonjour, ']);
    expect(el.querySelector("pre")?.textContent).toContain('m = f"Bonjour, {nom} !"');
  });

  it("CRLF : aucun retour chariot dans le rendu", () => {
    expect(renderMarkdown("ligne 1\r\nligne 2\r\n\r\n```\na\r\nb\r\n```")).not.toContain("\r");
  });

  it("cache : même texte, même chaîne", () => {
    expect(markdownCached("a **b**")).toBe(markdownCached("a **b**"));
  });
});
