/** The self-serve licence portal, which carries the Enterprise prices. nativelink.com does not. */
export const ENTERPRISE_URL = "https://enterprise.nativelink.com";

/** A table cell that links somewhere, such as the Enterprise hosting entry. */
export interface ComparisonLink {
  text: string;
  href: string;
}

export type ComparisonValue = string | boolean | ComparisonLink;

/** The tiers the comparison columns are keyed by, in display order. */
export type TierKey = "oss" | "t1" | "t2";
export const TIER_KEYS: readonly TierKey[] = ["oss", "t1", "t2"];

export interface ComparisonRow {
  label: string;
  oss: ComparisonValue;
  t1: ComparisonValue;
  t2: ComparisonValue;
}

export interface ComparisonGroup {
  section: string;
  rows: ComparisonRow[];
}

const inYourInfrastructure: ComparisonLink = { text: "Your infrastructure", href: ENTERPRISE_URL };

/**
 * The feature-for-feature comparison on /pricing: Open Source and the two
 * Enterprise tiers, mirroring the licence portal's own comparison so the two
 * pages, one click apart, say the same thing.
 */
export const comparison: ComparisonGroup[] = [
  {
    section: "Platform",
    rows: [
      { label: "Hosting", oss: "Self-hosted", t1: inYourInfrastructure, t2: inYourInfrastructure },
      { label: "Unlimited cores, machines and users", oss: true, t1: true, t2: true },
      { label: "Distributed scheduler", oss: true, t1: true, t2: true },
      { label: "Remote caching", oss: true, t1: true, t2: true },
      { label: "Remote execution", oss: true, t1: true, t2: true },
      { label: "Cross-compilation", oss: true, t1: true, t2: true },
      { label: "Deterministic, verifiable builds", oss: true, t1: true, t2: true },
      { label: "External storage (S3, GCS, Redis)", oss: true, t1: true, t2: true },
      {
        label: "Autoscaling",
        oss: false,
        t1: "Reactive, scale from zero",
        t2: "Adaptive, predictive, cost-aware",
      },
      { label: "Multi-region", oss: "DIY", t1: true, t2: true },
    ],
  },
  {
    section: "Compatibility",
    rows: [
      { label: "All supported build systems", oss: true, t1: true, t2: true },
      {
        label: "Operating systems",
        oss: "Linux, macOS",
        t1: "Linux, macOS, Windows",
        t2: "Linux, macOS, Windows",
      },
    ],
  },
  {
    section: "Operations",
    rows: [
      { label: "Admin UI and build dashboards", oss: false, t1: true, t2: true },
      { label: "MCP server for coding agents", oss: false, t1: true, t2: true },
      { label: "Signed, gated Helm charts", oss: false, t1: true, t2: true },
      { label: "Compliance-grade audit logs", oss: false, t1: true, t2: true },
      { label: "SSO / SAML", oss: false, t1: true, t2: true },
      { label: "SCIM provisioning", oss: false, t1: false, t2: true },
      { label: "Per-team access control", oss: false, t1: false, t2: true },
      {
        label: "Telemetry connectors (Datadog, Databricks, Splunk, Snowflake)",
        oss: false,
        t1: false,
        t2: true,
      },
      { label: "Subsidiaries and entities covered", oss: false, t1: false, t2: true },
    ],
  },
  {
    section: "Support",
    rows: [
      {
        label: "Live engineer support",
        oss: "Community Slack",
        t1: "6 hrs / month",
        t2: "12 hrs / month",
      },
      { label: "Response time", oss: false, t1: "24 hours", t2: "4 hours" },
    ],
  },
  {
    section: "Licence",
    rows: [
      {
        label: "Terms",
        oss: { text: "FSL-1.1-Apache-2.0", href: "/license" },
        t1: "Commercial, org-wide",
        t2: "Commercial, org-wide",
      },
    ],
  },
];

/** A row every tier includes outright says nothing about which tier to pick. */
export function isUniversal(row: ComparisonRow): boolean {
  return TIER_KEYS.every((key) => row[key] === true);
}

/** Whether a row shows the same thing in every column, which would make it noise. */
export function isIdenticalAcrossTiers(row: ComparisonRow): boolean {
  const first = JSON.stringify(row[TIER_KEYS[0] as TierKey]);
  return TIER_KEYS.every((key) => JSON.stringify(row[key]) === first);
}

/**
 * Split the comparison into the features every tier includes and the rows
 * that actually differ between tiers, dropping any section left empty.
 */
export function collapseUniversalRows(groups: ComparisonGroup[]): {
  universal: string[];
  differentiators: ComparisonGroup[];
} {
  const universal: string[] = [];
  const differentiators: ComparisonGroup[] = [];
  for (const group of groups) {
    const rows = group.rows.filter((row) => {
      if (isUniversal(row)) {
        universal.push(row.label);
        return false;
      }
      return true;
    });
    if (rows.length > 0) {
      differentiators.push({ section: group.section, rows });
    }
  }
  return { universal, differentiators };
}
