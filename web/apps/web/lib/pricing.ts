/** The self-serve licence portal. */
export const ENTERPRISE_URL = "https://enterprise.nativelink.com";

/** A table cell that links somewhere, such as the Enterprise hosting entry. */
export interface ComparisonLink {
  text: string;
  href: string;
}

export type ComparisonValue = string | boolean | ComparisonLink;

/** The tiers the comparison columns are keyed by, in display order. */
export type TierKey = "oss" | "ent";

export interface ComparisonRow {
  label: string;
  oss: ComparisonValue;
  ent: ComparisonValue;
}

export interface ComparisonGroup {
  section: string;
  rows: ComparisonRow[];
}

/** The feature-for-feature comparison on /pricing, Open Source vs Enterprise. */
export const comparison: ComparisonGroup[] = [
  {
    section: "Platform",
    rows: [
      {
        label: "Hosting",
        oss: "Self-hosted",
        ent: { text: "On-prem or managed", href: ENTERPRISE_URL },
      },
      { label: "Distributed scheduler", oss: true, ent: true },
      { label: "Remote caching", oss: true, ent: true },
      { label: "Remote execution", oss: true, ent: true },
      { label: "Cross-compilation", oss: true, ent: true },
      { label: "External storage (S3, Redis)", oss: true, ent: true },
      { label: "Autoscaling", oss: false, ent: true },
      { label: "Multi-region", oss: "DIY", ent: true },
    ],
  },
  {
    section: "Compatibility",
    rows: [
      { label: "Supported build systems", oss: "All", ent: "All + custom" },
      {
        label: "Operating systems",
        oss: "Linux, macOS",
        ent: "Linux, macOS, Windows",
      },
      { label: "Org-wide sharing", oss: true, ent: true },
    ],
  },
  {
    section: "Operations",
    rows: [
      { label: "GUI dashboard", oss: false, ent: true },
      { label: "Build action breakdown", oss: false, ent: true },
      { label: "Live build updates", oss: false, ent: true },
      { label: "Audit logs / SSO", oss: false, ent: true },
    ],
  },
  {
    section: "Support",
    rows: [
      {
        label: "Channel",
        oss: "Community Slack",
        ent: "Dedicated engineer",
      },
      { label: "Onboarding", oss: false, ent: "White-glove" },
    ],
  },
];

/** A row every tier includes outright says nothing about which tier to pick. */
export function isUniversal(row: ComparisonRow): boolean {
  return row.oss === true && row.ent === true;
}

/**
 * Split the comparison into the features every tier includes and the rows
 * that actually differ between tiers, dropping any section left empty.
 *
 * The table renders the first list on one line and the second as rows, so
 * identical checkmarks no longer dilute the rows that carry the decision.
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
