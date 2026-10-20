import { expect, test } from "bun:test";
import katex from "katex";
import sharp from "sharp";
import { SourceMapConsumer } from "source-map-js";

test("KaTeX ignores inherited trust while rendering ordinary math", () => {
  // GHSA-238p-pmpm-9mq7: the options prototype must not enable trusted HTML.
  const options = Object.create({ trust: true });
  const html = katex.renderToString("\\href{javascript:alert(1)}{link}", options);
  expect(html).not.toContain('href="javascript:');
  expect(katex.renderToString("x^2 + y^2")).toContain('class="katex"');
});

test("indexed source maps reject excessive section offsets before expansion", () => {
  // GHSA-68fv-2mgg-jv7q: reject the offset before serializing a huge map.
  const map = {
    version: 3,
    sections: [
      {
        offset: { line: Number.MAX_SAFE_INTEGER, column: 0 },
        map: { version: 3, sources: ["input.js"], names: [], mappings: "AAAA" },
      },
    ],
  };
  expect(() => new SourceMapConsumer(map)).toThrow();

  map.sections[0].offset.line = 0;
  const consumer = new SourceMapConsumer(map);
  expect(consumer.sources).toEqual(["input.js"]);
});

test("patched Sharp renders SVG through its bundled librsvg", async () => {
  const svg = Buffer.from(
    '<svg xmlns="http://www.w3.org/2000/svg" width="8" height="4"><rect width="8" height="4" fill="red"/></svg>',
  );
  const { info } = await sharp(svg).png().toBuffer({ resolveWithObject: true });
  expect(info.format).toBe("png");
  expect(info.width).toBe(8);
  expect(info.height).toBe(4);
});
