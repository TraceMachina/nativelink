// Transform a scanned NativeLink metric inventory into an MDX reference page.
//
// This is the metrics-side sibling of `schema-to-mdx.mjs`. Where that module
// turns a schemars JSON Schema into the configuration reference, this one turns
// the inventory produced by `scripts/gen-metrics-reference.mjs`, read straight
// out of `nativelink-util/src/metrics.rs` and the call sites across the
// workspace, into the metrics reference.
//
// The reason this exists rather than a hand-written catalogue: a metric that is
// *declared* is not a metric that is *emitted*. NativeLink declares fourteen
// OpenTelemetry instruments; several of them have no call site anywhere in the
// binary, and the shipped Prometheus rules reference two of those by name. A
// hand-written catalogue cannot notice that. A generated one notices it every
// time it runs.
//
// The module has no dependencies and no side effects: `metricsToMdx(inventory,
// opts)` is a pure function from inventory JSON to an MDX string.

/** @typedef {Record<string, any>} Inventory */

// --------------------------------------------------------------------------
// MDX-safe text. Rust doc-comments are CommonMark, but MDX also treats `<` as
// the start of JSX and `{` as the start of a JS expression.
// --------------------------------------------------------------------------

function escapeMdxText(text) {
  return text.replace(/</g, "&lt;").replace(/\{/g, "&#123;").replace(/\}/g, "&#125;");
}

function escapeTableCell(text) {
  return escapeMdxText(text).replace(/\|/g, "&#124;");
}

/** Flatten a Rust doc-comment to a single safe table cell. */
function cell(md) {
  if (!md) return "—";
  const head = md
    .split(/\n\s*\n/)[0]
    .replace(/\s*\n\s*/g, " ")
    .trim();
  return escapeTableCell(head) || "—";
}

function prose(md) {
  if (!md) return "";
  return md
    .split(/(`[^`]*`)/g)
    .map((segment) =>
      segment.startsWith("`") && segment.endsWith("`") ? segment : escapeMdxText(segment),
    )
    .join("");
}

function slug(name) {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9 -]/g, "-")
    .trim()
    .replace(/\s+/g, "-")
    .replace(/-+/g, "-");
}

// --------------------------------------------------------------------------
// Human labels for the mechanical verdicts the scanner produces.
// --------------------------------------------------------------------------

const VERDICT_LABEL = {
  emitted: "Yes",
  gated: "Conditionally",
  never: "**Never**",
};

function sourceLink(githubBase, ref, file, line) {
  const anchor = line ? `#L${line}` : "";
  const label = line ? `${file}:${line}` : file;
  return `[\`${label}\`](${githubBase}/blob/${ref}/${file}${anchor})`;
}

function siteList(sites, githubBase, ref) {
  if (!sites.length) return "—";
  return sites.map((s) => sourceLink(githubBase, ref, s.file, s.line)).join(", ");
}

// --------------------------------------------------------------------------
// Sections.
// --------------------------------------------------------------------------

function renderSummaryTable(inventory, githubBase, ref) {
  const rows = [];
  for (const group of inventory.groups) {
    for (const m of group.metrics) {
      const prom = m.promName
        ? `\`${m.promName}\``
        : `\`${m.promNameGuess}\` (unreferenced)`;
      rows.push(
        `| [\`${m.otelName}\`](#${slug(m.otelName)}) | ${prom} | ${m.kind} | ${
          VERDICT_LABEL[m.emit.verdict]
        } | ${escapeTableCell(m.emit.gate)} |`,
      );
    }
  }
  return [
    "| Instrument | Prometheus series | Kind | Emitted | Under what conditions |",
    "| --- | --- | --- | --- | --- |",
    ...rows,
    "",
  ].join("\n");
}

function renderNeverEmitted(inventory, githubBase, ref) {
  const dead = [];
  for (const group of inventory.groups) {
    for (const m of group.metrics) {
      if (m.emit.verdict === "never") dead.push({ group, m });
    }
  }
  if (dead.length === 0) {
    return [
      "## Declared but never emitted",
      "",
      "Every instrument declared in `metrics.rs` has at least one call site.",
      "",
    ].join("\n");
  }

  const out = [
    "## Declared but never emitted",
    "",
    `${dead.length === 1 ? "One instrument is" : `${dead.length} instruments are`} built by the`,
    "meter and stored on the metrics struct, but no code anywhere in the workspace",
    "ever calls `.add()` or `.record()` on them. They will never appear in your",
    "Prometheus output, at any configuration, on any release this page was",
    "generated from.",
    "",
  ];

  const rows = dead.map(({ m }) => {
    const refs = m.artifacts.length
      ? m.artifacts.map((a) => sourceLink(githubBase, ref, a.file, a.line)).join(", ")
      : "—";
    return `| \`${m.otelName}\` | \`${m.promName ?? m.promNameGuess}\` | ${refs} |`;
  });
  out.push(
    "| Instrument | Series it would produce | Referenced by shipped config |",
    "| --- | --- | --- |",
    ...rows,
    "",
  );

  const withRefs = dead.filter((d) => d.m.artifacts.length);
  if (withRefs.length) {
    out.push(
      `<Callout type="warn" title="The shipped observability examples query ${
        withRefs.length === 1 ? "a series that does not exist" : "series that do not exist"
      }">`,
      "",
      `  ${withRefs
        .map((d) => `\`${d.m.promName ?? d.m.promNameGuess}\``)
        .join(" and ")} appear in the recording rules and the metrics`,
      "  README under `deployment-examples/metrics/`, but nothing emits them. Any",
      "  recording rule, dashboard panel or alert built on them evaluates to an",
      "  empty vector forever, which reads on a dashboard as a healthy zero, not",
      "  as a missing signal. Delete those panels or treat them as known-empty.",
      "",
      "</Callout>",
      "",
    );
  }
  return out.join("\n");
}

function renderOrphans(inventory, githubBase, ref) {
  const orphans = inventory.orphanArtifactNames ?? [];
  if (orphans.length === 0) return "";
  const rows = orphans.map(
    (o) => `| \`${o.name}\` | ${siteList(o.sites, githubBase, ref)} |`,
  );
  return [
    "## Series referenced by shipped config with no matching instrument",
    "",
    "These `nativelink_`-prefixed series appear in the shipped Prometheus rules,",
    "dashboards or alerts, but do not correspond to any instrument declared in",
    "`metrics.rs`. Either they are stale names from an older release, or they are",
    "typos. Either way they resolve to nothing.",
    "",
    "| Series | Referenced from |",
    "| --- | --- |",
    ...rows,
    "",
  ].join("\n");
}

function renderMetric(m, githubBase, ref) {
  const out = [`### \`${m.otelName}\``, ""];
  if (m.description) out.push(prose(m.description), "");

  const facts = [
    `| Instrument type | \`${m.rustType}\` |`,
    `| Field | \`${m.field}\` |`,
    `| Unit | ${m.unit ? `\`${m.unit}\`` : "—"} |`,
    `| Prometheus series | ${m.promName ? `\`${m.promName}\`` : `\`${m.promNameGuess}\` (not referenced by any shipped rule or dashboard)`} |`,
    `| Emitted | ${VERDICT_LABEL[m.emit.verdict]}. ${escapeTableCell(m.emit.gate)} |`,
    `| Declared at | ${sourceLink(githubBase, ref, m.declaredAt.file, m.declaredAt.line)} |`,
    `| Emitted from | ${siteList(m.emit.sites, githubBase, ref)} |`,
  ];
  out.push("| Property | Value |", "| --- | --- |", ...facts, "");

  if (m.boundaries?.length) {
    out.push(
      `**Histogram buckets:** ${m.boundaries.length} explicit boundaries, from`,
      `\`${m.boundaries[0]}\` to \`${m.boundaries[m.boundaries.length - 1]}\`${
        m.unit ? ` \`${m.unit}\`` : ""
      }. Anything past the last boundary lands in the`,
      "`+Inf` bucket, so a quantile computed above it is a lower bound, not a",
      "measurement.",
      "",
    );
  }
  return out.join("\n");
}

function renderGroup(group, githubBase, ref) {
  const out = [`## ${group.title}`, ""];
  if (group.doc) out.push(prose(group.doc), "");
  out.push(
    `Declared as \`${group.name}\` in ${sourceLink(
      githubBase,
      ref,
      group.declaredAt.file,
      group.declaredAt.line,
    )}, backed by the \`${group.struct}\` struct.`,
    "",
  );
  if (group.gate) out.push(group.gate, "");
  for (const m of group.metrics) out.push(renderMetric(m, githubBase, ref));
  return out.join("\n");
}

function renderAttributes(inventory, githubBase, ref) {
  const out = ["## Attributes", ""];
  out.push(
    "Every attribute key NativeLink attaches to a metric, and the closed set of",
    "values it can take. The Prometheus label is the key with dots replaced by",
    "underscores.",
    "",
  );
  for (const group of inventory.attributeGroups) {
    out.push(`### ${group.title}`, "");
    const rows = group.keys.map((k) => {
      const values = k.values.length
        ? k.values.map((v) => `\`${v.value}\``).join(", ")
        : "open (free-form string)";
      return `| \`${k.key}\` | \`${k.key.replace(/\./g, "_")}\` | \`${k.const}\` | ${escapeTableCell(values)} |`;
    });
    out.push(
      "| Attribute | Prometheus label | Rust constant | Values |",
      "| --- | --- | --- | --- |",
      ...rows,
      "",
    );
  }
  const enums = inventory.valueEnums.filter((e) => e.variants.some((v) => v.doc));
  for (const e of enums) {
    out.push(`### \`${e.attribute ?? e.name}\` values`, "");
    out.push(
      "| Value | Meaning |",
      "| --- | --- |",
      ...e.variants.map((v) => `| \`${v.value}\` | ${cell(v.doc)} |`),
      "",
    );
  }
  return out.join("\n");
}

// --------------------------------------------------------------------------
// Public entry point.
// --------------------------------------------------------------------------

/**
 * @param {Inventory} inventory  Produced by scripts/gen-metrics-reference.mjs.
 * @param {object} opts
 * @param {string} opts.version      Display version, e.g. "v1.6.3".
 * @param {string} opts.ref          Git ref the sources were read from.
 * @param {string} [opts.commit]     Short commit the sources were read from.
 * @param {string} [opts.githubBase] e.g. "https://github.com/TraceMachina/nativelink".
 * @returns {string} MDX document.
 */
export function metricsToMdx(inventory, opts) {
  const {
    version,
    ref,
    commit,
    githubBase = "https://github.com/TraceMachina/nativelink",
  } = opts;

  const total = inventory.groups.reduce((n, g) => n + g.metrics.length, 0);
  const never = inventory.groups.reduce(
    (n, g) => n + g.metrics.filter((m) => m.emit.verdict === "never").length,
    0,
  );

  const frontmatter = [
    "---",
    "title: Metrics reference",
    "description: Every OpenTelemetry instrument NativeLink declares, with its type, unit, attributes, Prometheus series, and whether the binary actually emits it. Autogenerated from the Rust source and its call sites.",
    "full: true",
    "---",
    "",
  ].join("\n");

  const provenance = [
    "{/* AUTOGENERATED. Do not edit by hand.",
    `   Source: nativelink-util/src/metrics.rs @ ${ref}${commit ? ` (${commit})` : ""}`,
    "   Regenerate from web/: bun --filter @nativelink/docs gen:metrics-reference */}",
    "",
  ].join("\n");

  const intro = [
    `NativeLink **${version}** declares ${total} OpenTelemetry instruments. This page is`,
    "generated from the declarations in",
    `[\`nativelink-util/src/metrics.rs\`](${githubBase}/blob/${ref}/nativelink-util/src/metrics.rs)`,
    "and from every `.add()` and `.record()` call site across the workspace, so the",
    "**Emitted** column is not a claim, it is the result of looking.",
    "",
    "That column is the reason this page is generated rather than written. Declaring",
    "an instrument costs nothing and emits nothing; the instrument only produces a",
    "series when some code path calls it. A catalogue written by hand cannot tell",
    "those two states apart, and the gap between them is where dashboards go quietly",
    "wrong.",
    "",
  ].join("\n");

  const headline = never
    ? [
        `<Callout type="warn" title="${never} of the ${total} instruments are never emitted">`,
        "",
        "  They are constructed at startup and stored on the metrics struct, but no",
        "  code path calls them. See [Declared but never",
        "  emitted](#declared-but-never-emitted) for which ones, and for the shipped",
        "  Prometheus rules that query them anyway.",
        "",
        "</Callout>",
        "",
      ].join("\n")
    : "";

  const naming = [
    "## How an instrument becomes a Prometheus series",
    "",
    "NativeLink speaks OTLP only; there is no Prometheus scrape endpoint in the",
    "binary. The names in the middle column above are what the shipped OpenTelemetry",
    "collector produces after two transformations:",
    "",
    "1. The Prometheus exporter in",
    `   [\`otel-collector-config.yaml\`](${githubBase}/blob/${ref}/deployment-examples/metrics/otel-collector-config.yaml)`,
    "   is configured with `namespace: nativelink`, which prefixes every series with",
    "   `nativelink_`.",
    "2. The exporter rewrites the OTLP name into Prometheus form: dots become",
    "   underscores, and monotonic counters gain a `_total` suffix. Histograms fan out",
    "   into `_bucket`, `_sum` and `_count` series.",
    "",
    "`cache.operations` (a counter) is therefore scraped as `nativelink_cache_operations_total`,",
    "and `cache.operation.duration` (a histogram) as `nativelink_cache_operation_duration_bucket`",
    "and friends. Change the collector's `namespace` and every name on this page changes",
    "with it.",
    "",
    "Where a name in the **Prometheus series** column is unmarked, it is not a",
    "prediction: it is the name that actually appears in the shipped rules,",
    "dashboards and alerts under `deployment-examples/metrics/` and `kubernetes/`.",
    "A name marked _(unreferenced)_ is derived from the two rules above, because no",
    "shipped artifact mentions that metric at all: nothing has ever queried it, so",
    "nothing has confirmed the exporter's spelling of it.",
    "",
  ].join("\n");

  const body = [
    frontmatter,
    provenance,
    intro,
    headline,
    "## Every instrument at a glance",
    "",
    renderSummaryTable(inventory, githubBase, ref),
    naming,
    renderNeverEmitted(inventory, githubBase, ref),
    renderOrphans(inventory, githubBase, ref),
    ...inventory.groups.map((g) => renderGroup(g, githubBase, ref)),
    renderAttributes(inventory, githubBase, ref),
    "## Reading the source",
    "",
    "If anything here disagrees with the binary, the source wins:",
    "",
    `- [\`nativelink-util/src/metrics.rs\`](${githubBase}/blob/${ref}/nativelink-util/src/metrics.rs): every declaration`,
    `- [\`nativelink-util/src/telemetry.rs\`](${githubBase}/blob/${ref}/nativelink-util/src/telemetry.rs): the OTLP exporter and its interval`,
    `- [\`nativelink-store/src/cache_metrics_store.rs\`](${githubBase}/blob/${ref}/nativelink-store/src/cache_metrics_store.rs): every cache call site`,
    `- [\`nativelink-scheduler/src/simple_scheduler_state_manager.rs\`](${githubBase}/blob/${ref}/nativelink-scheduler/src/simple_scheduler_state_manager.rs): every execution call site`,
    "",
    "## Where to go next",
    "",
    "<NextStep href=\"/operate/observability\" title=\"Wire up the telemetry pipeline\">",
    "  The collector, the exporter interval and the queries worth alerting on: the",
    "  operational half of this reference.",
    "</NextStep>",
    "",
    "<NextStep href=\"/reference/nativelink-config\" title=\"Configuration reference\" kind=\"aside\">",
    "  Every configuration field, generated the same way from the same source of",
    "  truth.",
    "</NextStep>",
    "",
  ].join("\n");

  return `${body.replace(/\n{3,}/g, "\n\n").trimEnd()}\n`;
}
