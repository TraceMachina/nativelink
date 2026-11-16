import { TrialRequestForm } from "@/components/trial-request-form";
import { BUILD_SYSTEMS_SENTENCE } from "@/lib/copy";
import { type Cta, GET_STARTED, TALK_TO_US } from "@/lib/cta";
import {
  type ComparisonValue,
  ENTERPRISE_URL,
  type TierKey,
  collapseUniversalRows,
  comparison,
} from "@/lib/pricing";
import { TRIAL_WEEKS } from "@/lib/trial";
import { Button, Eyebrow, Reveal, Section, cn } from "@nativelink/ui";
import { Fragment } from "react";

export const metadata = {
  title: "Pricing",
  description:
    "Self-host NativeLink for free with no limits, or license the full platform for your whole company: unlimited engineers and builds, no per-seat or usage fees, running entirely in your infrastructure. Pricing is quoted by email: contact@tracemachina.com.",
};

const PRICING_EMAIL = "contact@tracemachina.com";
const PRICING_MAILTO = `mailto:${PRICING_EMAIL}?subject=${encodeURIComponent("NativeLink pricing")}`;

interface Tier {
  /** Which comparison column this tier owns. */
  key: TierKey;
  name: string;
  price: string;
  cadence: string;
  /** Who the tier is for, in one line. */
  audience: string;
  features: string[];
  cta: Cta;
  /** Enterprise tiers also offer the trial form and a link to the portal, where the price is. */
  enterprise?: { portalLabel: string };
}

const tiers: Tier[] = [
  {
    key: "oss",
    name: "Open Source",
    price: "Free",
    cadence: "forever",
    audience: "Everything needed to run NativeLink yourself, with no limits.",
    features: [
      "Unlimited cores, machines and users",
      "Distributed scheduler, remote cache and remote execution",
      `Every supported build system: ${BUILD_SYSTEMS_SENTENCE}`,
      "AWS, GCP, Azure or your own hardware",
      "Community support on Slack",
    ],
    cta: GET_STARTED,
  },
  {
    key: "t1",
    name: "Enterprise · Tier 1",
    price: "Custom",
    cadence: "annual licence",
    audience: "For private companies under $100M valuation.",
    features: [
      "Runs entirely in your infrastructure",
      "One licence, your whole company",
      "Admin UI and build dashboards",
      "MCP server for Claude Code, Cursor and Codex",
      "Autoscaling: reactive, scale from zero",
      "Signed, gated Helm charts",
      "Compliance-grade audit logs",
      "Live engineer support: 6 hours a month, 24-hour response",
    ],
    cta: TALK_TO_US,
    enterprise: { portalLabel: "Tier 1 on the Enterprise portal" },
  },
  {
    key: "t2",
    name: "Enterprise · Tier 2",
    price: "Custom",
    cadence: "annual licence",
    audience: "For public companies, or private companies at $100M+ valuation.",
    features: [
      "Everything in Tier 1",
      "Adaptive, predictive and cost-aware autoscaling",
      "SCIM provisioning and per-team access control",
      "Telemetry connectors: Datadog, Databricks, Splunk, Snowflake",
      "Subsidiaries and entities covered",
      "Live engineer support: 12 hours a month, 4-hour response",
    ],
    cta: TALK_TO_US,
    enterprise: { portalLabel: "Tier 2 on the Enterprise portal" },
  },
];

const { universal, differentiators } = collapseUniversalRows(comparison);

function Cell({ value }: { value: ComparisonValue }) {
  if (typeof value === "object") {
    const external = value.href.startsWith("http");
    return (
      <a
        href={value.href}
        target={external ? "_blank" : undefined}
        rel={external ? "noreferrer" : undefined}
        className="font-mono text-xs text-brand underline-offset-4 hover:underline sm:text-sm"
      >
        {value.text}
      </a>
    );
  }
  if (typeof value === "boolean") {
    return (
      <span
        aria-label={value ? "Included" : "Not included"}
        className={cn(
          "inline-flex h-5 w-5 items-center justify-center rounded-full font-mono text-[11px]",
          value
            ? "bg-brand-soft text-brand"
            : "bg-foreground/[0.04] text-muted",
        )}
      >
        {value ? "✓" : "—"}
      </span>
    );
  }
  return (
    <span className="font-mono text-xs text-foreground sm:text-sm">
      {value}
    </span>
  );
}

export default function PricingPage() {
  return (
    <>
      {/* HERO */}
      <section className="relative overflow-hidden">
        <div className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[500px] bg-[radial-gradient(ellipse_900px_500px_at_50%_-10%,rgb(var(--nl-color-brand)/0.15),transparent_70%)]" />
        <Section width="default" className="pt-24 pb-10 md:pt-32">
          <Reveal>
            <div className="mx-auto max-w-[860px] text-center">
              <Eyebrow className="mb-5">Pricing</Eyebrow>
              <h1 className="text-balance text-[44px] font-semibold leading-[1.02] tracking-[-0.04em] md:text-6xl">
                Free to run.{" "}
                <span className="bg-gradient-to-r from-brand to-brand-strong bg-clip-text text-transparent">
                  Licensed for your whole company.
                </span>
              </h1>
              <p className="mx-auto mt-6 max-w-[700px] text-[17px] leading-relaxed text-muted-foreground md:text-lg">
                The source-available release runs anywhere, with no limit on
                cores, machines or users. Enterprise adds the full platform
                under one licence: unlimited engineers and builds, no per-seat
                or usage fees, running entirely in your infrastructure. Your
                code and data stay with you.
              </p>
              <p className="mx-auto mt-4 max-w-[640px] text-[15px] leading-relaxed text-muted-foreground">
                Enterprise is priced by company size and quoted by the engineers
                who build NativeLink. Email{" "}
                <a
                  href={PRICING_MAILTO}
                  className="font-mono text-brand underline-offset-4 hover:underline"
                >
                  {PRICING_EMAIL}
                </a>
                , or{" "}
                <a
                  href="#trial"
                  className="text-brand underline-offset-4 hover:underline"
                >
                  request a {TRIAL_WEEKS}-week trial
                </a>
                .
              </p>
            </div>
          </Reveal>
        </Section>
      </section>

      {/* TIERS */}
      <Section width="default" className="pb-10">
        <div className="mx-auto grid max-w-[1100px] gap-5 md:grid-cols-3">
          {tiers.map((tier, i) => (
            <Reveal key={tier.key} delay={i * 0.06}>
              <div
                className={cn(
                  "relative flex h-full flex-col rounded-2xl border p-8",
                  tier.enterprise
                    ? "border-brand/30 bg-brand-soft/20"
                    : "border-border bg-surface",
                )}
              >
                <h2 className="font-mono text-xs uppercase tracking-[0.18em] text-muted">
                  {tier.name}
                </h2>
                <div className="mt-5 flex items-baseline gap-2">
                  <span className="text-[40px] font-semibold leading-none tracking-tight text-foreground">
                    {tier.price}
                  </span>
                  <span className="font-mono text-sm text-muted">
                    {tier.cadence}
                  </span>
                </div>
                <p className="mt-3 text-[15px] leading-relaxed text-foreground">
                  {tier.audience}
                </p>
                <ul className="mt-6 flex flex-col gap-3 text-sm">
                  {tier.features.map((f) => (
                    <li key={f} className="flex items-start gap-3">
                      <span
                        aria-hidden="true"
                        className="mt-0.5 inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-full bg-brand-soft text-[10px] text-brand"
                      >
                        ✓
                      </span>
                      <span className="text-foreground">{f}</span>
                    </li>
                  ))}
                </ul>
                <div className="mt-auto flex flex-col gap-3 pt-8">
                  <Button
                    asChild
                    size="lg"
                    variant="outline"
                    className="w-full"
                  >
                    <a
                      href={tier.cta.href}
                      target={tier.cta.external ? "_blank" : undefined}
                      rel={tier.cta.external ? "noreferrer" : undefined}
                    >
                      {tier.cta.label}
                    </a>
                  </Button>
                  {tier.enterprise ? (
                    <>
                      <Button asChild size="lg" className="w-full">
                        <a href="#trial">Request a {TRIAL_WEEKS}-week trial</a>
                      </Button>
                      <p className="text-center text-sm text-muted-foreground">
                        Price and checkout:{" "}
                        <a
                          href={ENTERPRISE_URL}
                          target="_blank"
                          rel="noreferrer"
                          className="inline-flex min-h-11 items-center text-brand underline-offset-4 hover:underline"
                        >
                          {tier.enterprise.portalLabel}
                        </a>
                      </p>
                    </>
                  ) : null}
                </div>
              </div>
            </Reveal>
          ))}
        </div>
        <Reveal delay={0.1}>
          <p className="mx-auto mt-8 max-w-[760px] text-center text-sm leading-relaxed text-muted-foreground">
            Most of the repository is FSL-1.1-Apache-2.0. Metrics and remote
            persistent workers are Business Source Licensed, so sharing one
            deployment across a team in production needs an Enterprise licence.{" "}
            <a
              href="/license"
              className="text-brand underline-offset-4 hover:underline"
            >
              Read the licence
            </a>
            .
          </p>
        </Reveal>
      </Section>

      {/* COMPARISON */}
      <Section width="default" className="py-24">
        <Reveal>
          <div className="mx-auto mb-12 max-w-[680px] text-center">
            <Eyebrow className="mb-4">Compare</Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
              Feature-for-feature comparison
            </h2>
            <p className="mx-auto mt-5 max-w-[560px] text-[15px] leading-relaxed text-muted-foreground">
              What every tier includes sits on one line at the top, so every row
              below marks a difference between the tiers.
            </p>
          </div>
        </Reveal>

        <Reveal>
          {/* From md up, overflow-clip rather than overflow-hidden: hidden would turn this box into
              the sticky header's scroll container and pin it to the box instead of the viewport.
              Below md four columns cannot fit a phone, so the box scrolls sideways instead; there
              the header is not sticky (a top offset inside a scroll box would open a blank band
              above it) and the feature column is pinned to the left edge instead. */}
          <p className="mx-auto mb-3 max-w-[1100px] font-mono text-[11px] uppercase tracking-[0.14em] text-muted md:hidden">
            Swipe to compare the tiers →
          </p>
          <div className="mx-auto max-w-[1100px] overflow-x-auto rounded-2xl border border-border bg-surface md:overflow-clip">
            <table className="w-full min-w-[640px] border-separate border-spacing-0 text-left md:min-w-0">
              <thead>
                <tr>
                  <th
                    scope="col"
                    className="sticky left-0 z-20 w-[34%] border-b border-border bg-surface-elevated px-3 py-4 text-left align-bottom font-mono text-[10px] uppercase tracking-widest text-muted sm:px-6 md:top-16"
                  >
                    Feature
                  </th>
                  {tiers.map((tier) => (
                    <th
                      key={tier.key}
                      scope="col"
                      className="z-10 border-b border-border bg-surface-elevated px-3 py-4 text-left align-bottom sm:px-6 md:sticky md:top-16"
                    >
                      <span className="block font-mono text-[10px] uppercase tracking-widest text-muted">
                        {tier.name}
                      </span>
                      <span className="mt-1.5 flex flex-wrap items-baseline gap-x-1.5 gap-y-0.5">
                        <span className="text-lg font-semibold leading-none tracking-tight text-foreground">
                          {tier.price}
                        </span>
                        <span className="font-mono text-xs text-muted">
                          {tier.cadence}
                        </span>
                      </span>
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {universal.length > 0 ? (
                  <tr className="bg-brand-soft/30">
                    <td className="sticky left-0 z-10 border-b border-border bg-surface px-3 py-4 align-top sm:px-6 md:static md:bg-transparent">
                      <span className="block text-[15px] font-medium text-foreground">
                        Core platform, included in every tier
                      </span>
                      <span className="mt-1 block text-sm leading-relaxed text-muted-foreground">
                        {universal.join(" · ")}
                      </span>
                    </td>
                    {tiers.map((tier) => (
                      <td
                        key={tier.key}
                        className="border-b border-border px-3 py-4 align-top sm:px-6"
                      >
                        <Cell value={true} />
                      </td>
                    ))}
                  </tr>
                ) : null}
                {differentiators.map((group) => (
                  <Fragment key={group.section}>
                    <tr>
                      <td
                        colSpan={tiers.length + 1}
                        className="border-b border-border bg-foreground/[0.015] px-3 py-3 font-mono text-[10px] uppercase tracking-[0.18em] text-muted-foreground sm:px-6"
                      >
                        {/* The cell spans the table, so on a phone the label would scroll out of
                            view with the tiers and leave an empty band; pin the text instead. */}
                        <span className="sticky left-3 inline-block sm:left-6 md:static">
                          {group.section}
                        </span>
                      </td>
                    </tr>
                    {group.rows.map((row) => (
                      <tr key={row.label}>
                        <td className="sticky left-0 z-10 border-b border-border bg-surface px-3 py-4 text-sm text-foreground sm:px-6 sm:text-[15px] md:static md:bg-transparent">
                          {row.label}
                        </td>
                        {tiers.map((tier) => (
                          <td
                            key={tier.key}
                            className="border-b border-border px-3 py-4 sm:px-6"
                          >
                            <Cell value={row[tier.key]} />
                          </td>
                        ))}
                      </tr>
                    ))}
                  </Fragment>
                ))}
              </tbody>
              <tfoot>
                <tr>
                  <td className="sticky left-0 z-10 bg-surface px-3 py-5 font-mono text-[10px] uppercase tracking-widest text-muted sm:px-6 md:static md:bg-transparent">
                    Next step
                  </td>
                  {tiers.map((tier) => (
                    <td key={tier.key} className="px-3 py-5 align-top sm:px-6">
                      {/* Wrapping label and full width: a no-wrap button is wider than a
                          phone-width column and the clipped wrapper would cut it off. */}
                      <Button
                        asChild
                        size="sm"
                        variant="outline"
                        className="h-auto w-full whitespace-normal py-2 text-center"
                      >
                        <a
                          href={tier.enterprise ? "#trial" : tier.cta.href}
                          target={
                            !tier.enterprise && tier.cta.external
                              ? "_blank"
                              : undefined
                          }
                          rel={
                            !tier.enterprise && tier.cta.external
                              ? "noreferrer"
                              : undefined
                          }
                        >
                          {tier.enterprise
                            ? `Request a ${TRIAL_WEEKS}-week trial`
                            : tier.cta.label}
                        </a>
                      </Button>
                    </td>
                  ))}
                </tr>
              </tfoot>
            </table>
          </div>
        </Reveal>
      </Section>

      {/* TRIAL */}
      <Section
        id="trial"
        width="default"
        className="scroll-mt-20 border-t border-border/60 bg-surface-elevated/40 py-24"
      >
        <div className="grid items-center gap-12 lg:grid-cols-[1fr_1fr] lg:gap-16">
          <Reveal>
            <Eyebrow className="mb-4">Trial</Eyebrow>
            <h2 className="text-balance text-4xl font-semibold leading-[1.05] tracking-[-0.03em] md:text-5xl">
              Try Enterprise for {TRIAL_WEEKS} weeks.
            </h2>
            <p className="mt-5 text-base leading-relaxed text-muted-foreground md:text-lg">
              A full trial licence for your own infrastructure: the complete
              platform, the admin UI, the MCP server and live engineer support.
              Tell us your work email and company and we send it, usually within
              a day.
            </p>
          </Reveal>
          <Reveal delay={0.1}>
            <TrialRequestForm />
          </Reveal>
        </div>
      </Section>
    </>
  );
}
