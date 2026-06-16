// Transform a schemars JSON Schema (draft 2020-12 / draft-07) into an MDX
// configuration-reference page.
//
// This is the plain-Node replacement for the legacy `metaphase_aot.ts`
// transform that the old site ran on the Bazel `nativelink-config:docs_json`
// output. The schema is produced by the `build-schema` binary
// (`cargo run --bin build-schema --features dev-schema --package nativelink-config`
// or `bazel run //nativelink-config:build-schema`), which derives a JSON Schema
// — including the `///` doc-comments as `description` fields — straight from the
// Rust config structs. Because the schema is generated from the same types the
// binary deserializes, the published reference can never drift from what the
// binary accepts.
//
// The module has no dependencies and no side effects: `schemaToMdx(schema, opts)`
// is a pure function from schema JSON to an MDX string.

/** @typedef {Record<string, any>} JsonSchema */

// --------------------------------------------------------------------------
// MDX-safe markdown sanitisation.
//
// Rust doc-comments are CommonMark, but MDX additionally treats `<` as the
// start of JSX and `{` as the start of a JS expression, so raw doc text such as
// `Vec<String>`, `<https://example.com>` or `{ "k": 1 }` outside a code fence
// would fail to compile. We preserve fenced and inline code verbatim (MDX keeps
// those literal) and only rewrite the plain-text segments.
// --------------------------------------------------------------------------

/** Escape the two characters MDX treats specially in plain text. */
function escapeMdxText(text) {
  return text.replace(/</g, "&lt;").replace(/\{/g, "&#123;").replace(/\}/g, "&#125;");
}

function autolinkText(url) {
  if (url === "https://docs.rs/hyper/latest/hyper/server/conn/struct.Http.html") {
    return "hyper HTTP server docs";
  }
  return url;
}

function normalizeProseText(text) {
  return text
    .replace(/\bhashmap\b/g, "hash map")
    .replace(/\bie:/g, "i.e.,")
    .replace(/\bbootup\b/g, "startup")
    .replace(/\bredis stores\b/g, "Redis stores")
    .replace(/\bredis store\b/g, "Redis store")
    .replace(/\bredis servers\b/g, "Redis servers")
    .replace(/\bredis server\b/g, "Redis server")
    .replace(/\bserver\(s\)/g, "servers")
    .replace(/\bByteStream\.Write\b/g, "`ByteStream.Write`")
    .replace(
      /\bgrpc\(s\):\/\/example\.com:443\b/g,
      "grpc://example.com:443 or grpcs://example.com:443",
    )
    .replace(/\bhttp\/2\b/g, "HTTP/2")
    .replace(/\bhttp\/1\.1\b/g, "HTTP/1.1")
    .replace(/\bHttp\/2\b/g, "HTTP/2")
    .replace(/\bAdvanced Http server configuration\b/g, "Advanced HTTP server configuration")
    .replace(
      /\bAdvanced Http configurations\. These are generally should not be set\./g,
      "Advanced HTTP configuration. These generally should not be set.",
    )
    .replace(/\bTls Configuration\b/g, "TLS configuration")
    .replace(/\bhyper's default values\b/g, "the default values from hyper")
    .replace(
      /\bFor documentation on what each of these do, see the hyper documentation:/g,
      "For documentation on these settings, see the hyper documentation:",
    )
    .replace(/\bThis configuration will be used to to restrict\b/g, "This configuration will be used to restrict")
    .replace(/\bA scheduler that simply forwards\b/g, "A scheduler that forwards")
    .replace(/\bThe the value\b/g, "The value")
    .replace(/\bset to -1 to disable\b/g, "set to `-1` to disable")
    .replace(/([.!?]) {2,}(?=[A-Z`])/g, "$1 ");
}

/** Sanitise a run of text that contains no code fences. Inline code spans are
 *  left untouched; CommonMark autolinks become real markdown links; everything
 *  else is escaped. */
function sanitizeProse(text) {
  // Preserve inline code spans (`...`) — MDX keeps them literal.
  return text
    .split(/(`[^`]*`)/g)
    .map((segment) => {
      if (segment.startsWith("`") && segment.endsWith("`")) return segment;
      segment = normalizeProseText(segment);
      // CommonMark autolink `<https://…>` / `<mailto:…>` → markdown link.
      const linked = segment.replace(
        /<((?:https?|mailto):[^>\s]+)>/g,
        (_, url) => `[${autolinkText(url)}](${url})`,
      );
      return escapeMdxText(linked);
    })
    .join("");
}

/** Sanitise a full multi-paragraph description, preserving fenced code blocks
 *  (e.g. the `**Example JSON Config:**` blocks) exactly as written. */
export function sanitizeMarkdown(md) {
  if (!md) return "";
  const fence = /(^|\n)([ \t]*)(```|~~~)[^\n]*\n[\s\S]*?\n\2\3[ \t]*(?=\n|$)/g;
  let out = "";
  let last = 0;
  let m = fence.exec(md);
  while (m !== null) {
    out += sanitizeProse(md.slice(last, m.index));
    out += m[0]; // fenced block, verbatim
    last = m.index + m[0].length;
    m = fence.exec(md);
  }
  out += sanitizeProse(md.slice(last));
  return out;
}

/** First paragraph of a description, flattened to a single safe table cell. */
function cellText(md) {
  if (!md) return "";
  // Stop at the first blank line or code fence — keep cells short.
  let head = md.split(/\n\s*\n/)[0].split(/\n[ \t]*(?:```|~~~)/)[0];
  head = head.replace(/\s*\n\s*/g, " ").trim();
  return sanitizeProse(head).replace(/\|/g, "\\|");
}

// --------------------------------------------------------------------------
// Slugs / anchors. Heading anchors are produced by rehype-slug (github-slugger
// semantics): lower-case, drop anything that is not alphanumeric/space/hyphen,
// collapse spaces to hyphens. Config type names are alphanumeric, so this is
// effectively `toLowerCase()`, but we slug generally to stay correct.
// --------------------------------------------------------------------------

function slug(name) {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9 -]/g, "")
    .trim()
    .replace(/\s+/g, "-");
}

// --------------------------------------------------------------------------
// Type rendering. Produces an MDX-safe string (uses `&lt;`/`&gt;`, escapes
// table pipes) that links to a type's heading anchor when it is a known `$def`.
// --------------------------------------------------------------------------

const refName = (ref) => ref.split("/").pop();

function typeLink(name, known) {
  return known.has(name) ? `[${name}](#${slug(name)})` : `\`${name}\``;
}

/** @param {JsonSchema} s @param {Set<string>} known */
function typeToString(s, known) {
  if (!s || typeof s !== "object") return "any";
  if (s.$ref) return typeLink(refName(s.$ref), known);
  if (Array.isArray(s.allOf) && s.allOf.length === 1) return typeToString(s.allOf[0], known);

  const union = s.anyOf || s.oneOf;
  if (union) {
    const nonNull = union.filter((x) => x && x.type !== "null");
    const nullable = nonNull.length !== union.length;
    const inner = nonNull.map((x) => typeToString(x, known)).join(" \\| ");
    return nullable && nonNull.length === 1 ? `${inner}?` : inner || "null";
  }

  const type = s.type;
  if (Array.isArray(type)) {
    const nn = type.filter((t) => t !== "null");
    const nullable = nn.length !== type.length;
    const base = typeToString({ ...s, type: nn.length === 1 ? nn[0] : nn }, known);
    return nullable ? `${base}?` : base;
  }

  if (type === "array") {
    return `${s.items ? typeToString(s.items, known) : "any"}[]`;
  }
  if (type === "object") {
    if (s.additionalProperties && typeof s.additionalProperties === "object") {
      return `map&lt;string, ${typeToString(s.additionalProperties, known)}&gt;`;
    }
    return "object";
  }
  if (typeof type === "string") {
    if (s.const !== undefined) return `\`${JSON.stringify(s.const)}\``;
    return s.format ? `${type} (${s.format})` : type;
  }
  if (Array.isArray(s.enum)) {
    return s.enum.map((v) => `\`${JSON.stringify(v)}\``).join(" \\| ");
  }
  if (s.const !== undefined) return `\`${JSON.stringify(s.const)}\``;
  return "any";
}

function defaultCell(p) {
  if (!p || !("default" in p)) return "—";
  return `\`${JSON.stringify(p.default).replace(/\|/g, "\\|")}\``;
}

// --------------------------------------------------------------------------
// Def classification + rendering.
// --------------------------------------------------------------------------

function classify(def) {
  if (def.oneOf) {
    if (def.oneOf.every((b) => b && b.const !== undefined)) return "string-enum";
    const tagged = def.oneOf.every(
      (b) =>
        b &&
        b.type === "object" &&
        Array.isArray(b.required) &&
        b.required.length === 1 &&
        b.properties,
    );
    if (tagged) return "tagged-enum";
    return "union";
  }
  if (Array.isArray(def.enum)) return "string-enum";
  if (def.type === "object" || def.properties) return "object";
  return "alias";
}

function renderPropsTable(def, known) {
  const props = def.properties || {};
  const names = Object.keys(props);
  if (names.length === 0) return "_No fields._\n";
  const required = new Set(def.required || []);
  const rows = names.map((name) => {
    const p = props[name];
    const req = required.has(name) ? "Yes" : "—";
    return `| \`${name}\` | ${typeToString(p, known)} | ${req} | ${defaultCell(p)} | ${cellText(p.description)} |`;
  });
  return [
    "| Field | Type | Required | Default | Description |",
    "| --- | --- | --- | --- | --- |",
    ...rows,
    "",
  ].join("\n");
}

function renderObject(name, def, known) {
  const out = [`## ${name}`, ""];
  if (def.description) out.push(sanitizeMarkdown(def.description), "");
  out.push(renderPropsTable(def, known));
  return out.join("\n");
}

function renderTaggedEnum(name, def, known) {
  const out = [`## ${name}`, ""];
  if (def.description) out.push(sanitizeMarkdown(def.description), "");
  // Some types (e.g. NamedConfig) carry common fields alongside a flattened
  // tagged spec — `{ name, #[serde(flatten)] spec }`. Render those first.
  if (def.properties && Object.keys(def.properties).length > 0) {
    out.push("**Common fields**", "", renderPropsTable(def, known));
  }
  out.push("Plus exactly one of the following variants (the key selects the variant):", "");
  for (const branch of def.oneOf) {
    const key = branch.required[0];
    const payload = branch.properties[key];
    out.push(`### \`${key}\``, "");
    if (branch.description) out.push(sanitizeMarkdown(branch.description), "");
    out.push(`**Type:** ${typeToString(payload, known)}`, "");
  }
  return out.join("\n");
}

function renderStringEnum(name, def) {
  const out = [`## ${name}`, ""];
  if (def.description) out.push(sanitizeMarkdown(def.description), "");
  const variants = def.oneOf
    ? def.oneOf.map((b) => ({ value: b.const, description: b.description }))
    : def.enum.map((v) => ({ value: v, description: "" }));
  out.push(
    "| Value | Description |",
    "| --- | --- |",
    ...variants.map((v) => `| \`${JSON.stringify(v.value)}\` | ${cellText(v.description)} |`),
    "",
  );
  return out.join("\n");
}

function renderAlias(name, def, known) {
  const out = [`## ${name}`, ""];
  if (def.description) out.push(sanitizeMarkdown(def.description), "");
  out.push(`**Type:** ${typeToString(def, known)}`, "");
  return out.join("\n");
}

function renderUnion(name, def, known) {
  const out = [`## ${name}`, ""];
  if (def.description) out.push(sanitizeMarkdown(def.description), "");
  out.push("One of:", "");
  for (const branch of def.oneOf) {
    const desc = branch.description ? ` — ${cellText(branch.description)}` : "";
    out.push(`- ${typeToString(branch, known)}${desc}`);
  }
  out.push("");
  return out.join("\n");
}

function renderDef(name, def, known) {
  switch (classify(def)) {
    case "object":
      return renderObject(name, def, known);
    case "tagged-enum":
      return renderTaggedEnum(name, def, known);
    case "string-enum":
      return renderStringEnum(name, def);
    case "union":
      return renderUnion(name, def, known);
    default:
      return renderAlias(name, def, known);
  }
}

// --------------------------------------------------------------------------
// Ordering: breadth-first from the root so related types cluster, then any
// unreferenced types alphabetically.
// --------------------------------------------------------------------------

function collectRefs(node, acc) {
  if (!node || typeof node !== "object") return;
  if (Array.isArray(node)) {
    for (const x of node) collectRefs(x, acc);
    return;
  }
  if (typeof node.$ref === "string") acc.push(refName(node.$ref));
  for (const k of Object.keys(node)) {
    if (k === "$ref") continue;
    collectRefs(node[k], acc);
  }
}

function orderDefs(root, defs) {
  const order = [];
  const seen = new Set();
  const queue = [];
  collectRefs({ properties: root.properties }, queue);
  while (queue.length) {
    const name = queue.shift();
    if (seen.has(name) || !defs[name]) continue;
    seen.add(name);
    order.push(name);
    const childRefs = [];
    collectRefs(defs[name], childRefs);
    for (const r of childRefs) if (!seen.has(r)) queue.push(r);
  }
  for (const n of Object.keys(defs).sort()) {
    if (!seen.has(n)) {
      seen.add(n);
      order.push(n);
    }
  }
  return order;
}

// --------------------------------------------------------------------------
// Top-level (root) rendering.
// --------------------------------------------------------------------------

function renderRoot(root, known) {
  const props = root.properties || {};
  const required = new Set(root.required || []);
  const rows = Object.keys(props).map((name) => {
    const p = props[name];
    const req = required.has(name) ? "Yes" : "—";
    return `| \`${name}\` | ${typeToString(p, known)} | ${req} | ${cellText(p.description)} |`;
  });
  return [
    "## Top-level fields",
    "",
    `The root object (\`${root.title || "CasConfig"}\`) accepts the following fields:`,
    "",
    "| Field | Type | Required | Description |",
    "| --- | --- | --- | --- |",
    ...rows,
    "",
  ].join("\n");
}

// --------------------------------------------------------------------------
// Public entry point.
// --------------------------------------------------------------------------

/**
 * @param {JsonSchema} schema  Parsed schemars JSON Schema.
 * @param {object} opts
 * @param {string} opts.version       Display version, e.g. "v1.5.0" or "main".
 * @param {string} opts.ref           Git ref the schema was built from.
 * @param {string} [opts.commit]      Short commit the schema was built from.
 * @param {boolean} [opts.isLatest]   Whether this is the canonical/latest page.
 * @param {string} [opts.switcher]    JSX string for the version switcher.
 * @param {string} [opts.githubBase]  e.g. "https://github.com/TraceMachina/nativelink".
 * @returns {string} MDX document.
 */
export function schemaToMdx(schema, opts) {
  const {
    version,
    ref,
    commit,
    switcher = "",
    githubBase = "https://github.com/TraceMachina/nativelink",
  } = opts;
  const defs = schema.$defs || schema.definitions || {};
  const known = new Set(Object.keys(defs));
  const order = orderDefs(schema, defs);
  const srcBase = `${githubBase}/tree/${ref}/nativelink-config/src`;

  const frontmatter = [
    "---",
    "title: Configuration reference",
    "description: Every knob in the NativeLink JSON5 configuration — types, defaults, and links to source, autogenerated from the Rust config crate.",
    "full: true",
    "---",
    "",
  ].join("\n");

  const provenance = [
    "{/* AUTOGENERATED — do not edit by hand.",
    `   Source: nativelink-config @ ${ref}${commit ? ` (${commit})` : ""}`,
    "   Regenerate: bun run --filter=@nativelink/docs gen:config-reference */}",
    "",
  ].join("\n");

  const intro = [
    `This is the canonical NativeLink configuration reference for **${version}**.`,
    "It is autogenerated from the Rust config crate",
    `([\`nativelink-config/src\`](${srcBase})) via the \`build-schema\` binary, so`,
    "it can never drift from what the binary actually deserializes.",
    "",
  ].join("\n");

  const body = [
    frontmatter,
    provenance,
    switcher ? `${switcher}\n` : "",
    intro,
    renderRoot(schema, known),
    "## Configuration types",
    "",
    "Every type reachable from the root configuration, in reading order.",
    "",
    ...order.map((name) => renderDef(name, defs[name], known)),
    "## Reading the source",
    "",
    "If anything here disagrees with the binary, the source wins:",
    "",
    `- [\`stores.rs\`](${srcBase}/stores.rs)`,
    `- [\`cas_server.rs\`](${srcBase}/cas_server.rs)`,
    `- [\`schedulers.rs\`](${srcBase}/schedulers.rs)`,
    "",
  ].join("\n");

  // Collapse any accidental run of >2 blank lines.
  return `${body.replace(/\n{3,}/g, "\n\n").trimEnd()}\n`;
}
