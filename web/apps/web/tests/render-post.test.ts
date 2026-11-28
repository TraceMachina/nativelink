import { expect, test } from "@jest/globals";
import { codeLanguage, renderPostHtml } from "../lib/render-post";

test("a fenced block gets a wrapper, a language label and a copy button", () => {
  const html = renderPostHtml("```json5\n{ stores: [] }\n```\n");
  expect(html).toContain('<div class="code-block">');
  expect(html).toContain('<span class="code-block-lang">json5</span>');
  expect(html).toContain(
    '<button type="button" class="code-copy" aria-live="polite" aria-label="Copy code to clipboard">Copy</button>',
  );
  expect(html).toContain(
    '<pre><code class="language-json5">{ stores: [] }</code></pre>',
  );
});

test("code content is escaped, so markup inside a block is shown, not rendered", () => {
  const html = renderPostHtml(
    '```\n<script>alert("x")</script> && a < b\n```\n',
  );
  expect(html).toContain(
    "&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt; &amp;&amp; a &lt; b",
  );
  expect(html).not.toContain("<script>");
});

test("a block with no language has no label and a bare code element", () => {
  const html = renderPostHtml("```\nbazel build //...\n```\n");
  expect(html).not.toContain("code-block-lang");
  expect(html).toContain("<pre><code>bazel build //...</code></pre>");
});

test("the language is the first token of the info string, with unsafe characters dropped", () => {
  expect(codeLanguage("yaml title=pipeline.yml")).toBe("yaml");
  expect(codeLanguage(" c++ ")).toBe("c++");
  expect(codeLanguage('x" onclick="y')).toBe("x");
  expect(codeLanguage(undefined)).toBe("");
});

test("prose and raw figures still render the way marked renders them", () => {
  const html = renderPostHtml(
    '**Author**\n\n<figure>\n  <img src="/assets/a.png" alt="a" />\n</figure>\n\nA paragraph with `inline code`.\n',
  );
  expect(html).toContain("<p><strong>Author</strong></p>");
  expect(html).toContain('<img src="/assets/a.png" alt="a" />');
  expect(html).toContain("<code>inline code</code>");
  expect(html).not.toContain("code-block");
});
