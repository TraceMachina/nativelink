import { Marked } from "marked";

// Blog bodies are markdown rendered to HTML at build time. The only
// customisation is fenced code: each block is wrapped so the page can show
// the language and a copy button, and so readers can paste a config straight
// out of a post. Everything else uses marked's defaults.

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

// The info string after the opening fence is "lang" or "lang extra words";
// only the first token names the language. Anything but letters, digits,
// "+", "-" and "#" is dropped so the value is safe in a class and in text.
export function codeLanguage(info: string | undefined): string {
  const first = (info ?? "").trim().split(/\s+/)[0] ?? "";
  return first.replace(/[^A-Za-z0-9+#-]/g, "");
}

const marked = new Marked({
  renderer: {
    code({ text, lang }) {
      const language = codeLanguage(lang);
      const label = language
        ? `<span class="code-block-lang">${language}</span>`
        : "";
      const classAttr = language ? ` class="language-${language}"` : "";
      const button =
        '<button type="button" class="code-copy" aria-live="polite" aria-label="Copy code to clipboard">Copy</button>';
      const bar = `<div class="code-block-bar">${label}${button}</div>`;
      const code = `<pre><code${classAttr}>${escapeHtml(text)}</code></pre>`;
      return `<div class="code-block">${bar}${code}</div>\n`;
    },
  },
});

export function renderPostHtml(body: string): string {
  return marked.parse(body, { async: false }) as string;
}
